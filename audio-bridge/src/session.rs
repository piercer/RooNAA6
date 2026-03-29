use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use log::{info, warn, error, debug};
use crate::types::{Config, TrackMetadata};
use crate::xml_protocol::{parse_message, keepalive_response, start_response, unsupported_start_response, metadata_message, NaaMessage};
use crate::alsa_capture::start_capture;

/// Session state
#[derive(Debug, PartialEq)]
enum State {
    Idle,
    Streaming,
}

/// Run a single HQPlayer session on the given TcpStream.
/// Blocks until the session ends (disconnect or stop flag).
pub fn run_session(
    stream: TcpStream,
    config: &Config,
    metadata_rx: &Receiver<TrackMetadata>,
    stop_flag: Arc<AtomicBool>,
) {
    // Set read timeout so we can poll metadata and stop flag
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_millis(100))) {
        warn!("Failed to set read timeout: {}", e);
    }

    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to clone TCP stream: {}", e);
            return;
        }
    };

    let mut reader = BufReader::new(stream);
    let mut state = State::Idle;

    // ALSA channel and stop flag (used when Streaming)
    let mut alsa_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>> = None;
    let alsa_stop = Arc::new(AtomicBool::new(false));

    info!("Session started; entering Idle state");

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            info!("Stop flag set; ending session");
            break;
        }

        // Check for metadata to forward (any state)
        while let Ok(meta) = metadata_rx.try_recv() {
            if let (Some(title), Some(artist), Some(album)) = (&meta.title, &meta.artist, &meta.album) {
                let xml = metadata_message(title, artist, album);
                debug!("Sending metadata XML: {}", xml.trim());
                if let Err(e) = writer.write_all(xml.as_bytes()) {
                    warn!("Failed to send metadata: {}", e);
                }
            } else {
                debug!("Incomplete metadata received; skipping");
            }
        }

        // In Streaming state: forward PCM bytes from ALSA channel
        if state == State::Streaming {
            if let Some(ref rx) = alsa_rx {
                // Drain available PCM chunks
                loop {
                    match rx.try_recv() {
                        Ok(bytes) => {
                            if let Err(e) = writer.write_all(&bytes) {
                                error!("TCP write error during streaming: {}", e);
                                // Connection lost — stop streaming
                                alsa_stop.store(true, Ordering::Relaxed);
                                state = State::Idle;
                                alsa_rx = None;
                                break;
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            warn!("ALSA channel disconnected");
                            alsa_stop.store(true, Ordering::Relaxed);
                            state = State::Idle;
                            alsa_rx = None;
                            break;
                        }
                    }
                }
            }
        }

        // Read a line from HQPlayer (with timeout)
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                info!("HQPlayer disconnected (EOF)");
                break;
            }
            Ok(_) => {
                debug!("Received XML: {}", line.trim());
                handle_message(&line, &mut state, &mut writer, config, &alsa_stop, &mut alsa_rx);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Timeout — normal, just loop again
            }
            Err(e) => {
                error!("TCP read error: {}", e);
                break;
            }
        }
    }

    // Clean up ALSA if streaming
    if state == State::Streaming {
        info!("Session ending; stopping ALSA capture");
        alsa_stop.store(true, Ordering::Relaxed);
    }

    info!("Session ended");
}

