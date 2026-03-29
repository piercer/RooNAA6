mod types;
mod config_reader;
mod xml_protocol;
mod alsa_capture;
mod ipc_listener;
mod shutdown;
mod control_server;
mod control_client;
mod http_server;

use std::sync::mpsc;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use log::{info, error};
use crate::config_reader::load_config;
use crate::ipc_listener::start_ipc_listener;
use crate::shutdown::ShutdownHandler;
use crate::types::{FormatDescriptor, TrackMetadata};
use crate::alsa_capture::start_capture;
use crate::control_server::run_control_server;
use crate::control_client::run_control_client;
use crate::http_server::run_http_server;

/// Detect the local IP address on the network toward `hqplayer_host`.
/// Uses the UDP connect trick — no packets are sent.
fn detect_local_ip(hqplayer_host: &str) -> String {
    let target = format!("{}:80", hqplayer_host);
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect(&target).is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    // Fallback
    "127.0.0.1".to_string()
}

fn main() {
    // Load configuration
    let config = load_config();

    // Initialise logger
    let log_level = match config.log_level.as_str() {
        "debug" => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    };
    env_logger::Builder::new()
        .filter_level(log_level)
        .init();

    info!("naa6-audio-bridge starting up");
    info!(
        "Config: outputName={}, alsaDevice={}, listenPort={}, hqplayerHost={}:{}, httpPort={}, reconnectBackoff={}ms",
        config.output_name, config.alsa_device, config.listen_port,
        config.hqplayer_host, config.hqplayer_port,
        config.http_port, config.reconnect_backoff
    );

    // Set up shutdown handler
    let shutdown = ShutdownHandler::new().unwrap_or_else(|e| {
        error!("Failed to register signal handlers: {}", e);
        std::process::exit(1);
    });
    let stop_flag: Arc<AtomicBool> = shutdown.flag();

    // Detect our local IP for the HTTP stream URL
    let local_ip = detect_local_ip(&config.hqplayer_host);
    let stream_token = uuid::Uuid::new_v4().to_string();
    let stream_url = format!("http://{}:{}/{}/stream.raw", local_ip, config.http_port, stream_token);
    info!("HTTP stream URL: {}", stream_url);

    // ── Channels ────────────────────────────────────────────────────────────

    // IPC metadata channel: ipc_listener → (unused in new arch, kept for future use)
    let (meta_tx, _meta_rx) = mpsc::channel::<TrackMetadata>();

    // PCM audio channel: alsa_capture → http_server
    let (pcm_tx, pcm_rx) = mpsc::sync_channel::<Vec<u8>>(64);

    // Shared audio format: alsa_capture writes, http_server reads
    let shared_format = Arc::new(Mutex::new(FormatDescriptor {
        bits: 16,
        channels: 2,
        rate: 44100,
        netbuftime: "1.5000000000000000".to_string(),
        stream: "pcm".to_string(),
    }));

    // PlaylistAdd signal: control_server → control_client
    let (playlist_tx, playlist_rx) = mpsc::channel::<String>();

    // ── Threads ─────────────────────────────────────────────────────────────

    // IPC listener (metadata from roon-metadata-extension)
    let ipc_socket = config.ipc_socket.clone();
    std::thread::spawn(move || {
        start_ipc_listener(&ipc_socket, meta_tx);
    });

    // ALSA capture → PCM channel
    {
        let alsa_device = config.alsa_device.clone();
        let stop_clone = Arc::clone(&stop_flag);
        let fmt = FormatDescriptor {
            bits: 16,
            channels: 2,
            rate: 44100,
            netbuftime: "1.5000000000000000".to_string(),
            stream: "pcm".to_string(),
        };
        std::thread::spawn(move || {
            start_capture(&alsa_device, &fmt, pcm_tx, stop_clone);
        });
    }

    // HTTP Audio Server
    {
        let config_clone = config.clone();
        let stop_clone = Arc::clone(&stop_flag);
        let fmt_clone = Arc::clone(&shared_format);
        std::thread::spawn(move || {
            run_http_server(config_clone, pcm_rx, fmt_clone, stop_clone);
        });
    }

    // Control Client (connects to HQPlayer when signalled)
    {
        let config_clone = config.clone();
        let stop_clone = Arc::clone(&stop_flag);
        std::thread::spawn(move || {
            run_control_client(config_clone, playlist_rx, stop_clone);
        });
    }

    // Control Server (main loop — accepts Roon connections)
    run_control_server(config, playlist_tx, stream_url, stop_flag);

    info!("naa6-audio-bridge shutting down");
    std::process::exit(0);
}
