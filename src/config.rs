use std::net::Ipv4Addr;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub naa: NaaConfig,
    pub roon: RoonConfig,
}

#[derive(Deserialize)]
pub struct NaaConfig {
    /// IP of the real NAA endpoint. Optional if `target` is set (auto-discovered).
    pub host: Option<String>,
    pub mcast_iface: Ipv4Addr,
    /// NAA version string override. Auto-discovered from the target endpoint if omitted.
    pub version: Option<String>,
    /// NAA endpoint name to proxy (matched against discover responses).
    pub target: Option<String>,
}

#[derive(Deserialize)]
pub struct RoonConfig {
    pub host: String,
    #[serde(default = "default_roon_port")]
    pub port: u16,
    pub zone: String,
    #[serde(default = "default_token_file")]
    pub token_file: String,
}

fn default_roon_port() -> u16 {
    9330
}

fn default_token_file() -> String {
    "/etc/roonaa6/roon_token.json".to_string()
}

pub fn load() -> Config {
    let path = std::env::args().nth(1).unwrap_or_else(|| "config.toml".to_string());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read config file '{}': {}", path, e);
        std::process::exit(1);
    });
    toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("Failed to parse config file '{}': {}", path, e);
        std::process::exit(1);
    })
}