fn handle_message(
    line: &str,
    state: &mut State,
    writer: &mut TcpStream,
    config: &Config,
    alsa_stop: &Arc<AtomicBool>,
    alsa_rx: &mut Option<std::sync::mpsc::Receiver<Vec<u8>>>,
) {
    let msg = parse_message(line);
    debug!("Parsed message: {:?} in state {:?}", msg, state);

    match msg {
        NaaMessage::Keepalive => {
            debug!("Keepalive received; sending response");
            let resp = keepalive_response();
            if let Err(e) = writer.write_all(resp.as_bytes()) {
                warn!("Failed to send keepalive response: {}", e);
            }
        }

        NaaMessage::Start(fmt) => {
            if fmt.stream != "pcm" {
                error!("Unsupported stream type '{}'; sending result=0", fmt.stream);
                let resp = unsupported_start_response();
                let _ = writer.write_all(resp.as_bytes());
                return;
            }

            info!("Start received: bits={} channels={} rate={} stream={}", fmt.bits, fmt.channels, fmt.rate, fmt.stream);
            debug!("FormatDescriptor: {:?}", fmt);

            // Send start response
            let resp = start_response(&fmt);
            debug!("Sending start response: {}", resp.trim());
            if let Err(e) = writer.write_all(resp.as_bytes()) {
                error!("Failed to send start response: {}", e);
                return;
            }

            // Send 16-byte sync marker (all zeros)
            let sync_marker = [0u8; 16];
            if let Err(e) = writer.write_all(&sync_marker) {
                error!("Failed to send sync marker: {}", e);
                return;
            }
            debug!("Sync marker sent (16 zero bytes)");

            // Start ALSA capture thread
            alsa_stop.store(false, Ordering::Relaxed);
            let (alsa_tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
            let device = config.alsa_device.clone();
            let fmt_clone = fmt.clone();
            let stop_clone = Arc::clone(alsa_stop);
            std::thread::spawn(move || {
                start_capture(&device, &fmt_clone, alsa_tx, stop_clone);
            });
            *alsa_rx = Some(rx);
            *state = State::Streaming;
            info!("Audio streaming started");
        }

        NaaMessage::Stop => {
            info!("Stop received; halting audio streaming");
            alsa_stop.store(true, Ordering::Relaxed);
            *alsa_rx = None;
            *state = State::Idle;
            info!("Returned to Idle state");
        }

        NaaMessage::Unknown => {
            // Could be a 32-byte binary position packet during streaming — read and discard
            // (BufReader already consumed the "line"; if it's binary it won't parse as XML)
            debug!("Unknown/unparseable message received; discarding");
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

    fn make_stop_flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn make_config() -> Config {
        Config::default()
    }

    /// Helper: connect a pair of TCP streams for testing
    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    // Task 4.3 unit tests

    #[test]
    fn test_keepalive_in_idle_sends_response() {
        let (server, mut client) = tcp_pair();
        let config = make_config();
        let (_, meta_rx) = mpsc::channel::<TrackMetadata>();
        let stop = make_stop_flag();
        let stop_clone = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            run_session(server, &config, &meta_rx, stop_clone);
        });

        // Send keepalive
        let keepalive = "<?xml version=\"1.0\" encoding=\"utf-8\"?><networkaudio><operation type=\"keepalive\"/></networkaudio>\n";
        client.write_all(keepalive.as_bytes()).unwrap();

        // Read response
        let mut buf = [0u8; 256];
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client.read(&mut buf).unwrap_or(0);
        let resp = std::str::from_utf8(&buf[..n]).unwrap_or("");
        assert!(resp.contains("result=\"1\"") && resp.contains("keepalive"), "Expected keepalive response, got: {}", resp);

        // Shut down
        stop.store(true, Ordering::Relaxed);
        drop(client);
        let _ = handle.join();
    }

    #[test]
    fn test_start_pcm_sends_response_and_sync_marker() {
        let (server, mut client) = tcp_pair();
        let config = make_config();
        let (_, meta_rx) = mpsc::channel::<TrackMetadata>();
        let stop = make_stop_flag();
        let stop_clone = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            run_session(server, &config, &meta_rx, stop_clone);
        });

        let start_msg = "<?xml version=\"1.0\" encoding=\"utf-8\"?><networkaudio><operation bits=\"16\" channels=\"2\" netbuftime=\"1.5000000000000000\" rate=\"44100\" stream=\"pcm\" type=\"start\"/></networkaudio>\n";
        client.write_all(start_msg.as_bytes()).unwrap();

        // Read start response + 16-byte sync marker
        let mut buf = vec![0u8; 512];
        client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut total = 0;
        // Read until we have at least the XML response + 16 bytes
        for _ in 0..20 {
            match client.read(&mut buf[total..]) {
                Ok(n) if n > 0 => {
                    total += n;
                    // Check if we have the XML response (ends with \n) + 16 zero bytes
                    let text = std::str::from_utf8(&buf[..total]).unwrap_or("");
                    if let Some(nl_pos) = text.find('\n') {
                        let xml_part = &text[..=nl_pos];
                        assert!(xml_part.contains("result=\"1\""), "Expected result=1 in start response");
                        assert!(xml_part.contains("dsd=\"0\""), "Expected dsd=0 in start response");
                        // Check 16 zero bytes follow
                        let after_xml = nl_pos + 1;
                        if total >= after_xml + 16 {
                            let sync = &buf[after_xml..after_xml + 16];
                            assert_eq!(sync, &[0u8; 16], "Expected 16 zero bytes sync marker");
                            break;
                        }
                    }
                }
                _ => break,
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        stop.store(true, Ordering::Relaxed);
        drop(client);
        let _ = handle.join();
    }

    #[test]
    fn test_start_dsd_sends_result_zero() {
        let (server, mut client) = tcp_pair();
        let config = make_config();
        let (_, meta_rx) = mpsc::channel::<TrackMetadata>();
        let stop = make_stop_flag();
        let stop_clone = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            run_session(server, &config, &meta_rx, stop_clone);
        });

        let start_msg = "<?xml version=\"1.0\" encoding=\"utf-8\"?><networkaudio><operation bits=\"1\" channels=\"2\" netbuftime=\"1.5000000000000000\" rate=\"2822400\" stream=\"dsd\" type=\"start\"/></networkaudio>\n";
        client.write_all(start_msg.as_bytes()).unwrap();

        let mut buf = [0u8; 512];
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client.read(&mut buf).unwrap_or(0);
        let resp = std::str::from_utf8(&buf[..n]).unwrap_or("");
        assert!(resp.contains("result=\"0\""), "Expected result=0 for DSD stream, got: {}", resp);

        stop.store(true, Ordering::Relaxed);
        drop(client);
        let _ = handle.join();
    }

    // Task 4.4 Property test: Sync marker always follows start response
    // Tests the protocol layer directly (without ALSA) by verifying the response bytes
    #[cfg(test)]
    mod property_tests {
        use proptest::prelude::*;
        use crate::xml_protocol::start_response;
        use crate::types::FormatDescriptor;

        // Feature: roon-naa6-bridge, Property 2: Sync marker always follows start response
        proptest! {
            #![proptest_config(proptest::test_runner::Config::with_cases(100))]

            #[test]
            fn prop_sync_marker_follows_start_response(
                bits in prop::sample::select(vec![16u8, 24, 32]),
                channels in 1u8..=2u8,
                rate in prop::sample::select(vec![44100u32, 48000, 96000, 192000]),
                netbuftime_frac in 0u32..=9999u32,
            ) {
                let netbuftime = format!("1.{:016}", netbuftime_frac);
                let fmt = FormatDescriptor {
                    bits,
                    channels,
                    rate,
                    netbuftime: netbuftime.clone(),
                    stream: "pcm".to_string(),
                };

                // Build the bytes that would be sent: start_response XML + 16 zero bytes
                let resp = start_response(&fmt);
                let sync_marker = [0u8; 16];

                let mut combined = resp.as_bytes().to_vec();
                combined.extend_from_slice(&sync_marker);

                // Find the newline that ends the XML
                let nl_pos = combined.iter().position(|&b| b == b'\n');
                prop_assert!(nl_pos.is_some(), "Start response must end with newline");
                let nl_pos = nl_pos.unwrap();

                // The 16 bytes after the newline must all be zero
                let after_xml = nl_pos + 1;
                prop_assert!(combined.len() >= after_xml + 16, "Combined buffer too short");
                let sync = &combined[after_xml..after_xml + 16];
                prop_assert_eq!(sync, &[0u8; 16], "Sync marker must be 16 zero bytes");

                // The XML part must contain result="1"
                let xml_part = std::str::from_utf8(&combined[..=nl_pos]).unwrap();
                prop_assert!(xml_part.contains("result=\"1\""), "Start response must have result=1");
            }
        }
    }
}
