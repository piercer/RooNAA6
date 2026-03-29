use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::mpsc::Sender;
use std::time::Duration;
use log::{info, warn, error, debug};
use crate::types::Config;
use crate::xml_protocol::{
    parse_message, HqpMessage,
    getinfo_response, stop_response, status_response,
    volume_range_response, state_response, playlist_clear_response, playlist_add_response,
    play_response, get_modes_response, get_filters_response, get_shapers_response, get_rates_response,
};

/// Current playback state shared across keepalive responses
#[derive(Debug, Clone)]
struct PlaybackState {
    bits: u8,
    channels: u8,
    rate: u32,
}

impl Default for PlaybackState {
    fn default() -> Self {
        PlaybackState { bits: 16, channels: 2, rate: 44100 }
    }
}

/// Proxy a single XML message to HQPlayer and return its response line.
/// Used to forward SessionAuthentication so Roon gets a cryptographically valid response.
fn proxy_to_hqplayer(config: &Config, xml_line: &str) -> Option<String> {
    let addr = format!("{}:{}", config.hqplayer_host, config.hqplayer_port);
    match TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:4321".parse().unwrap()),
        Duration::from_secs(5),
    ) {
        Ok(stream) => {
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut writer = match stream.try_clone() {
                Ok(s) => s,
                Err(e) => { warn!("Auth proxy: failed to clone stream: {}", e); return None; }
            };
            let mut reader = BufReader::new(stream);

            // Send the message to HQPlayer
            if let Err(e) = writer.write_all(xml_line.as_bytes()) {
                warn!("Auth proxy: failed to send to HQPlayer: {}", e);
                return None;
            }

            // Read HQPlayer's response
            let mut response = String::new();
            match reader.read_line(&mut response) {
                Ok(0) | Err(_) => {
                    warn!("Auth proxy: no response from HQPlayer");
                    None
                }
                Ok(_) => {
                    debug!("Auth proxy ← HQPlayer: {}", response.trim());
                    Some(response)
                }
            }
        }
        Err(e) => {
            warn!("Auth proxy: failed to connect to HQPlayer at {}: {}", addr, e);
            None
        }
    }
}

/// Run the Control Server: listen on config.listen_port, accept Roon connections,
/// handle the HQPlayer XML session protocol.
///
/// When a PlaylistAdd is received from Roon, the stream URL is sent via `playlist_tx`
/// so the Control Client can forward it to HQPlayer.
pub fn run_control_server(
    config: Config,
    playlist_tx: Sender<String>,
    stream_url: String,
    stop_flag: Arc<AtomicBool>,
) {
    let addr = format!("0.0.0.0:{}", config.listen_port);
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        error!("Failed to bind Control Server on {}: {}", addr, e);
        std::process::exit(1);
    });
    listener.set_nonblocking(true).unwrap_or_else(|e| {
        error!("Failed to set non-blocking on Control Server listener: {}", e);
        std::process::exit(1);
    });
    info!("Control Server listening on {} (Roon connects here)", addr);

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            info!("Control Server: stop flag set; exiting accept loop");
            break;
        }

        match listener.accept() {
            Ok((stream, peer)) => {
                info!("Roon connection accepted from {}", peer);
                let _ = stream.set_nonblocking(false);
                handle_roon_session(stream, &playlist_tx, &stream_url, &config, &stop_flag);
                info!("Roon disconnected; resuming listen");
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                warn!("Control Server accept error: {}", e);
            }
        }
    }
}

