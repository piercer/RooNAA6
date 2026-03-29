use serde::{Deserialize, Serialize};

/// Describes the audio format negotiated in a Start message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatDescriptor {
    pub bits: u8,
    pub channels: u8,
    pub rate: u32,
    /// Echoed verbatim from HQPlayer's start message (e.g. "1.5000000000000000")
    pub netbuftime: String,
    /// Stream type, e.g. "pcm"
    pub stream: String,
}

/// Track metadata from Roon (via IPC socket)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
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
    /// Control server listen port (Roon connects here)
    #[serde(default = "Config::default_listen_port")]
    pub listen_port: u16,
    /// HQPlayer host IP
    #[serde(default = "Config::default_hqplayer_host")]
    pub hqplayer_host: String,
    /// HQPlayer control port
    #[serde(default = "Config::default_hqplayer_port")]
    pub hqplayer_port: u16,
    /// HTTP audio server port (HQPlayer fetches PCM here)
    #[serde(default = "Config::default_http_port")]
    pub http_port: u16,
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
    pub fn default_listen_port() -> u16 { 4321 }
    pub fn default_hqplayer_host() -> String { "192.168.30.212".to_string() }
    pub fn default_hqplayer_port() -> u16 { 4321 }
    pub fn default_http_port() -> u16 { 30001 }
    pub fn default_reconnect_backoff() -> u64 { 5000 }
    pub fn default_log_level() -> String { "info".to_string() }
    pub fn default_ipc_socket() -> String { "/run/roon-naa6-bridge/meta.sock".to_string() }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_name: Config::default_output_name(),
            alsa_device: Config::default_alsa_device(),
            listen_port: Config::default_listen_port(),
            hqplayer_host: Config::default_hqplayer_host(),
            hqplayer_port: Config::default_hqplayer_port(),
            http_port: Config::default_http_port(),
            reconnect_backoff: Config::default_reconnect_backoff(),
            log_level: Config::default_log_level(),
            ipc_socket: Config::default_ipc_socket(),
        }
    }
}
