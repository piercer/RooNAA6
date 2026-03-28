use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{info, warn, error, debug};
use crate::types::{Config, FormatDescriptor, TrackMetadata, Naa6Frame, Encoding, DsdRate, naa6_msg_type};

/// Keepalive interval
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Shared state for the NAA6 connection
pub struct Naa6Client {
    config: Config,
    stream: Option<TcpStream>,
    last_keepalive: Instant,
    /// Whether the handshake has been completed
    handshake_done: bool,
    /// Backpressure signal: true when TCP send buffer is full
    pub backpressure: Arc<Mutex<bool>>,
}

impl Naa6Client {
    pub fn new(config: Config) -> Self {
        Naa6Client {
            config,
            stream: None,
            last_keepalive: Instant::now(),
            handshake_done: false,
            backpressure: Arc::new(Mutex::new(false)),
        }
    }

    /// Connect to HQPlayer via TCP
    pub fn connect(&mut self) -> io::Result<()> {
        let addr = format!("{}:{}", self.config.hqplayer_host, self.config.hqplayer_port);
        info!("Connecting to HQPlayer at {}", addr);
        let stream = TcpStream::connect(&addr)?;
        stream.set_nonblocking(false)?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        self.stream = Some(stream);
        self.handshake_done = false;
        info!("NAA6 connection established to {}", addr);
        Ok(())
    }

    /// Perform the NAA 6 handshake, advertising the format descriptor
    pub fn handshake(&mut self, fmt: &FormatDescriptor) -> io::Result<()> {
        let payload = encode_format_descriptor(fmt);
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::HANDSHAKE,
            payload,
        };
        self.write_frame(&frame)?;
        self.handshake_done = true;
        info!("NAA6 handshake sent with format: {:?}", fmt);
        Ok(())
    }

    /// Send raw audio bytes. Must only be called after handshake.
    pub fn send_audio(&mut self, buf: &[u8]) -> io::Result<()> {
        if !self.handshake_done {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Cannot send audio before handshake",
            ));
        }
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::AUDIO,
            payload: buf.to_vec(),
        };
        self.write_frame_with_backpressure(&frame)
    }

    /// Send a format change notification
    pub fn send_format_change(&mut self, fmt: &FormatDescriptor) -> io::Result<()> {
        let payload = encode_format_descriptor(fmt);
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::FORMAT_CHANGE,
            payload,
        };
        debug!("Sending format change: {:?}", fmt);
        self.write_frame(&frame)
    }

    /// Send track metadata
    pub fn send_metadata(&mut self, meta: &TrackMetadata) -> io::Result<()> {
        let payload = encode_metadata(meta);
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::METADATA,
            payload,
        };
        self.write_frame(&frame)
    }

    /// Send a keepalive message if the interval has elapsed
    pub fn tick_keepalive(&mut self) -> io::Result<()> {
        if self.last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
            self.send_keepalive()?;
        }
        Ok(())
    }

    /// Send a keepalive message immediately
    pub fn send_keepalive(&mut self) -> io::Result<()> {
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::KEEPALIVE,
            payload: vec![],
        };
        self.write_frame(&frame)?;
        self.last_keepalive = Instant::now();
        debug!("Keepalive sent");
        Ok(())
    }

    /// Send the NAA 6 session termination message
    pub fn send_termination(&mut self) -> io::Result<()> {
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::TERMINATION,
            payload: vec![],
        };
        info!("Sending NAA6 termination message");
        self.write_frame(&frame)
    }

    /// Close the TCP connection
    pub fn disconnect(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            info!("NAA6 connection closed");
        }
        self.handshake_done = false;
    }

    /// Returns true if currently connected
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Write a frame to the TCP stream, handling backpressure
    fn write_frame_with_backpressure(&mut self, frame: &Naa6Frame) -> io::Result<()> {
        match self.write_frame(frame) {
            Ok(()) => {
                // Clear backpressure on success
                if let Ok(mut bp) = self.backpressure.lock() {
                    *bp = false;
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                warn!("NAA6 send buffer full (EAGAIN/EWOULDBLOCK); signalling backpressure");
                if let Ok(mut bp) = self.backpressure.lock() {
                    *bp = true;
                }
                Err(e)
            }
            Err(e) => {
                error!("NAA6 send error: {} (code: {:?})", e, e.raw_os_error());
                self.stream = None;
                self.handshake_done = false;
                Err(e)
            }
        }
    }

    /// Write a frame to the TCP stream
    fn write_frame(&mut self, frame: &Naa6Frame) -> io::Result<()> {
        let encoded = frame.encode();
        match &mut self.stream {
            Some(stream) => {
                stream.write_all(&encoded).map_err(|e| {
                    error!("NAA6 write error: {} (code: {:?})", e, e.raw_os_error());
                    e
                })
            }
            None => Err(io::Error::new(io::ErrorKind::NotConnected, "Not connected to HQPlayer")),
        }
    }
}

