use std::fs;
use std::path::Path;
use serde_json::Value;
use log::{warn, error};
use crate::types::Config;

const SYSTEM_CONFIG_PATH: &str = "/etc/roon-naa6-bridge/config.json";
const LOCAL_CONFIG_PATH: &str = "./config.json";

/// Known configuration keys
const KNOWN_KEYS: &[&str] = &[
    "outputName",
    "alsaDevice",
    "hqplayerHost",
    "hqplayerPort",
    "reconnectBackoff",
    "logLevel",
    "ipcSocket",
];

/// Load and validate configuration from the well-known locations.
/// Falls back to defaults if no config file is found.
/// Exits with code 1 on invalid required parameter values.
pub fn load_config() -> Config {
    let raw = read_config_file();
    match raw {
        Some((path, content)) => parse_and_validate(&path, &content),
        None => {
            warn!("No config file found at {} or {}; using defaults", SYSTEM_CONFIG_PATH, LOCAL_CONFIG_PATH);
            Config::default()
        }
    }
}

fn read_config_file() -> Option<(String, String)> {
    if Path::new(SYSTEM_CONFIG_PATH).exists() {
        match fs::read_to_string(SYSTEM_CONFIG_PATH) {
            Ok(content) => return Some((SYSTEM_CONFIG_PATH.to_string(), content)),
            Err(e) => warn!("Failed to read {}: {}", SYSTEM_CONFIG_PATH, e),
        }
    }
    if Path::new(LOCAL_CONFIG_PATH).exists() {
        match fs::read_to_string(LOCAL_CONFIG_PATH) {
            Ok(content) => return Some((LOCAL_CONFIG_PATH.to_string(), content)),
            Err(e) => warn!("Failed to read {}: {}", LOCAL_CONFIG_PATH, e),
        }
    }
    None
}

fn parse_and_validate(path: &str, content: &str) -> Config {
    // Parse as generic JSON first to detect unknown keys
    let json: Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to parse config file {}: {}", path, e);
            std::process::exit(1);
        }
    };

    // Warn on unknown keys
    if let Value::Object(map) = &json {
        for key in map.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                warn!("Unknown config key '{}' in {}; ignoring", key, path);
            }
        }
    }

    // Deserialize into Config struct (applies defaults for missing fields)
    let config: Config = match serde_json::from_value(json) {
        Ok(c) => c,
        Err(e) => {
            error!("Invalid config in {}: {}", path, e);
            std::process::exit(1);
        }
    };

    validate_config(&config);
    config
}

fn validate_config(config: &Config) {
    // hqplayerPort is already constrained to u16 (1–65535) by the type system,
    // but port 0 is invalid.
    if config.hqplayer_port == 0 {
        error!(
            "Invalid config value: hqplayerPort={} is out of range [1, 65535]",
            config.hqplayer_port
        );
        std::process::exit(1);
    }

    if config.reconnect_backoff == 0 {
        error!(
            "Invalid config value: reconnectBackoff={} must be greater than 0",
            config.reconnect_backoff
        );
        std::process::exit(1);
    }

    let valid_log_levels = ["info", "debug"];
    if !valid_log_levels.contains(&config.log_level.as_str()) {
        error!(
            "Invalid config value: logLevel='{}' must be one of {:?}",
            config.log_level, valid_log_levels
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn parse_config_from_str(content: &str) -> Config {
        let json: Value = serde_json::from_str(content).expect("valid JSON");
        if let Value::Object(map) = &json {
            for key in map.keys() {
                if !KNOWN_KEYS.contains(&key.as_str()) {
                    warn!("Unknown config key '{}'; ignoring", key);
                }
            }
        }
        let config: Config = serde_json::from_value(json).expect("valid config");
        config
    }

    #[test]
    fn test_defaults_when_no_file() {
        // When no config file is present, defaults should be used
        let config = Config::default();
        assert_eq!(config.output_name, "HQPlayer via NAA6");
        assert_eq!(config.alsa_device, "hw:Loopback,1,0");
        assert_eq!(config.hqplayer_host, "127.0.0.1");
        assert_eq!(config.hqplayer_port, 10700);
        assert_eq!(config.reconnect_backoff, 5000);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.ipc_socket, "/run/roon-naa6-bridge/meta.sock");
    }

    #[test]
    fn test_partial_config_uses_defaults_for_missing_fields() {
        let content = r#"{"outputName": "My Bridge"}"#;
        let config = parse_config_from_str(content);
        assert_eq!(config.output_name, "My Bridge");
        assert_eq!(config.hqplayer_port, 10700);
        assert_eq!(config.reconnect_backoff, 5000);
    }

    #[test]
    fn test_valid_config_parsed_correctly() {
        let content = r#"{
            "outputName": "Test Output",
            "alsaDevice": "hw:Loopback,1,0",
            "hqplayerHost": "192.168.1.100",
            "hqplayerPort": 10700,
            "reconnectBackoff": 3000,
            "logLevel": "debug"
        }"#;
        let config = parse_config_from_str(content);
        assert_eq!(config.output_name, "Test Output");
        assert_eq!(config.hqplayer_host, "192.168.1.100");
        assert_eq!(config.hqplayer_port, 10700);
        assert_eq!(config.reconnect_backoff, 3000);
        assert_eq!(config.log_level, "debug");
    }

    #[test]
    fn test_unknown_keys_do_not_prevent_parsing() {
        // Unknown keys should be warned about but not cause failure
        let content = r#"{"outputName": "Test", "unknownKey": "value", "anotherUnknown": 42}"#;
        let config = parse_config_from_str(content);
        assert_eq!(config.output_name, "Test");
    }
}
