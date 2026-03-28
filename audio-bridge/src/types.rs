use serde::{Deserialize, Serialize};

/// Audio format encoding type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    PCM,
    DSD_NATIVE,
    DSD_DOP,
}

/// DSD rate multiplier
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DsdRate {
    DSD64,
    DSD128,
    DSD256,
    DSD512,
}

/// Describes the audio format of the current stream
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatDescriptor {
    pub encoding: Encoding,
    /// Hz for PCM; DSD clock rate for DSD
    pub sample_rate: u32,
    /// Bits per sample (PCM: 16/24/32; DSD: 1)
    pub bit_depth: u8,
    /// Number of channels (1–8)
    pub channels: u8,
    /// DSD rate multiplier (DSD only)
    pub dsd_rate: Option<DsdRate>,
}

/// Track metadata from Roon
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Raw image bytes, max 1024×1024
    pub cover_art: Option<Vec<u8>>,
}

/// NAA 6 wire frame (internal representation)
#[derive(Debug, Clone)]
pub struct Naa6Frame {
    /// Message type byte per NAA 6 spec
    pub frame_type: u8,
    /// Payload bytes
    pub payload: Vec<u8>,
}

impl Naa6Frame {
    /// Encode the frame to bytes: 1-byte type + uint32 LE length + payload
    pub fn encode(&self) -> Vec<u8> {
        let len = self.payload.len() as u32;
        let mut buf = Vec::with_capacity(5 + self.payload.len());
        buf.push(self.frame_type);
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }
}

/// Bridge configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Roon Output display name
    #[serde(default = "Config::default_output_name")]
    pub output_name: String,
    /// ALSA loopback capture device
    #[serde(default = "Config::default_alsa_device")]
    pub alsa_device: String,
    /// HQPlayer hostname or IP
    #[serde(default = "Config::default_hqplayer_host")]
    pub hqplayer_host: String,
    /// NAA 6 TCP port
    #[serde(default = "Config::default_hqplayer_port")]
    pub hqplayer_port: u16,
    /// Reconnect interval in ms
    #[serde(default = "Config::default_reconnect_backoff")]
    pub reconnect_backoff: u64,
    /// Log verbosity level
    #[serde(default = "Config::default_log_level")]
    pub log_level: String,
    /// Unix domain socket path for metadata IPC
    #[serde(default = "Config::default_ipc_socket")]
    pub ipc_socket: String,
}

impl Config {
    pub fn default_output_name() -> String { "HQPlayer via NAA6".to_string() }
    pub fn default_alsa_device() -> String { "hw:Loopback,1,0".to_string() }
    pub fn default_hqplayer_host() -> String { "127.0.0.1".to_string() }
    pub fn default_hqplayer_port() -> u16 { 10700 }
    pub fn default_reconnect_backoff() -> u64 { 5000 }
    pub fn default_log_level() -> String { "info".to_string() }
    pub fn default_ipc_socket() -> String { "/run/roon-naa6-bridge/meta.sock".to_string() }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_name: Config::default_output_name(),
            alsa_device: Config::default_alsa_device(),
            hqplayer_host: Config::default_hqplayer_host(),
            hqplayer_port: Config::default_hqplayer_port(),
            reconnect_backoff: Config::default_reconnect_backoff(),
            log_level: Config::default_log_level(),
            ipc_socket: Config::default_ipc_socket(),
        }
    }
}

/// NAA 6 message type constants
pub mod naa6_msg_type {
    pub const HANDSHAKE: u8 = 0x01;
    pub const AUDIO: u8 = 0x02;
    pub const FORMAT_CHANGE: u8 = 0x03;
    pub const METADATA: u8 = 0x04;
    pub const KEEPALIVE: u8 = 0x05;
    pub const TERMINATION: u8 = 0x06;
}

/// Valid PCM sample rates
pub const VALID_PCM_SAMPLE_RATES: &[u32] = &[
    44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000,
];

/// Valid PCM bit depths
pub const VALID_BIT_DEPTHS: &[u8] = &[16, 24, 32];