/// Encode a FormatDescriptor into a NAA 6 payload
fn encode_format_descriptor(fmt: &FormatDescriptor) -> Vec<u8> {
    let mut payload = Vec::new();

    // Encoding byte: 0=PCM, 1=DSD_NATIVE, 2=DSD_DOP
    let enc_byte: u8 = match fmt.encoding {
        Encoding::PCM => 0,
        Encoding::DSD_NATIVE => 1,
        Encoding::DSD_DOP => 2,
    };
    payload.push(enc_byte);

    // Sample rate: uint32 LE
    payload.extend_from_slice(&fmt.sample_rate.to_le_bytes());

    // Bit depth
    payload.push(fmt.bit_depth);

    // Channels
    payload.push(fmt.channels);

    // DSD rate (if present): 0=none, 1=DSD64, 2=DSD128, 3=DSD256, 4=DSD512
    let dsd_byte: u8 = match &fmt.dsd_rate {
        None => 0,
        Some(DsdRate::DSD64) => 1,
        Some(DsdRate::DSD128) => 2,
        Some(DsdRate::DSD256) => 3,
        Some(DsdRate::DSD512) => 4,
    };
    payload.push(dsd_byte);

    payload
}

/// Encode TrackMetadata into a NAA 6 metadata payload
fn encode_metadata(meta: &TrackMetadata) -> Vec<u8> {
    let mut payload = Vec::new();

    // Simple TLV-style encoding: for each field, write a tag byte, uint32 LE length, then UTF-8 bytes
    // Tags: 0x01=title, 0x02=artist, 0x03=album, 0x04=coverArt
    fn write_field(payload: &mut Vec<u8>, tag: u8, data: &[u8]) {
        payload.push(tag);
        payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
        payload.extend_from_slice(data);
    }

    if let Some(title) = &meta.title {
        write_field(&mut payload, 0x01, title.as_bytes());
    }
    if let Some(artist) = &meta.artist {
        write_field(&mut payload, 0x02, artist.as_bytes());
    }
    if let Some(album) = &meta.album {
        write_field(&mut payload, 0x03, album.as_bytes());
    }
    if let Some(cover_art) = &meta.cover_art {
        write_field(&mut payload, 0x04, cover_art);
    }

    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Encoding, FormatDescriptor};

    #[test]
    fn test_frame_encoding() {
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::AUDIO,
            payload: vec![0xAA, 0xBB, 0xCC],
        };
        let encoded = frame.encode();
        // type byte
        assert_eq!(encoded[0], naa6_msg_type::AUDIO);
        // length: 3 as uint32 LE
        assert_eq!(&encoded[1..5], &[3u8, 0, 0, 0]);
        // payload
        assert_eq!(&encoded[5..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_format_descriptor_encoding() {
        let fmt = FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: 44100,
            bit_depth: 24,
            channels: 2,
            dsd_rate: None,
        };
        let payload = encode_format_descriptor(&fmt);
        assert_eq!(payload[0], 0); // PCM
        let sr = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
        assert_eq!(sr, 44100);
        assert_eq!(payload[5], 24); // bit depth
        assert_eq!(payload[6], 2);  // channels
        assert_eq!(payload[7], 0);  // no DSD rate
    }

    #[test]
    fn test_metadata_encoding_with_all_fields() {
        let meta = TrackMetadata {
            title: Some("Test Title".to_string()),
            artist: Some("Test Artist".to_string()),
            album: Some("Test Album".to_string()),
            cover_art: None,
        };
        let payload = encode_metadata(&meta);
        // Should contain title tag 0x01
        assert!(payload.contains(&0x01));
        // Should contain artist tag 0x02
        assert!(payload.contains(&0x02));
        // Should contain album tag 0x03
        assert!(payload.contains(&0x03));
        // Should NOT contain cover art tag 0x04
        assert!(!payload.contains(&0x04));
    }

    #[test]
    fn test_metadata_encoding_without_cover_art() {
        let meta = TrackMetadata {
            title: Some("Song".to_string()),
            artist: None,
            album: None,
            cover_art: None,
        };
        let payload = encode_metadata(&meta);
        assert!(payload.contains(&0x01)); // title present
        assert!(!payload.contains(&0x04)); // no cover art
    }

    #[test]
    fn test_keepalive_frame_has_empty_payload() {
        let frame = Naa6Frame {
            frame_type: naa6_msg_type::KEEPALIVE,
            payload: vec![],
        };
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 5); // 1 type + 4 length, no payload
        assert_eq!(&encoded[1..5], &[0u8, 0, 0, 0]);
    }
}
