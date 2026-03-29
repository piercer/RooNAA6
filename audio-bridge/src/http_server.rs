use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use log::{info, warn, error, debug};
use crate::types::{Config, FormatDescriptor};

/// Run the HTTP Audio Server: listen on config.http_port.
pub fn run_http_server(
    config: Config,
    pcm_rx: Receiver<Vec<u8>>,
    shared_format: Arc<Mutex<FormatDescriptor>>,
    stop_flag: Arc<AtomicBool>,
) {
    let addr = format!("0.0.0.0:{}", config.http_port);
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        error!("Failed to bind HTTP Audio Server on {}: {}", addr, e);
        std::process::exit(1);
    });
    listener.set_nonblocking(true).unwrap_or_else(|e| {
        error!("Failed to set non-blocking on HTTP server listener: {}", e);
        std::process::exit(1);
    });
    info!("HTTP Audio Server listening on {} (HQPlayer fetches PCM here)", addr);

    // We share the PCM receiver across connections — only one GET stream at a time.
    // Wrap in Arc<Mutex> so we can move it into the connection handler.
    let pcm_rx = Arc::new(std::sync::Mutex::new(pcm_rx));

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            info!("HTTP Audio Server: stop flag set; exiting");
            break;
        }

        match listener.accept() {
            Ok((stream, peer)) => {
                debug!("HTTP Audio Server: connection from {}", peer);
                let _ = stream.set_nonblocking(false);
                let pcm_rx_clone = Arc::clone(&pcm_rx);
                let stop_clone = Arc::clone(&stop_flag);
                let fmt_clone = Arc::clone(&shared_format);
                std::thread::spawn(move || {
                    handle_http_connection(stream, pcm_rx_clone, fmt_clone, stop_clone);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                warn!("HTTP Audio Server accept error: {}", e);
            }
        }
    }
}

