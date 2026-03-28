mod types;
mod config_reader;
mod naa6_client;
mod alsa_capture;
mod format_detector;
mod backpressure;
mod shutdown;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use log::{info, warn, error};
use crate::config_reader::load_config;
use crate::naa6_client::Naa6Client;
use crate::alsa_capture::AlsaCaptureReader;
use crate::format_detector::FormatDetector;
use crate::backpressure::BackpressureController;
use crate::shutdown::{ShutdownHandler, run_shutdown_sequence};

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
    info!("Config: outputName={}, alsaDevice={}, hqplayerHost={}:{}, reconnectBackoff={}ms",
        config.output_name, config.alsa_device,
        config.hqplayer_host, config.hqplayer_port,
        config.reconnect_backoff);

    // Set up shutdown handler
    let shutdown = ShutdownHandler::new().unwrap_or_else(|e| {
        error!("Failed to register signal handlers: {}", e);
        std::process::exit(1);
    });

    // Set up backpressure controller
    let backpressure = Arc::new(Mutex::new(BackpressureController::new()));

    // Set up format detector
    let mut format_detector = FormatDetector::new();

    // Set up NAA6 client
    let mut naa6 = Naa6Client::new(config.clone());

    // Set up ALSA capture reader
    let mut alsa = AlsaCaptureReader::new(config.clone(), Arc::clone(&backpressure));

    // Open ALSA device
    if let Err(e) = alsa.open() {
        error!("Fatal: failed to open ALSA device: {}", e);
        std::process::exit(1);
    }

    // Main loop: connect to HQPlayer and forward audio
    'outer: loop {
        if shutdown.should_shutdown() {
            break 'outer;
        }

        // Connect to HQPlayer
        if let Err(e) = naa6.connect() {
            error!("Failed to connect to HQPlayer: {} — retrying in {}ms", e, config.reconnect_backoff);
            std::thread::sleep(Duration::from_millis(config.reconnect_backoff));
            continue;
        }

        // Perform handshake with current format
        let fmt = match alsa.current_format() {
            Some(f) => f.clone(),
            None => {
                warn!("No format detected yet; waiting...");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };

        if let Err(e) = naa6.handshake(&fmt) {
            error!("NAA6 handshake failed: {} — retrying in {}ms", e, config.reconnect_backoff);
            naa6.disconnect();
            std::thread::sleep(Duration::from_millis(config.reconnect_backoff));
            continue;
        }

        info!("NAA6 session established; forwarding audio");

        // Audio forwarding loop
        loop {
            if shutdown.should_shutdown() {
                break 'outer;
            }

            // Tick keepalive
            if let Err(e) = naa6.tick_keepalive() {
                error!("Keepalive failed: {} — reconnecting", e);
                naa6.disconnect();
                break;
            }

            // Read frames from ALSA
            match alsa.read_frames() {
                Ok((bytes, format_changed, new_fmt)) => {
                    // Handle format change: send format-change message before audio
                    if format_changed {
                        if let Some(ref fmt) = new_fmt {
                            if let Err(e) = naa6.send_format_change(fmt) {
                                error!("Failed to send format change: {} — reconnecting", e);
                                naa6.disconnect();
                                break;
                            }
                        }
                    }

                    // Forward audio bytes
                    if !bytes.is_empty() {
                        if let Err(e) = naa6.send_audio(&bytes) {
                            if e.kind() == std::io::ErrorKind::WouldBlock {
                                // Backpressure: pause ALSA
                                if let Ok(mut bp) = backpressure.lock() {
                                    bp.on_buffer_full();
                                }
                                // Wait briefly for socket to drain
                                std::thread::sleep(Duration::from_millis(10));
                                if let Ok(mut bp) = backpressure.lock() {
                                    bp.on_socket_drain();
                                }
                            } else {
                                error!("Failed to send audio: {} — reconnecting", e);
                                naa6.disconnect();
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("ALSA read error: {} — continuing", e);
                    // Don't break the session on ALSA errors (Req 2.8)
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        // Reconnect after backoff
        if !shutdown.should_shutdown() {
            warn!("NAA6 connection lost; reconnecting in {}ms", config.reconnect_backoff);
            std::thread::sleep(Duration::from_millis(config.reconnect_backoff));
        }
    }

    // Graceful shutdown
    info!("Shutdown signal received; starting graceful shutdown");
    run_shutdown_sequence(
        || naa6.send_termination(),
        || naa6.disconnect(),
        || alsa.close(),
    );
}
