use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;

use crate::discovery::NaaEndpoint;
use crate::metadata::SharedMetadata;
use crate::{config, iptables, ts};

const MAX_BODY: usize = 64 * 1024;

pub struct WebServer {
    pub shared: SharedMetadata,
    pub endpoints: Vec<NaaEndpoint>,
    pub config_path: String,
    shutdown: AtomicBool,
}

impl WebServer {
    pub fn new(shared: SharedMetadata, endpoints: Vec<NaaEndpoint>, config_path: String) -> Self {
        Self {
            shared,
            endpoints,
            config_path,
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn run(&self, port: u16) {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{} [web] bind failed on :{}: {}", ts(), port, e);
                return;
            }
        };
        eprintln!("{} [web] listening on :{}", ts(), port);

        for stream in listener.incoming() {
            if self.shutdown.load(Ordering::Relaxed) {
                eprintln!("{} [web] shutting down", ts());
                break;
            }
            match stream {
                Ok(mut s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    self.handle_request(&mut s, port);
                }
                Err(e) => {
                    eprintln!("{} [web] accept error: {}", ts(), e);
                }
            }
        }

        eprintln!("{} [web] stopped", ts());
    }

    fn handle_request(&self, stream: &mut TcpStream, port: u16) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        // Read the request line.
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let request_line = request_line.trim_end().to_string();

        // Parse method and path.
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();

