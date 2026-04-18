use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct Config {
    pub naa: NaaConfig,
    pub roon: RoonConfig,
    pub web: Option<WebConfig>,
    pub iptables: Option<IptablesConfig>,
}

#[derive(Deserialize, Serialize)]
pub struct NaaConfig {
    /// IP of the real NAA endpoint. Optional if `target` is set (auto-discovered).
    pub host: Option<String>,
    pub mcast_iface: Ipv4Addr,
    /// NAA version string override. Auto-discovered from the target endpoint if omitted.
    pub version: Option<String>,
    /// NAA endpoint name to proxy (matched against discover responses).
    pub target: Option<String>,
    /// HQPlayer host IP. Only needed when proxy runs on a different machine than HQP.
    /// Enables the Status proxy to listen on 4321 directly (no iptables needed).
    pub hqp_host: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct RoonConfig {
    pub host: String,
    #[serde(default = "default_roon_port")]
    pub port: u16,
    pub zone: String,
    #[serde(default = "default_token_file")]
    pub token_file: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct WebConfig {
    pub enable: bool,
    #[serde(default = "default_web_port")]
    pub port: u16,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct IptablesConfig {
    pub enable: bool,
    pub naa_host: String,
}

fn default_roon_port() -> u16 {
    9330
}

fn default_token_file() -> String {
    "/etc/roonaa6/roon_token.json".to_string()
}

fn default_web_port() -> u16 {
    8080
}

pub fn config_path() -> String {
    std::env::args().nth(1).unwrap_or_else(|| "config.toml".to_string())
}

pub fn load() -> Config {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read config file '{}': {}", path, e);
        std::process::exit(1);
    });
    toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("Failed to parse config file '{}': {}", path, e);
        std::process::exit(1);
    })
}

/// Load config from an explicit path, returning an error string on failure.
pub fn load_from(path: &str) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config file '{}': {}", path, e))?;
    toml::from_str(&text)
        .map_err(|e| format!("Failed to parse config file '{}': {}", path, e))
}

pub fn save(path: &str, cfg: &Config) -> Result<(), String> {
    let bak = format!("{}.bak", path);
    if !std::path::Path::new(&bak).exists() && std::path::Path::new(path).exists() {
        let _ = std::fs::copy(path, &bak);
    }
    let text = toml::to_string(cfg).map_err(|e| format!("Failed to serialise config: {}", e))?;
    std::fs::write(path, text).map_err(|e| format!("Failed to write config file '{}': {}", path, e))
}
