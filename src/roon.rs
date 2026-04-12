use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tungstenite::{Message, WebSocket};

use crate::config::RoonConfig;
use crate::metadata::{Metadata, SharedMetadata};
use crate::ts;

pub fn run(shared: SharedMetadata, cfg: &RoonConfig) {
    loop {
        if let Err(e) = run_once(&shared, cfg) {
            eprintln!("{} [roon] connection error: {}", ts(), e);
        }
        eprintln!("{} [roon] reconnecting in 5s...", ts());
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn run_once(
    shared: &SharedMetadata,
    cfg: &RoonConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("ws://{}:{}/api", cfg.host, cfg.port);
    let tcp = TcpStream::connect((&*cfg.host, cfg.port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
    let (mut ws, _) = tungstenite::client::client_with_config(&url, tcp, None)?;

    eprintln!("{} [roon] connected to Roon Core", ts());

    let http_agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    );

    let mut reqid: u64 = 0;
    let mut last_image_key = String::new();

    // Get core info
    send_request(&mut ws, &mut reqid, "com.roonlabs.registry:1/info", None)?;
    let (_first, _headers, body) = recv_response(&mut ws)?;
    let core_id = json_str(&body, "core_id").unwrap_or("").to_string();
    if core_id.is_empty() {
        eprintln!("{} [roon] warning: no core_id in info response", ts());
    }
    let display = json_str(&body, "display_name").unwrap_or("?");
    let version = json_str(&body, "display_version").unwrap_or("?");
    eprintln!("{} [roon] core: {} v{}", ts(), display, version);

    // Register extension
    let token = load_token(&cfg.token_file);
    let mut reg = serde_json::json!({
        "extension_id": "com.roonaa6.metadata",
        "display_name": "RooNAA6 Metadata",
        "display_version": "1.0.0",
        "publisher": "RooNAA6",
        "email": "noreply@example.com",
        "provided_services": ["com.roonlabs.pairing:1", "com.roonlabs.ping:1"],
        "required_services": ["com.roonlabs.transport:2"],
        "optional_services": [],
        "website": ""
    });
    if let Some(t) = &token {
        reg["token"] = Value::String(t.clone());
    }
    send_request(
        &mut ws,
        &mut reqid,
        "com.roonlabs.registry:1/register",
        Some(reg),
    )?;
    eprintln!("{} [roon] registration sent", ts());

    // Main event loop
    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    eprintln!("{} [roon] read timeout, still connected", ts());
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        let data = match msg {
            Message::Binary(d) => d.to_vec(),
            Message::Text(t) => t.as_bytes().to_vec(),
            Message::Close(_) => return Ok(()),
            _ => continue,
        };

        let (first_line, headers, body) = parse_moo_response(&data);

        // Handle incoming REQUESTs (ping, pairing)
        if first_line.starts_with("MOO/1 REQUEST") {
            if let Some(rid) = headers.get("Request-Id") {
                if first_line.contains("subscribe_pairing") {
                    let body = serde_json::json!({"paired_core_id": core_id});
                    send_response(&mut ws, "CONTINUE Changed", rid, Some(&body))?;
                } else {
                    send_response(&mut ws, "COMPLETE Success", rid, None)?;
                }
            }
            continue;
        }

        // Handle token (pairing response)
        if let Some(t) = body.get("token").and_then(|v| v.as_str()) {
            save_token(&cfg.token_file, t);
            eprintln!("{} [roon] paired!", ts());
            send_request(
                &mut ws,
                &mut reqid,
                "com.roonlabs.transport:2/subscribe_zones",
                Some(serde_json::json!({"subscription_key": "zones"})),
            )?;
            continue;
        }

        // Handle zone updates
        let zones = body
            .get("zones")
            .or_else(|| body.get("zones_changed"))
            .or_else(|| body.get("zones_added"))
            .and_then(|v| v.as_array());

        if let Some(zones) = zones {
            for zone in zones {
                if json_str(zone, "display_name") != Some(cfg.zone.as_str()) {
                    continue;
                }
                if let Some(np) = zone.get("now_playing") {
                    let (title, artist, album) = extract_track_info(np);
                    let image_key = json_str(np, "image_key").unwrap_or("").to_string();

                    let mut cover_art = shared.get().cover_art;
                    if !image_key.is_empty() && image_key != last_image_key {
                        last_image_key = image_key.clone();
                        cover_art =
                            download_cover(&http_agent, &cfg.host, cfg.port, &image_key)
                                .map(Arc::new);
                    }

                    eprintln!(
                        "{} [roon] {} \u{2014} {} ({})",
                        ts(), artist, title, album,
                    );

                    shared.set(Metadata {
                        title,
                        artist,
                        album,
                        cover_art,
                        position: extract_playback_position(zone, std::time::Instant::now()),
                    });
                }
            }
        }

        apply_zones_seek(shared, &body);
    }
}

// --- Track info extraction ---

/// Parse the `state`, `seek_position`, and `now_playing.length` fields from a
/// zone object into a PlaybackPosition. Returns None if any required field is
/// missing or if state is "stopped".
pub(crate) fn extract_playback_position(
    zone: &Value,
    captured_at: std::time::Instant,
) -> Option<crate::metadata::PlaybackPosition> {
    use crate::metadata::{PlayState, PlaybackPosition};

    let state_str = zone.get("state").and_then(|v| v.as_str())?;
    let state = match state_str {
        "playing" | "loading" => PlayState::Playing,
        "paused" => PlayState::Paused,
        _ => return None, // "stopped" and anything unknown
    };

    let seek_position = zone
        .get("seek_position")
        .and_then(|v| v.as_f64())
        .or_else(|| zone.get("seek_position").and_then(|v| v.as_i64()).map(|i| i as f64))?;

    let np = zone.get("now_playing")?;
    let length = np
        .get("length")
        .and_then(|v| v.as_u64())
        .or_else(|| np.get("length").and_then(|v| v.as_i64()).map(|i| i.max(0) as u64))?
        as u32;

    let tracks_total = zone
        .get("queue_items_remaining")
        .and_then(|v| v.as_u64())
        .map(|r| (r as u32) + 1)
        .unwrap_or(1);
    let track = 1;

    Some(PlaybackPosition {
        length_seconds: length,
        position_seconds: seek_position,
        captured_at,
        state,
        track,
        tracks_total,
    })
}

/// Apply a `zones_seek_changed` body to the shared metadata. Updates only
/// position_seconds and captured_at on an existing PlaybackPosition —
/// title/artist/album and length are untouched. No-op if there's no prior
/// PlaybackPosition (we need length to build one).
pub(crate) fn apply_zones_seek(shared: &SharedMetadata, body: &Value) {
    let seeks = match body.get("zones_seek_changed").and_then(|v| v.as_array()) {
        Some(s) => s,
        None => return,
    };
    if seeks.is_empty() {
        return;
    }

    let mut meta = shared.get();
    let Some(mut pos) = meta.position else { return };

    let entry = &seeks[0];
    let Some(seek) = entry
        .get("seek_position")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
    else {
        return;
    };
    pos.position_seconds = seek;
    pos.captured_at = std::time::Instant::now();
    meta.position = Some(pos);
    shared.set(meta);
}

fn extract_track_info(np: &Value) -> (String, String, String) {
    let three = np.get("three_line").unwrap_or(&Value::Null);
    let two = np.get("two_line").unwrap_or(&Value::Null);
    let one = np.get("one_line").unwrap_or(&Value::Null);

    let title = [three, two, one]
        .iter()
        .find_map(|v| json_str(v, "line1"))
        .unwrap_or("")
        .to_string();
    let artist = [three, two]
        .iter()
        .find_map(|v| json_str(v, "line2"))
        .unwrap_or("")
        .to_string();
    let album = json_str(three, "line3").unwrap_or("").to_string();

    (title, artist, album)
}

fn json_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|v| v.as_str())
}