        // Read headers, capture Content-Length.
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                let val = lower["content-length:".len()..].trim();
                content_length = val.parse().unwrap_or(0);
            }
        }

        // Read POST body if needed.
        if content_length > MAX_BODY {
            self.send_error(stream, "request body too large");
            return;
        }
        let body: Vec<u8> = if content_length > 0 {
            let mut buf = vec![0u8; content_length];
            use std::io::Read;
            reader.read_exact(&mut buf).ok();
            buf
        } else {
            Vec::new()
        };

        eprintln!("{} [web] {} {}", ts(), method, path);

        match (method.as_str(), path.as_str()) {
            ("GET", "/") => self.handle_index(stream),
            ("GET", "/api/status") => self.handle_get_status(stream),
            ("GET", "/api/config") => self.handle_get_config(stream),
            ("GET", "/api/cover") => self.handle_get_cover(stream),
            ("POST", "/api/save") => self.handle_post_save(stream, &body),
            ("POST", "/api/iptables") => self.handle_post_iptables(stream, &body),
            ("POST", "/api/restart") => self.handle_post_restart(stream),
            ("POST", "/api/shutdown") => self.handle_post_shutdown(stream, port),
            _ => {
                self.send_response(stream, 404, "application/json", b"{\"error\":\"not found\"}");
            }
        }
    }

    fn handle_index(&self, stream: &mut TcpStream) {
        let html = include_str!("web_page.html");
        self.send_response(stream, 200, "text/html; charset=utf-8", html.as_bytes());
    }

    fn handle_get_status(&self, stream: &mut TcpStream) {
        let meta = self.shared.get();
        let playing = match meta.play_state {
            Some(crate::metadata::PlayState::Playing) => true,
            Some(crate::metadata::PlayState::Paused) => true,
            _ => false,
        };
        let val = json!({
            "title": if playing { &meta.title } else { "" },
            "artist": if playing { &meta.artist } else { "" },
            "album": if playing { &meta.album } else { "" },
            "has_cover": playing && meta.cover_art.is_some(),
            "playing": playing,
        });
        self.send_json(stream, &val);
    }

    fn handle_get_config(&self, stream: &mut TcpStream) {
        let cfg = match config::load_from(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                self.send_error(stream, &format!("failed to load config: {}", e));
                return;
            }
        };

        let naa_host_for_check = cfg
            .iptables
            .as_ref()
            .map(|i| i.naa_host.clone())
            .or_else(|| cfg.naa.host.clone())
            .unwrap_or_default();

        let active = if naa_host_for_check.is_empty() {
            false
        } else {
            iptables::check_rule(&naa_host_for_check)
        };

        let endpoints_val: Vec<serde_json::Value> = self
            .endpoints
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "ip": e.addr.ip().to_string(),
                    "version": e.version,
                })
            })
            .collect();

        let zones = self.shared.get_zones();

        let val = json!({
            "naa": {
                "host": cfg.naa.host,
                "target": cfg.naa.target,
                "mcast_iface": cfg.naa.mcast_iface.to_string(),
            },
            "roon": {
                "host": cfg.roon.host,
                "port": cfg.roon.port,
                "zone": cfg.roon.zone,
            },
            "iptables": {
                "enable": cfg.iptables.as_ref().map(|i| i.enable).unwrap_or(false),
                "naa_host": cfg.iptables.as_ref().map(|i| i.naa_host.as_str()).unwrap_or(""),
                "active": active,
            },
            "endpoints": endpoints_val,
            "zones": zones,
        });
        self.send_json(stream, &val);
    }

    fn handle_get_cover(&self, stream: &mut TcpStream) {
        let meta = self.shared.get();
        match meta.cover_art {
            Some(arc) => {
                self.send_response(stream, 200, "image/jpeg", &arc);
            }
            None => {
                self.send_response(stream, 404, "application/json", b"{\"error\":\"no cover art\"}");
            }
        }
    }

    fn handle_post_save(&self, stream: &mut TcpStream, body: &[u8]) {
        let parsed: serde_json::Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                self.send_error(stream, &format!("invalid JSON: {}", e));
                return;
            }
        };

        let mut cfg = match config::load_from(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                self.send_error(stream, &format!("failed to load config: {}", e));
                return;
            }
        };

        // Merge naa fields. Treat empty strings as None to avoid breaking resolve_target.
        if let Some(naa) = parsed.get("naa") {
            if let Some(v) = naa.get("host").and_then(|v| v.as_str()) {
                cfg.naa.host = if v.is_empty() { None } else { Some(v.to_string()) };
            }
            if let Some(v) = naa.get("target").and_then(|v| v.as_str()) {
                cfg.naa.target = if v.is_empty() { None } else { Some(v.to_string()) };
            }
        }

        // Merge roon fields.
        if let Some(roon) = parsed.get("roon") {
            if let Some(v) = roon.get("host").and_then(|v| v.as_str()) {
                cfg.roon.host = v.to_string();
            }
            if let Some(v) = roon.get("port").and_then(|v| v.as_u64()) {
                cfg.roon.port = v as u16;
            }
            if let Some(v) = roon.get("zone").and_then(|v| v.as_str()) {
                cfg.roon.zone = v.to_string();
            }
        }

        if let Err(e) = config::save(&self.config_path, &cfg) {
            self.send_error(stream, &e);
            return;
        }

        self.send_json(stream, &json!({"ok": true}));
    }

    fn handle_post_iptables(&self, stream: &mut TcpStream, body: &[u8]) {
        let parsed: serde_json::Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                self.send_error(stream, &format!("invalid JSON: {}", e));
                return;
            }
        };

        let enable = parsed.get("enable").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut cfg = match config::load_from(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                self.send_error(stream, &format!("failed to load config: {}", e));
                return;
            }
        };

        // Determine naa_host: iptables section first, fall back to naa.host.
        let naa_host = cfg
            .iptables
            .as_ref()
            .map(|i| i.naa_host.clone())
            .or_else(|| cfg.naa.host.clone())
            .unwrap_or_default();

        if naa_host.is_empty() {
            self.send_error(stream, "no naa_host configured");
            return;
        }

        if enable {
            if let Err(e) = iptables::add_rule(&naa_host) {
                self.send_error(stream, &format!("iptables add failed: {}", e));
                return;
            }
        } else if let Err(e) = iptables::remove_rule(&naa_host) {
            self.send_error(stream, &format!("iptables remove failed: {}", e));
            return;
        }

        // Persist enable flag to config.
        if let Some(ref mut ipt) = cfg.iptables {
            ipt.enable = enable;
        } else {
            cfg.iptables = Some(config::IptablesConfig {
                enable,
                naa_host: naa_host.clone(),
            });
        }

        if let Err(e) = config::save(&self.config_path, &cfg) {
            self.send_error(stream, &e);
            return;
        }

        let active = iptables::check_rule(&naa_host);
        self.send_json(stream, &json!({"ok": true, "active": active}));
    }

    fn handle_post_restart(&self, stream: &mut TcpStream) {
        self.send_json(stream, &json!({"ok": true}));
        // Flush / close the stream before spawning systemctl.
        stream.flush().ok();
        let _ = Command::new("systemctl")
            .args(["restart", "roonaa6"])
            .spawn();
    }

    fn handle_post_shutdown(&self, stream: &mut TcpStream, port: u16) {
        let cfg = match config::load_from(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                self.send_error(stream, &format!("failed to load config: {}", e));
                return;
            }
        };

        let mut cfg = cfg;
        if let Some(ref mut web) = cfg.web {
            web.enable = false;
        }

        if let Err(e) = config::save(&self.config_path, &cfg) {
            self.send_error(stream, &e);
            return;
        }

        self.send_json(stream, &json!({"ok": true}));
        stream.flush().ok();

        self.shutdown.store(true, Ordering::Relaxed);
        // Poke the listener to unblock accept().
        let _ = TcpStream::connect(("127.0.0.1", port));
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn send_response(&self, stream: &mut TcpStream, status_code: u16, content_type: &str, body_bytes: &[u8]) {
        let status_text = match status_code {
            200 => "OK",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Unknown",
        };
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            status_code,
            status_text,
            content_type,
            body_bytes.len(),
        );
        stream.write_all(header.as_bytes()).ok();
        stream.write_all(body_bytes).ok();
        stream.flush().ok();
    }

    fn send_json(&self, stream: &mut TcpStream, val: &serde_json::Value) {
        let bytes = serde_json::to_vec(val).unwrap_or_else(|_| b"{}".to_vec());
        self.send_response(stream, 200, "application/json", &bytes);
    }

    fn send_error(&self, stream: &mut TcpStream, msg: &str) {
        let val = json!({"error": msg});
        let bytes = serde_json::to_vec(&val).unwrap_or_else(|_| b"{}".to_vec());
        self.send_response(stream, 500, "application/json", &bytes);
    }
}
