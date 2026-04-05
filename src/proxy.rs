use std::io::{Read, Write};
use std::net::TcpStream;

use crate::metadata::SharedMetadata;
use crate::ts;

/// Forward T8→HQP: simple byte passthrough.
pub fn forward_passthrough(mut src: TcpStream, mut dst: TcpStream, label: &str) {
    let mut buf = [0u8; 65536];
    loop {
        match src.read(&mut buf) {
            Ok(0) => {
                eprintln!("{} [{}] EOF", ts(), label);
                break;
            }
            Ok(n) => {
                if let Err(e) = dst.write_all(&buf[..n]) {
                    eprintln!("{} [{}] write error: {}", ts(), label, e);
                    break;
                }
            }
            Err(e) => {
                eprintln!("{} [{}] read error: {}", ts(), label, e);
                break;
            }
        }
    }
}

/// Forward HQP→T8: frame-level processing with metadata injection.
/// For now, just passthrough — the state machine comes in Task 5.
pub fn forward_hqp_to_naa(
    src: TcpStream,
    dst: TcpStream,
    _shared: SharedMetadata,
) {
    forward_passthrough(src, dst, "HQP->NAA");
}

/// Log XML messages (skip keepalive).
pub fn log_xml(label: &str, data: &[u8]) {
    if data.is_empty() || data[0] != b'<' {
        return;
    }
    if data.windows(9).any(|w| w == b"keepalive") {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        eprintln!("{} [{}] {}", ts(), label, text.trim());
    }
}
