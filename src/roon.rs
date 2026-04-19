use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tungstenite::{Message, WebSocket};

use crate::config::RoonConfig;
use crate::metadata::{PlayState, SharedMetadata};
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
            let names: Vec<String> = zones
                .iter()
                .filter_map(|z| json_str(z, "display_name").map(|s| s.to_string()))
                .collect();
            if !names.is_empty() {
                let mut all = shared.get_zones();
                for name in &names {
                    if !all.contains(name) {
                        all.push(name.clone());
                    }
                }
                all.sort();
                shared.set_zones(all);
            }

            for zone in zones {
                if json_str(zone, "display_name") != Some(cfg.zone.as_str()) {
                    continue;
                }
                apply_zone_update(
                    shared,
                    zone,
                    &http_agent,
                    &cfg.host,
                    cfg.port,
                    &mut last_image_key,
                );
            }
        }

        apply_zones_seek(shared, &body);
    }
}

/// Apply a full zone object to shared metadata. Does partial updates —
/// each field is only touched if present on the zone body, so other
/// events (zones_seek_changed, previous zones_changed) are never wiped.
pub(crate) fn apply_zone_update(
    shared: &SharedMetadata,
    zone: &Value,
    http_agent: &ureq::Agent,
    host: &str,
    port: u16,
    last_image_key: &mut String,
) {
    let mut meta = shared.get();

    if let Some(np) = zone.get("now_playing") {
        let (title, artist, album) = extract_track_info(np);
        meta.title = title;
        meta.artist = artist;
        meta.album = album;

        if let Some(image_key) = json_str(np, "image_key") {
            if !image_key.is_empty() && image_key != *last_image_key {
                *last_image_key = image_key.to_string();
                meta.cover_art = download_cover(http_agent, host, port, image_key).map(Arc::new);
            }
        }

        if let Some(l) = np
            .get("length")
            .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i.max(0) as u64)))
        {
            meta.length_seconds = Some(l as u32);
        }
    }

    if let Some(state_str) = zone.get("state").and_then(|v| v.as_str()) {
        meta.play_state = match state_str {
            "playing" | "loading" => Some(PlayState::Playing),
            "paused" => Some(PlayState::Paused),
            _ => None, // stopped / unknown
        };
    }

    // seek_position is a zone-level field in the Roon Transport API; check
    // now_playing as a fallback defensively.
    let seek = zone
        .get("seek_position")
        .or_else(|| zone.get("now_playing").and_then(|np| np.get("seek_position")))
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)));
    if let Some(s) = seek {
        meta.seek_position = Some(s);
    }

    if let Some(r) = zone.get("queue_items_remaining").and_then(|v| v.as_u64()) {
        meta.tracks_total = (r as u32) + 1;
    } else if meta.tracks_total == 0 {
        meta.tracks_total = 1;
    }
    if meta.track == 0 {
        meta.track = 1;
    }

    eprintln!(
        "{} [roon] {} \u{2014} {} ({}) len={:?} seek={:?} state={:?}",
        ts(),
        meta.artist,
        meta.title,
        meta.album,
        meta.length_seconds,
        meta.seek_position,
        meta.play_state,
    );

    shared.set(meta);
}

/// Apply a `zones_seek_changed` body: update only `seek_position` on the
/// shared metadata. No-op if the body has no entries or the entry lacks a
/// seek_position field.
pub(crate) fn apply_zones_seek(shared: &SharedMetadata, body: &Value) {
    let Some(seeks) = body.get("zones_seek_changed").and_then(|v| v.as_array()) else {
        return;
    };
    let Some(entry) = seeks.first() else { return };
    let Some(seek) = entry
        .get("seek_position")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
    else {
        return;
    };

    let mut meta = shared.get();
    meta.seek_position = Some(seek);
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

type MooResponse = (String, HashMap<String, String>, Value);

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

fn send_moo(ws: &mut WebSocket<TcpStream>, msg: Vec<u8>) -> Result<(), Box<tungstenite::Error>> {
    ws.send(Message::Binary(msg.into())).map_err(Box::new)
}

fn send_request(
    ws: &mut WebSocket<TcpStream>,
    reqid: &mut u64,
    name: &str,
    body: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rid = *reqid;
    *reqid += 1;
    let content = body.as_ref().map(serde_json::to_vec).transpose()?;
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
    let content = body.map(serde_json::to_vec).transpose()?;
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
) -> Result<MooResponse, Box<dyn std::error::Error>> {
    loop {
        match ws.read()? {
            Message::Binary(d) => return Ok(parse_moo_response(&d)),
            Message::Text(t) => return Ok(parse_moo_response(t.as_bytes())),
            Message::Close(_) => return Err("websocket closed".into()),
            _ => continue,
        }
    }
}

fn parse_moo_response(data: &[u8]) -> MooResponse {
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
