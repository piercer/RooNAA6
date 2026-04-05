use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;
use tungstenite::{Message, WebSocket};

use crate::metadata::{Metadata, SharedMetadata};
use crate::ts;

pub fn run(shared: SharedMetadata, roon_host: &str, roon_port: u16, zone_name: &str, token_file: &str) {
    loop {
        match run_once(&shared, roon_host, roon_port, zone_name, token_file) {
            Ok(()) => {}
            Err(e) => eprintln!("{} [roon] connection error: {}", ts(), e),
        }
        eprintln!("{} [roon] reconnecting in 5s...", ts());
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn run_once(shared: &SharedMetadata, roon_host: &str, roon_port: u16, zone_name: &str, token_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("ws://{}:{}/api", roon_host, roon_port);
    let tcp = TcpStream::connect((roon_host, roon_port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
    let (mut ws, _) = tungstenite::client::client_with_config(
        &url,
        tcp,
        None,
    )?;

    eprintln!("{} [roon] connected to Roon Core", ts());

    let mut reqid: u64 = 0;
    let mut core_id = String::new();
    let mut last_image_key = String::new();

    // Get core info
    send_request(&mut ws, &mut reqid, "com.roonlabs.registry:1/info", None)?;
    let (_first, _headers, body) = recv_response(&mut ws)?;
    if let Some(id) = body.get("core_id").and_then(|v| v.as_str()) {
        core_id = id.to_string();
    }
    if core_id.is_empty() {
        eprintln!("{} [roon] warning: no core_id in info response", ts());
    }
    let display = body.get("display_name").and_then(|v| v.as_str()).unwrap_or("?");
    let version = body.get("display_version").and_then(|v| v.as_str()).unwrap_or("?");
    eprintln!("{} [roon] core: {} v{}", ts(), display, version);

    // Register extension
    let token = load_token(token_file);
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

        let data: Vec<u8> = match msg {
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
                    send_continue_response(&mut ws, rid, &body)?;
                } else {
                    send_complete_response(&mut ws, rid)?;
                }
            }
            continue;
        }

        // Handle token (pairing response)
        if let Some(t) = body.get("token").and_then(|v| v.as_str()) {
            save_token(token_file, t);
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
            .and_then(|v| v.as_array());

        if let Some(zones) = zones {
            for zone in zones {
                let zn = zone
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if zn != zone_name {
                    continue;
                }
                if let Some(np) = zone.get("now_playing") {
                    let three = np.get("three_line").unwrap_or(&Value::Null);
                    let two = np.get("two_line").unwrap_or(&Value::Null);
                    let one = np.get("one_line").unwrap_or(&Value::Null);

                    let title = three
                        .get("line1")
                        .or_else(|| two.get("line1"))
                        .or_else(|| one.get("line1"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let artist = three
                        .get("line2")
                        .or_else(|| two.get("line2"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let album = three
                        .get("line3")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let image_key = np
                        .get("image_key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Download cover art if image_key changed
                    let mut cover_art = shared.get_cover_art();
                    if !image_key.is_empty() && image_key != last_image_key {
                        last_image_key = image_key.clone();
                        cover_art = download_cover(roon_host, roon_port, &image_key);
                    }

                    eprintln!(
                        "{} [roon] {} \u{2014} {} ({})",
                        ts(),
                        artist,
                        title,
                        album
                    );

                    shared.set(Metadata {
                        title,
                        artist,
                        album,
                        image_key,
                        cover_art,
                    });
                }
            }
        }
    }
}

fn send_request(
    ws: &mut WebSocket<TcpStream>,
    reqid: &mut u64,
    name: &str,
    body: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rid = *reqid;
    *reqid += 1;
    let msg = if let Some(b) = body {
        let content = serde_json::to_vec(&b)?;
        format!(
            "MOO/1 REQUEST {}\nRequest-Id: {}\nContent-Length: {}\nContent-Type: application/json\n\n",
            name, rid, content.len()
        )
        .into_bytes()
        .into_iter()
        .chain(content)
        .collect::<Vec<u8>>()
    } else {
        format!("MOO/1 REQUEST {}\nRequest-Id: {}\n\n", name, rid).into_bytes()
    };
    ws.send(Message::Binary(msg.into()))?;
    Ok(())
}

fn recv_response(
    ws: &mut WebSocket<TcpStream>,
) -> Result<(String, std::collections::HashMap<String, String>, Value), Box<dyn std::error::Error>>
{
    loop {
        let msg = ws.read()?;
        let data: Vec<u8> = match msg {
            Message::Binary(d) => d.to_vec(),
            Message::Text(t) => t.as_bytes().to_vec(),
            _ => continue,
        };
        let (first, headers, body) = parse_moo_response(&data);
        return Ok((first, headers, body));
    }
}

fn send_complete_response(
    ws: &mut WebSocket<TcpStream>,
    request_id: &str,
) -> Result<(), tungstenite::Error> {
    let msg = format!(
        "MOO/1 COMPLETE Success\nRequest-Id: {}\n\n",
        request_id
    );
    ws.send(Message::Binary(msg.into_bytes().into()))?;
    Ok(())
}

fn send_continue_response(
    ws: &mut WebSocket<TcpStream>,
    request_id: &str,
    body: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let content = serde_json::to_vec(body)?;
    let msg = format!(
        "MOO/1 CONTINUE Changed\nRequest-Id: {}\nContent-Length: {}\nContent-Type: application/json\n\n",
        request_id,
        content.len()
    );
    let full: Vec<u8> = msg.into_bytes().into_iter().chain(content).collect();
    ws.send(Message::Binary(full.into()))?;
    Ok(())
}

fn parse_moo_response(data: &[u8]) -> (String, std::collections::HashMap<String, String>, Value) {
    let mut headers = std::collections::HashMap::new();
    let text = String::from_utf8_lossy(data);

    let sep = match text.find("\n\n") {
        Some(s) => s,
        None => return (String::new(), headers, Value::Null),
    };

    let header_part = &text[..sep];
    let body_part = &data[sep + 2..];

    let mut lines = header_part.lines();
    let first_line = lines.next().unwrap_or("").to_string();

    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    let body = if body_part.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_part).unwrap_or(Value::Null)
    };

    (first_line, headers, body)
}

fn load_token(token_file: &str) -> Option<String> {
    let data = std::fs::read_to_string(token_file).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string())
}

fn save_token(token_file: &str, token: &str) {
    let v = serde_json::json!({"token": token});
    if let Err(e) = std::fs::write(token_file, serde_json::to_string(&v).unwrap()) {
        eprintln!("{} [roon] failed to save token: {}", ts(), e);
    }
}

fn download_cover(roon_host: &str, roon_port: u16, image_key: &str) -> Option<Vec<u8>> {
    let url = format!(
        "http://{}:{}/api/image/{}?scale=fit&width=250&height=250&format=image/jpeg",
        roon_host, roon_port, image_key
    );
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    );
    match agent.get(&url).call() {
        Ok(resp) => {
            match resp.into_body().read_to_vec() {
                Ok(data) if data.len() > 100 && data[0] == 0xFF && data[1] == 0xD8 => {
                    eprintln!("{} [roon] cover art: {}b", ts(), data.len());
                    Some(data)
                }
                _ => None,
            }
        }
        Err(e) => {
            eprintln!("{} [roon] cover download failed: {}", ts(), e);
            None
        }
    }
}
