use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use log::{info, warn, error, debug};
use crate::types::Config;
use crate::xml_protocol::{
    getinfo_response, session_auth_response, stop_response,
    playlist_clear_response, playlist_add_response, play_response,
};

/// Run the Control Client: wait for XML messages from the Control Server,
/// then forward them to HQPlayer over an authenticated session.
pub fn run_control_client(
    config: Config,
    msg_rx: Receiver<String>,
    stop_flag: Arc<AtomicBool>,
) {
    info!("Control Client ready; waiting for messages from Control Server");

    // Collect messages until we have both PlaylistAdd and Play, then send them
    // in a single authenticated session to HQPlayer
    let mut pending: Vec<String> = Vec::new();

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            info!("Control Client: stop flag set; exiting");
            break;
        }

        let xml = match msg_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(xml) => xml,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // If we have pending messages and no more coming, flush them
                if !pending.is_empty() {
                    send_session(&config, &pending, &stop_flag);
                    pending.clear();
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                info!("Control Client: channel disconnected; exiting");
                break;
            }
        };

        debug!("Control Client: queued message: {}", xml.trim());
        pending.push(xml);
    }
}

/// Open an authenticated session to HQPlayer and send all pending messages.
fn send_session(config: &Config, messages: &[String], stop_flag: &Arc<AtomicBool>) {
    let addr = format!("{}:{}", config.hqplayer_host, config.hqplayer_port);
    info!("Control Client: opening session to HQPlayer at {} ({} messages to send)", addr, messages.len());

    let stream = match TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:4321".parse().unwrap()),
        Duration::from_secs(5),
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("Control Client: failed to connect to HQPlayer at {}: {}", addr, e);
            return;
        }
    };

    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => { error!("Control Client: failed to clone stream: {}", e); return; }
    };
    let mut reader = BufReader::new(stream);

    // Send each message and read the response
    for xml in messages {
        if stop_flag.load(Ordering::Relaxed) { return; }

        debug!("Control Client → HQPlayer: {}", xml.trim());
        if let Err(e) = writer.write_all(xml.as_bytes()) {
            error!("Control Client: write error: {}", e);
            return;
        }

        // Read response
        let mut resp = String::new();
        match reader.read_line(&mut resp) {
            Ok(0) => {
                warn!("Control Client: HQPlayer closed connection");
                return;
            }
            Ok(_) => {
                debug!("Control Client ← HQPlayer: {}", resp.trim());
            }
            Err(e) => {
                warn!("Control Client: read error from HQPlayer: {}", e);
                return;
            }
        }
    }

    info!("Control Client: session complete — all messages sent successfully");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn test_control_client_sends_messages() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = vec![0u8; 4096];
            let n = conn.read(&mut buf).unwrap_or(0);
            // Send a dummy response so the client doesn't hang
            let _ = conn.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistAdd result=\"OK\"/>\n");
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let mut config = Config::default();
        config.hqplayer_host = "127.0.0.1".to_string();
        config.hqplayer_port = port;

        let (tx, rx) = mpsc::channel::<String>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);

        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistAdd secure_uri=\"x\" nonce=\"n\" queued=\"0\" clear=\"0\"/>\n".to_string();
        tx.send(xml).unwrap();
        drop(tx);

        thread::spawn(move || {
            run_control_client(config, rx, stop2);
        });

        let received = handle.join().unwrap();
        assert!(received.contains("PlaylistAdd"), "Expected PlaylistAdd, got: {}", received);

        stop.store(true, Ordering::Relaxed);
    }
}