// --- MOO protocol helpers ---

fn build_moo_message(first_line: &str, request_id: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut msg = format!("{}\nRequest-Id: {}\n", first_line, request_id);
    if let Some(content) = body {
        use std::fmt::Write;
        write!(
            msg,
            "Content-Length: {}\nContent-Type: application/json\n",
            content.len()
        )
        .unwrap();
    }
    msg.push('\n');
    let mut bytes = msg.into_bytes();
    if let Some(content) = body {
        bytes.extend_from_slice(content);
    }
    bytes
}

fn send_moo(ws: &mut WebSocket<TcpStream>, msg: Vec<u8>) -> Result<(), tungstenite::Error> {
    ws.send(Message::Binary(msg.into()))
}

fn send_request(
    ws: &mut WebSocket<TcpStream>,
    reqid: &mut u64,
    name: &str,
    body: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rid = *reqid;
    *reqid += 1;
    let content = body.map(|b| serde_json::to_vec(&b)).transpose()?;
    let msg = build_moo_message(
        &format!("MOO/1 REQUEST {}", name),
        &rid.to_string(),
        content.as_deref(),
    );
    send_moo(ws, msg)?;
    Ok(())
}

fn send_response(
    ws: &mut WebSocket<TcpStream>,
    status: &str,
    request_id: &str,
    body: Option<&Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let content = body.map(|b| serde_json::to_vec(b)).transpose()?;
    let msg = build_moo_message(
        &format!("MOO/1 {}", status),
        request_id,
        content.as_deref(),
    );
    send_moo(ws, msg)?;
    Ok(())
}

