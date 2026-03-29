use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::sync::mpsc::Sender;
use log::{info, warn, error, debug};
use crate::types::TrackMetadata;

/// Start the IPC listener on the given Unix domain socket path.
/// Accepts connections and reads newline-delimited JSON metadata messages.
/// Sends parsed TrackMetadata to `tx`.
/// Runs forever (call from a dedicated thread).
pub fn start_ipc_listener(socket_path: &str, tx: Sender<TrackMetadata>) {
    // Remove stale socket file if it exists
    let _ = std::fs::remove_file(socket_path);

    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => {
            info!("IPC listener bound to {}", socket_path);
            l
        }
        Err(e) => {
            error!("Failed to bind IPC socket at {}: {}", socket_path, e);
            return;
        }
    };

    for stream in listener.incoming() {
        match stream {
            Ok(conn) => {
                debug!("IPC client connected");
                let tx_clone = tx.clone();
                std::thread::spawn(move || {
                    handle_ipc_connection(conn, tx_clone);
                });
            }
            Err(e) => {
                warn!("IPC accept error: {}", e);
            }
        }
    }
}

fn handle_ipc_connection(conn: std::os::unix::net::UnixStream, tx: Sender<TrackMetadata>) {
    let reader = BufReader::new(conn);
    for line in reader.lines() {
        match line {
            Ok(json_str) => {
                let json_str = json_str.trim().to_string();
                if json_str.is_empty() {
                    continue;
                }
                debug!("IPC received: {}", json_str);
                match serde_json::from_str::<TrackMetadata>(&json_str) {
                    Ok(meta) => {
                        debug!("Parsed metadata: title={:?} artist={:?} album={:?}", meta.title, meta.artist, meta.album);
                        if tx.send(meta).is_err() {
                            debug!("Metadata channel closed; stopping IPC handler");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse IPC JSON '{}': {}", json_str, e);
                    }
                }
            }
            Err(e) => {
                debug!("IPC connection closed: {}", e);
                break;
            }
        }
    }
    debug!("IPC connection handler exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    fn temp_socket_path(dir: &TempDir) -> String {
        dir.path().join("test.sock").to_string_lossy().to_string()
    }

    #[test]
    fn test_valid_json_metadata_forwarded() {
        let dir = TempDir::new().unwrap();
        let path = temp_socket_path(&dir);
        let (tx, rx) = mpsc::channel::<TrackMetadata>();

        let path_clone = path.clone();
        std::thread::spawn(move || {
            start_ipc_listener(&path_clone, tx);
        });

        // Wait for socket to be ready
        std::thread::sleep(Duration::from_millis(100));

        let mut conn = UnixStream::connect(&path).unwrap();
        let json = r#"{"title":"My Song","artist":"My Artist","album":"My Album"}"#;
        writeln!(conn, "{}", json).unwrap();

        let meta = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(meta.title.as_deref(), Some("My Song"));
        assert_eq!(meta.artist.as_deref(), Some("My Artist"));
        assert_eq!(meta.album.as_deref(), Some("My Album"));
    }

    #[test]
    fn test_malformed_json_does_not_crash() {
        let dir = TempDir::new().unwrap();
        let path = temp_socket_path(&dir);
        let (tx, rx) = mpsc::channel::<TrackMetadata>();

        let path_clone = path.clone();
        std::thread::spawn(move || {
            start_ipc_listener(&path_clone, tx);
        });

        std::thread::sleep(Duration::from_millis(100));

        let mut conn = UnixStream::connect(&path).unwrap();
        // Send malformed JSON first, then valid JSON
        writeln!(conn, "{{not valid json}}").unwrap();
        writeln!(conn, r#"{{"title":"Good","artist":"Artist","album":"Album"}}"#).unwrap();

        // Should receive the valid one
        let meta = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Good"));
    }

    // Task 6.4 Property test: Metadata UTF-8 round-trip
    #[cfg(test)]
    mod property_tests {
        use proptest::prelude::*;
        use crate::types::TrackMetadata;

        // Feature: roon-naa6-bridge, Property 4: Metadata UTF-8 round-trip
        proptest! {
            #![proptest_config(proptest::test_runner::Config::with_cases(100))]

            #[test]
            fn prop_metadata_utf8_round_trip(
                title in ".*",
                artist in ".*",
                album in ".*",
            ) {
                let meta = TrackMetadata {
                    title: Some(title.clone()),
                    artist: Some(artist.clone()),
                    album: Some(album.clone()),
                };
                let json = serde_json::to_string(&meta).unwrap();
                let decoded: TrackMetadata = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(decoded.title.as_deref(), Some(title.as_str()));
                prop_assert_eq!(decoded.artist.as_deref(), Some(artist.as_str()));
                prop_assert_eq!(decoded.album.as_deref(), Some(album.as_str()));
            }
        }
    }
}