fn handle_roon_session(
    stream: TcpStream,
    playlist_tx: &Sender<String>,
    stream_url: &str,
    config: &Config,
    stop_flag: &Arc<AtomicBool>,
) {
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_millis(200))) {
        warn!("Failed to set read timeout on Roon stream: {}", e);
    }

    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to clone Roon TCP stream: {}", e);
            return;
        }
    };

    let mut reader = BufReader::new(stream);
    let mut state = PlaybackState::default();

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                info!("Roon disconnected (EOF)");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                debug!("Control Server ← Roon: {}", trimmed);

                let msg = parse_message(trimmed);
                let response = dispatch_message(msg, trimmed, &mut state, stream_url, playlist_tx, config);

                if let Some(resp) = response {
                    debug!("Control Server → Roon: {}", resp.trim());
                    if let Err(e) = writer.write_all(resp.as_bytes()) {
                        error!("Failed to write response to Roon: {}", e);
                        break;
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Timeout — normal, loop again
            }
            Err(e) => {
                error!("Control Server TCP read error: {}", e);
                break;
            }
        }
    }
}

fn dispatch_message(
    msg: HqpMessage,
    raw_line: &str,
    state: &mut PlaybackState,
    stream_url: &str,
    playlist_tx: &Sender<String>,
    config: &Config,
) -> Option<String> {
    match msg {
        HqpMessage::GetInfo => {
            info!("Control Server: GetInfo received — proxying to HQPlayer for real response");
            // Proxy to get HQPlayer's real identity (Roon may verify engine version)
            if let Some(resp) = proxy_to_hqplayer(config, raw_line) {
                return Some(resp);
            }
            // Fallback if HQPlayer unreachable
            Some(getinfo_response())
        }
        HqpMessage::SessionAuthentication { .. } => {
            info!("Control Server: SessionAuthentication received — proxying to HQPlayer for signed response");
            // Must proxy — Roon verifies the cryptographic signature
            if let Some(resp) = proxy_to_hqplayer(config, raw_line) {
                return Some(resp);
            }
            warn!("Control Server: HQPlayer unreachable for auth proxy; Roon will likely disconnect");
            None
        }
        HqpMessage::Stop => {
            info!("Control Server: Stop received");
            Some(stop_response())
        }
        HqpMessage::Status { subscribe } => {
            debug!("Control Server: Status received (subscribe={})", subscribe);
            Some(status_response(state.bits, state.channels, state.rate))
        }
        HqpMessage::VolumeRange => {
            debug!("Control Server: VolumeRange received");
            Some(volume_range_response())
        }
        HqpMessage::State => {
            debug!("Control Server: State received");
            Some(state_response(state.rate))
        }
        HqpMessage::PlaylistClear => {
            info!("Control Server: PlaylistClear received");
            Some(playlist_clear_response())
        }
        HqpMessage::PlaylistAdd { ref secure_uri, ref nonce } => {
            info!("Control Server: PlaylistAdd received from Roon; sending our HTTP stream URL to HQPlayer");
            // Send our own URI (not Roon's secure_uri) so HQPlayer fetches from our HTTP server
            let our_playlist_add = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistAdd uri=\"{}\" queued=\"0\" clear=\"1\"/>\n",
                stream_url
            );
            if let Err(e) = playlist_tx.send(our_playlist_add) {
                warn!("Failed to send PlaylistAdd to Control Client: {}", e);
            }
            Some(playlist_add_response())
        }
        HqpMessage::Play => {
            info!("Control Server: Play received from Roon — forwarding to HQPlayer");
            let play_xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><Play/>\n".to_string();
            if let Err(e) = playlist_tx.send(play_xml) {
                warn!("Failed to send Play to Control Client: {}", e);
            }
            Some(play_response())
        }
        HqpMessage::GetModes => {
            debug!("Control Server: GetModes received");
            Some(get_modes_response())
        }
        HqpMessage::GetFilters => {
            debug!("Control Server: GetFilters received");
            Some(get_filters_response())
        }
        HqpMessage::GetShapers => {
            debug!("Control Server: GetShapers received");
            Some(get_shapers_response())
        }
        HqpMessage::GetRates => {
            debug!("Control Server: GetRates received");
            Some(get_rates_response())
        }
        HqpMessage::Unknown => {
            debug!("Control Server: Unknown message received; ignoring");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    fn stop_flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn make_config() -> Config {
        Config::default()
    }

    #[test]
    fn test_getinfo_response_sent() {
        let (server, mut client) = tcp_pair();
        let (tx, _rx) = mpsc::channel::<String>();
        let flag = stop_flag();
        let flag2 = Arc::clone(&flag);
        let config = make_config();

        thread::spawn(move || {
            handle_roon_session(server, &tx, "http://127.0.0.1:30001/tok/stream.raw", &config, &flag2);
        });

        client.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n").unwrap();

        let mut buf = [0u8; 512];
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client.read(&mut buf).unwrap_or(0);
        let resp = std::str::from_utf8(&buf[..n]).unwrap_or("");
        assert!(resp.contains("HQPlayerEmbedded"), "Expected HQPlayerEmbedded in response, got: {}", resp);

        flag.store(true, Ordering::Relaxed);
        drop(client);
    }

    #[test]
    fn test_session_auth_response_ok() {
        // This test verifies the session auth message is handled (proxy may fail in test env)
        let (server, mut client) = tcp_pair();
        let (tx, _rx) = mpsc::channel::<String>();
        let flag = stop_flag();
        let flag2 = Arc::clone(&flag);
        let mut config = make_config();
        // Point to a non-existent host so proxy fails fast and we get fallback
        config.hqplayer_host = "127.0.0.1".to_string();
        config.hqplayer_port = 1; // nothing listening here

        thread::spawn(move || {
            handle_roon_session(server, &tx, "http://127.0.0.1:30001/tok/stream.raw", &config, &flag2);
        });

        client.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><SessionAuthentication client_id=\"x\" public_key=\"y\" signature=\"z\"/>\n").unwrap();

        // With proxy failing, we get no response (None returned) — just verify no crash
        let mut buf = [0u8; 512];
        client.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let _ = client.read(&mut buf); // may timeout, that's fine

        flag.store(true, Ordering::Relaxed);
        drop(client);
    }

    #[test]
    fn test_playlist_add_triggers_channel() {
        let (server, mut client) = tcp_pair();
        let (tx, rx) = mpsc::channel::<String>();
        let flag = stop_flag();
        let flag2 = Arc::clone(&flag);
        let url = "http://127.0.0.1:30001/tok/stream.raw";
        let config = make_config();

        thread::spawn(move || {
            handle_roon_session(server, &tx, url, &config, &flag2);
        });

        client.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistAdd secure_uri=\"roon://x\" nonce=\"n\" queued=\"0\" clear=\"0\"/>\n").unwrap();

        // Should receive the forwarded PlaylistAdd XML on the channel
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(received.contains("PlaylistAdd"), "Expected PlaylistAdd XML, got: {}", received);
        assert!(received.contains("secure_uri"), "Expected secure_uri in forwarded XML, got: {}", received);

        // Should also get PlaylistAdd result=OK back
        let mut buf = [0u8; 512];
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client.read(&mut buf).unwrap_or(0);
        let resp = std::str::from_utf8(&buf[..n]).unwrap_or("");
        assert!(resp.contains("<PlaylistAdd result=\"OK\"/>"), "Expected PlaylistAdd OK, got: {}", resp);

        flag.store(true, Ordering::Relaxed);
        drop(client);
    }

    #[test]
    fn test_keepalive_volume_range_state_respond() {
        let (server, mut client) = tcp_pair();
        let (tx, _rx) = mpsc::channel::<String>();
        let flag = stop_flag();
        let flag2 = Arc::clone(&flag);
        let config = make_config();

        thread::spawn(move || {
            handle_roon_session(server, &tx, "http://127.0.0.1:30001/tok/stream.raw", &config, &flag2);
        });

        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        // VolumeRange keepalive
        client.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><VolumeRange/>\n").unwrap();
        let mut buf = [0u8; 512];
        let n = client.read(&mut buf).unwrap_or(0);
        let resp = std::str::from_utf8(&buf[..n]).unwrap_or("");
        assert!(resp.contains("VolumeRange"), "Expected VolumeRange response, got: {}", resp);

        flag.store(true, Ordering::Relaxed);
        drop(client);
    }
}