fn recv_response(
    ws: &mut WebSocket<TcpStream>,
) -> Result<(String, HashMap<String, String>, Value), Box<dyn std::error::Error>> {
    loop {
        match ws.read()? {
            Message::Binary(d) => return Ok(parse_moo_response(&d)),
            Message::Text(t) => return Ok(parse_moo_response(t.as_bytes())),
            Message::Close(_) => return Err("websocket closed".into()),
            _ => continue,
        }
    }
}

fn parse_moo_response(data: &[u8]) -> (String, HashMap<String, String>, Value) {
    // Search raw bytes for header/body separator — avoids UTF-8 converting the entire message
    let sep = match data.windows(2).position(|w| w == b"\n\n") {
        Some(s) => s,
        None => return (String::new(), HashMap::new(), Value::Null),
    };

    let header_bytes = &data[..sep];
    let body_bytes = &data[sep + 2..];

    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.lines();
    let first_line = lines.next().unwrap_or("").to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_bytes).unwrap_or(Value::Null)
    };

    (first_line, headers, body)
}

// --- Token persistence ---

fn load_token(token_file: &str) -> Option<String> {
    let data = std::fs::read_to_string(token_file).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    v.get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

fn save_token(token_file: &str, token: &str) {
    let v = serde_json::json!({"token": token});
    if let Err(e) = std::fs::write(token_file, serde_json::to_string(&v).unwrap()) {
        eprintln!("{} [roon] failed to save token: {}", ts(), e);
    }
}

// --- Cover art ---

fn download_cover(
    agent: &ureq::Agent,
    roon_host: &str,
    roon_port: u16,
    image_key: &str,
) -> Option<Vec<u8>> {
    let url = format!(
        "http://{}:{}/api/image/{}?scale=fit&width=250&height=250&format=image/jpeg",
        roon_host, roon_port, image_key,
    );
    match agent.get(&url).call() {
        Ok(resp) => match resp.into_body().read_to_vec() {
            Ok(data) if data.len() > 100 && data.starts_with(&[0xFF, 0xD8]) => {
                eprintln!("{} [roon] cover art: {}b", ts(), data.len());
                Some(data)
            }
            _ => None,
        },
        Err(e) => {
            eprintln!("{} [roon] cover download failed: {}", ts(), e);
            None
        }
    }
}