fn handle_http_connection(
    stream: TcpStream,
    pcm_rx: Arc<std::sync::Mutex<Receiver<Vec<u8>>>>,
    shared_format: Arc<Mutex<FormatDescriptor>>,
    stop_flag: Arc<AtomicBool>,
) {
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
        warn!("HTTP: failed to set read timeout: {}", e);
    }

    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            error!("HTTP: failed to clone stream: {}", e);
            return;
        }
    };

    let mut reader = BufReader::new(stream);

    // Read request line
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let request_line = request_line.trim().to_string();
    debug!("HTTP request: {}", request_line);

    // Drain headers
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if header.trim().is_empty() {
                    break; // blank line = end of headers
                }
            }
        }
    }

    // Parse method and path
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        let _ = writer.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
        return;
    }
    let method = parts[0];
    let path = parts[1];

    // Accept any path ending in stream.raw
    if !path.ends_with("stream.raw") {
        let _ = writer.write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
        return;
    }

    // Build format headers from shared format
    let (sample_rate, channels, raw_format) = {
        let fmt = shared_format.lock().unwrap();
        let raw_fmt = match fmt.bits {
            16 => "int16le",
            24 => "int24le",
            32 => "int32le",
            _ => "int16le",
        };
        (fmt.rate, fmt.channels, raw_fmt.to_string())
    };

    let format_headers = format!(
        "Content-Type: application/x-hqplayer-raw\r\n\
X-HQPlayer-Raw-Title: Roon NAA6 Bridge\r\n\
X-HQPlayer-Raw-Artist: \r\n\
X-HQPlayer-Raw-Album: \r\n\
X-HQPlayer-Raw-SampleRate: {}\r\n\
X-HQPlayer-Raw-Channels: {}\r\n\
X-HQPlayer-Raw-Format: {}\r\n\
Connection: close\r\n",
        sample_rate, channels, raw_format
    );

    match method {
        "HEAD" => {
            info!("HTTP Audio Server: HEAD {} — responding with format headers", path);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n{}\r\n",
                format_headers
            );
            let _ = writer.write_all(response.as_bytes());
        }
        "GET" => {
            info!("HTTP Audio Server: GET {} — starting PCM stream", path);
            let response = format!("HTTP/1.1 200 OK\r\n{}\r\n", format_headers);
            if let Err(e) = writer.write_all(response.as_bytes()) {
                error!("HTTP: failed to write GET response headers: {}", e);
                return;
            }

            // Stream PCM bytes
            let rx = pcm_rx.lock().unwrap();
            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(bytes) => {
                        if let Err(e) = writer.write_all(&bytes) {
                            info!("HTTP Audio Server: HQPlayer disconnected ({})", e);
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // No audio yet — keep waiting
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        info!("HTTP Audio Server: PCM channel disconnected; closing stream");
                        break;
                    }
                }
            }
            info!("HTTP Audio Server: GET stream ended");
        }
        _ => {
            let _ = writer.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn start_test_server(pcm_rx: Receiver<Vec<u8>>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut config = Config::default();
        config.http_port = port;
        let stop = Arc::new(AtomicBool::new(false));
        let fmt = Arc::new(Mutex::new(FormatDescriptor {
            bits: 16, channels: 2, rate: 44100,
            netbuftime: "1.5".to_string(), stream: "pcm".to_string(),
        }));

        thread::spawn(move || {
            run_http_server(config, pcm_rx, fmt, stop);
        });

        // Give server time to bind
        thread::sleep(Duration::from_millis(100));
        port
    }

    #[test]
    fn test_head_request_returns_format_headers() {
        let (_tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
        let port = start_test_server(rx);

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(b"HEAD /stream.raw HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let resp = String::from_utf8_lossy(&buf[..n]);

        assert!(resp.contains("200 OK"), "Expected 200 OK, got: {}", resp);
        assert!(resp.contains("application/x-hqplayer-raw"), "Missing Content-Type");
        assert!(resp.contains("X-HQPlayer-Raw-SampleRate: 44100"), "Missing SampleRate");
        assert!(resp.contains("X-HQPlayer-Raw-Channels: 2"), "Missing Channels");
        assert!(resp.contains("X-HQPlayer-Raw-Format: int16le"), "Missing Format");
        assert!(resp.contains("Content-Length: 0"), "Missing Content-Length for HEAD");
    }

    #[test]
    fn test_get_request_streams_pcm_bytes() {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
        let port = start_test_server(rx);

        let pcm_data = vec![0x01u8, 0x02, 0x03, 0x04];

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(b"GET /tok/stream.raw HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

        // Send PCM data after the connection is established
        thread::sleep(Duration::from_millis(50));
        tx.send(pcm_data.clone()).unwrap();

        // Read headers + body
        let mut buf = vec![0u8; 2048];
        let mut total = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match stream.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    // Check if we have headers + at least 4 body bytes
                    let so_far = &buf[..total];
                    if let Some(pos) = so_far.windows(4).position(|w| w == b"\r\n\r\n") {
                        let body_start = pos + 4;
                        if total >= body_start + pcm_data.len() {
                            break;
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(_) => break,
            }
        }

        let resp_str = String::from_utf8_lossy(&buf[..total]);
        assert!(resp_str.contains("200 OK"), "Expected 200 OK, got: {}", resp_str);
        assert!(resp_str.contains("application/x-hqplayer-raw"), "Missing Content-Type");

        let header_end = buf[..total].windows(4).position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4).unwrap_or(0);
        let body = &buf[header_end..total];
        assert!(body.starts_with(&pcm_data), "Expected PCM bytes in body, got {} bytes", body.len());
    }

    #[test]
    fn test_unknown_path_returns_404() {
        let (_tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
        let port = start_test_server(rx);

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(b"GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

        let mut buf = vec![0u8; 256];
        let n = stream.read(&mut buf).unwrap_or(0);
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.contains("404"), "Expected 404, got: {}", resp);
    }
}
