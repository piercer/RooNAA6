# RooNAA6 Rust NAA Proxy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust binary that proxies NAA v6 traffic between HQPlayer and an NAA endpoint, injecting Roon track metadata into the binary audio stream.

**Architecture:** Single binary with std threads (no async). TCP proxy with frame-level state machine intercepts the HQP→endpoint direction to inject/strip metadata. UDP multicast handles NAA discovery. Shared `Arc<Mutex<Metadata>>` carries track info from a Roon WebSocket listener to the forwarding thread.

**Tech Stack:** Rust (std threads), socket2 (multicast), tungstenite (WebSocket), serde_json, ureq (HTTP)

**Reference:** The Python proof-of-concept at `proxy/naa_proxy.py` is the authoritative implementation. The spec is at `docs/superpowers/specs/2026-04-05-rust-naa-proxy-design.md`.

---

## File Structure

```
Cargo.toml              — package "RooNAA6", Stage 1 deps: socket2
src/main.rs             — constants, TCP listener, thread spawning, timestamp helper
src/discovery.rs        — UDP multicast discovery responder
src/metadata.rs         — SharedMetadata wrapper (Arc<Mutex<>>), Metadata struct
src/frame.rs            — FrameHeader parsing, metadata section building, XML parsing (pure functions, unit-testable)
src/proxy.rs            — HQP→endpoint forwarding with frame state machine, T8→HQP passthrough
src/roon.rs             — (Stage 2) Roon WebSocket client, MOO/1 protocol, cover art download
```

---

## Stage 1: TCP Proxy + Discovery + Frame State Machine

### Task 1: Project scaffold and metadata types

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/metadata.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "RooNAA6"
version = "0.1.0"
edition = "2021"
description = "NAA v6 metadata-injecting proxy for HQPlayer → NAA endpoint"

[[bin]]
name = "RooNAA6"
path = "src/main.rs"

[dependencies]
socket2 = "0.5"
```

- [ ] **Step 2: Create src/metadata.rs**

```rust
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub image_key: String,
    pub cover_art: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct SharedMetadata {
    inner: Arc<Mutex<Metadata>>,
}

impl SharedMetadata {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Metadata::default())),
        }
    }

    pub fn get(&self) -> Metadata {
        self.inner.lock().unwrap().clone()
    }

    pub fn set(&self, meta: Metadata) {
        *self.inner.lock().unwrap() = meta;
    }

    pub fn get_cover_art(&self) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().cover_art.clone()
    }
}
```

- [ ] **Step 3: Create src/main.rs with constants and stub**

```rust
mod metadata;
mod discovery;
mod frame;
mod proxy;

use std::time::SystemTime;

pub const NAA_HOST: &str = "192.168.30.109";
pub const NAA_PORT: u16 = 43210;

pub fn ts() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs() % 86400;
    let millis = now.as_millis() % 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        millis
    )
}

fn main() {
    eprintln!("{} RooNAA6 starting", ts());
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles with warnings about unused modules (expected at this stage)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/main.rs src/metadata.rs
git commit -m "feat: project scaffold with metadata types"
```

---

### Task 2: Frame header parsing and metadata section building

**Files:**
- Create: `src/frame.rs`

This file contains pure functions — no I/O, fully unit-testable. It handles:
- Parsing 32-byte frame headers
- Building NAA metadata sections from track info
- Parsing XML start messages for stream parameters

- [ ] **Step 1: Write the failing tests**

```rust
// src/frame.rs

pub const FRAME_HEADER_SIZE: usize = 32;
pub const TYPE_PCM: u32 = 0x01;
pub const TYPE_PIC: u32 = 0x04;
pub const TYPE_META: u32 = 0x08;
pub const TYPE_POS: u32 = 0x10;

pub struct FrameHeader {
    pub type_mask: u32,
    pub pcm_len: u32,
    pub pos_len: u32,
    pub meta_len: u32,
    pub pic_len: u32,
}

impl FrameHeader {
    pub fn has_meta(&self) -> bool {
        self.type_mask & TYPE_META != 0
    }

    pub fn has_pic(&self) -> bool {
        self.type_mask & TYPE_PIC != 0
    }
}

pub struct StreamParams {
    pub bits: u32,
    pub rate: u32,
    pub is_dsd: bool,
    pub bytes_per_sample: u32,
}

/// Parse a 32-byte frame header. Returns None if buffer is too short.
pub fn parse_header(buf: &[u8]) -> Option<FrameHeader> {
    if buf.len() < FRAME_HEADER_SIZE {
        return None;
    }
    Some(FrameHeader {
        type_mask: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        pcm_len: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        pos_len: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        meta_len: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        pic_len: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
    })
}

/// Serialize a FrameHeader back to 32 bytes (for modified headers).
pub fn serialize_header(h: &FrameHeader) -> [u8; FRAME_HEADER_SIZE] {
    let mut buf = [0u8; FRAME_HEADER_SIZE];
    buf[0..4].copy_from_slice(&h.type_mask.to_le_bytes());
    buf[4..8].copy_from_slice(&h.pcm_len.to_le_bytes());
    buf[8..12].copy_from_slice(&h.pos_len.to_le_bytes());
    buf[12..16].copy_from_slice(&h.meta_len.to_le_bytes());
    buf[16..20].copy_from_slice(&h.pic_len.to_le_bytes());
    buf
}

/// Compute bytes_per_sample: max(1, bits / 8).
/// PCM (bits=32) → 4. DSD (bits=1) → 1.
pub fn bytes_per_sample(bits: u32) -> u32 {
    std::cmp::max(1, bits / 8)
}

/// Build the metadata section bytes for NAA v6.
/// Format: `[metadata]\n` + key=value lines + `\0`
///
/// `params` provides the format fields (bitrate, bits, channels, etc).
/// Only whitelisted fields — the NAA endpoint rejects unknown fields.
pub fn build_meta_section(
    params: &StreamParams,
    title: &str,
    artist: &str,
    album: &str,
) -> Vec<u8> {
    let meta_rate = if params.is_dsd { 2822400 } else { params.rate };
    let bitrate = meta_rate as u64 * params.bits as u64 * 2;
    let sdm = if params.is_dsd { 1 } else { 0 };
    let content = format!(
        "bitrate={}\nbits={}\nchannels=2\nfloat=0\nsamplerate={}\nsdm={}\nsong={}\nartist={}\nalbum={}\n",
        bitrate, params.bits, meta_rate, sdm, title, artist, album
    );
    let mut section = format!("[metadata]\n{}", content).into_bytes();
    section.push(0x00);
    section
}

/// Build a meta_template from stream parameters (used as default metadata).
/// This is what HQPlayer sends — format fields + `song=Roon`.
pub fn build_meta_template(params: &StreamParams) -> Vec<u8> {
    let meta_rate = if params.is_dsd { 2822400 } else { params.rate };
    let bitrate = meta_rate as u64 * params.bits as u64 * 2;
    let sdm = if params.is_dsd { 1 } else { 0 };
    format!(
        "bitrate={}\nbits={}\nchannels=2\nfloat=0\nsamplerate={}\nsdm={}\nsong=Roon\n",
        bitrate, params.bits, meta_rate, sdm
    )
    .into_bytes()
}

/// Parse an XML start message for stream parameters.
/// Extracts bits="N", rate="N", stream="pcm|dsd" from the XML text.
/// Returns None if this isn't a start message or lacks required attributes.
pub fn parse_start_message(xml: &[u8]) -> Option<StreamParams> {
    let text = std::str::from_utf8(xml).ok()?;
    if !text.contains("type=\"start\"") || text.contains("result=") {
        return None;
    }
    let bits = extract_xml_attr(text, "bits")?.parse::<u32>().ok()?;
    let rate = extract_xml_attr(text, "rate")?.parse::<u32>().ok()?;
    let stream = extract_xml_attr(text, "stream").unwrap_or("pcm".to_string());
    let is_dsd = stream == "dsd";
    Some(StreamParams {
        bits,
        rate,
        is_dsd,
        bytes_per_sample: bytes_per_sample(bits),
    })
}

/// Extract an XML attribute value: `name="value"` → `value`
fn extract_xml_attr(text: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    let start = text.find(&pattern)? + pattern.len();
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

/// Returns true if the header looks corrupt (sanity check).
pub fn is_corrupt(header: &FrameHeader) -> bool {
    header.pcm_len > 1_000_000 || header.pos_len > 10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_pcm() {
        // type=0x1D (PCM+POS+META+PIC), pcm_len=81920, pos_len=271, meta_len=100, pic_len=5000
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&0x1Du32.to_le_bytes());
        buf[4..8].copy_from_slice(&81920u32.to_le_bytes());
        buf[8..12].copy_from_slice(&271u32.to_le_bytes());
        buf[12..16].copy_from_slice(&100u32.to_le_bytes());
        buf[16..20].copy_from_slice(&5000u32.to_le_bytes());

        let h = parse_header(&buf).unwrap();
        assert_eq!(h.type_mask, 0x1D);
        assert_eq!(h.pcm_len, 81920);
        assert_eq!(h.pos_len, 271);
        assert_eq!(h.meta_len, 100);
        assert_eq!(h.pic_len, 5000);
        assert!(h.has_meta());
        assert!(h.has_pic());
    }

    #[test]
    fn test_parse_header_dsd() {
        // type=0x11 (PCM+POS), pcm_len=602112, pos_len=271
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&0x11u32.to_le_bytes());
        buf[4..8].copy_from_slice(&602112u32.to_le_bytes());
        buf[8..12].copy_from_slice(&271u32.to_le_bytes());

        let h = parse_header(&buf).unwrap();
        assert_eq!(h.type_mask, 0x11);
        assert_eq!(h.pcm_len, 602112);
        assert!(!h.has_meta());
        assert!(!h.has_pic());
    }

    #[test]
    fn test_parse_header_too_short() {
        let buf = [0u8; 16];
        assert!(parse_header(&buf).is_none());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let h = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 81920,
            pos_len: 271,
            meta_len: 100,
            pic_len: 5000,
        };
        let buf = serialize_header(&h);
        let h2 = parse_header(&buf).unwrap();
        assert_eq!(h2.type_mask, 0x1D);
        assert_eq!(h2.pcm_len, 81920);
        assert_eq!(h2.pos_len, 271);
        assert_eq!(h2.meta_len, 100);
        assert_eq!(h2.pic_len, 5000);
    }

    #[test]
    fn test_bytes_per_sample_pcm() {
        assert_eq!(bytes_per_sample(32), 4);
        assert_eq!(bytes_per_sample(16), 2);
        assert_eq!(bytes_per_sample(24), 3);
    }

    #[test]
    fn test_bytes_per_sample_dsd() {
        assert_eq!(bytes_per_sample(1), 1);
    }

    #[test]
    fn test_build_meta_section_pcm() {
        let params = StreamParams {
            bits: 32,
            rate: 384000,
            is_dsd: false,
            bytes_per_sample: 4,
        };
        let section = build_meta_section(&params, "My Song", "My Artist", "My Album");
        let text = String::from_utf8_lossy(&section);
        assert!(text.starts_with("[metadata]\n"));
        assert!(text.contains("bitrate=24576000\n"));
        assert!(text.contains("bits=32\n"));
        assert!(text.contains("samplerate=384000\n"));
        assert!(text.contains("sdm=0\n"));
        assert!(text.contains("song=My Song\n"));
        assert!(text.contains("artist=My Artist\n"));
        assert!(text.contains("album=My Album\n"));
        assert!(section.last() == Some(&0x00));
    }

    #[test]
    fn test_build_meta_section_dsd() {
        let params = StreamParams {
            bits: 1,
            rate: 22579200,
            is_dsd: true,
            bytes_per_sample: 1,
        };
        let section = build_meta_section(&params, "DSD Track", "Artist", "Album");
        let text = String::from_utf8_lossy(&section);
        // DSD always uses base rate 2822400 in metadata
        assert!(text.contains("bitrate=5644800\n"));
        assert!(text.contains("bits=1\n"));
        assert!(text.contains("samplerate=2822400\n"));
        assert!(text.contains("sdm=1\n"));
    }

    #[test]
    fn test_parse_start_message_pcm() {
        let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="32" channels="2" rate="384000" stream="pcm" type="start"/></networkaudio>"#;
        let params = parse_start_message(xml).unwrap();
        assert_eq!(params.bits, 32);
        assert_eq!(params.rate, 384000);
        assert!(!params.is_dsd);
        assert_eq!(params.bytes_per_sample, 4);
    }

    #[test]
    fn test_parse_start_message_dsd() {
        let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="1" channels="2" rate="22579200" stream="dsd" type="start"/></networkaudio>"#;
        let params = parse_start_message(xml).unwrap();
        assert_eq!(params.bits, 1);
        assert_eq!(params.rate, 22579200);
        assert!(params.is_dsd);
        assert_eq!(params.bytes_per_sample, 1);
    }

    #[test]
    fn test_parse_start_message_result_ignored() {
        // Response messages (with result=) should be ignored
        let xml = br#"<networkaudio><operation result="1" type="start" rate="384000" bits="32"/></networkaudio>"#;
        assert!(parse_start_message(xml).is_none());
    }

    #[test]
    fn test_parse_start_message_not_start() {
        let xml = br#"<networkaudio><operation type="stop"/></networkaudio>"#;
        assert!(parse_start_message(xml).is_none());
    }

    #[test]
    fn test_is_corrupt() {
        let h = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 2_000_000,
            pos_len: 271,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(is_corrupt(&h));

        let h2 = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 81920,
            pos_len: 20_000,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(is_corrupt(&h2));

        let h3 = FrameHeader {
            type_mask: 0x11,
            pcm_len: 602112,
            pos_len: 271,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(!is_corrupt(&h3));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test`
Expected: All 11 tests pass

- [ ] **Step 3: Commit**

```bash
git add src/frame.rs
git commit -m "feat: frame header parsing, metadata building, XML parsing with tests"
```

---

### Task 3: Discovery responder

**Files:**
- Create: `src/discovery.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create src/discovery.rs**

```rust
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::{Ipv4Addr, SocketAddrV4};

use crate::{ts, NAA_PORT};

const MCAST_ADDRS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(224, 0, 0, 199),
    Ipv4Addr::new(239, 192, 0, 199),
];

const DISCOVER_RESPONSE: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio>\
<discover result=\"OK\" name=\"RooNAA6 Proxy\" version=\"eversolo naa\" protocol=\"6\" trigger=\"0\">\
network audio\
</discover>\
</networkaudio>\n";

pub fn run(bind_addr: Ipv4Addr) {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    sock.set_reuse_address(true).unwrap();
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, NAA_PORT);
    sock.bind(&SockAddr::from(addr)).unwrap();

    for mcast in &MCAST_ADDRS {
        if let Err(e) = sock.join_multicast_v4(mcast, &bind_addr) {
            eprintln!("{} [discovery] failed to join {}: {}", ts(), mcast, e);
        }
    }

    eprintln!(
        "{} [discovery] listening on :{} (mcast on {})",
        ts(),
        NAA_PORT,
        bind_addr
    );

    let mut buf = [0u8; 4096];
    loop {
        match sock.recv_from(unsafe {
            &mut *(buf.as_mut_slice() as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
        }) {
            Ok((len, addr)) => {
                let data = &buf[..len];
                if data.windows(8).any(|w| w == b"discover")
                    && data.windows(13).any(|w| w == b"network audio")
                {
                    eprintln!("{} [discovery] responded to {}", ts(), addr.as_socket().unwrap());
                    let _ = sock.send_to(DISCOVER_RESPONSE, &addr);
                }
            }
            Err(e) => {
                eprintln!("{} [discovery] recv error: {}", ts(), e);
            }
        }
    }
}
```

- [ ] **Step 2: Update src/main.rs to spawn discovery thread**

```rust
mod metadata;
mod discovery;
mod frame;
mod proxy;

use std::net::Ipv4Addr;
use std::thread;
use std::time::SystemTime;

pub const NAA_HOST: &str = "192.168.30.109";
pub const NAA_PORT: u16 = 43210;
pub const BIND_ADDR: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
// For multicast, set this to the interface IP where HQPlayer discovers NAA devices
pub const MCAST_IFACE: Ipv4Addr = Ipv4Addr::new(192, 168, 30, 212);

pub fn ts() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs() % 86400;
    let millis = now.as_millis() % 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        millis
    )
}

fn main() {
    eprintln!("{} RooNAA6 starting", ts());

    thread::Builder::new()
        .name("discovery".into())
        .spawn(move || discovery::run(MCAST_IFACE))
        .unwrap();

    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        NAA_PORT,
        NAA_HOST,
        NAA_PORT
    );

    // TCP listener will go here in Task 4
    loop {
        thread::park();
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles (warnings about unused modules are fine)

- [ ] **Step 4: Commit**

```bash
git add src/discovery.rs src/main.rs
git commit -m "feat: NAA multicast discovery responder"
```

---

### Task 4: TCP listener and bidirectional forwarding (passthrough)

**Files:**
- Create: `src/proxy.rs`
- Modify: `src/main.rs`

Get a basic TCP proxy working first — no frame processing, just byte forwarding in both directions. This validates the network plumbing before adding the state machine.

- [ ] **Step 1: Create src/proxy.rs with passthrough forwarding**

```rust
use std::io::{Read, Write};
use std::net::TcpStream;

use crate::metadata::SharedMetadata;
use crate::ts;

/// Forward T8→HQP: simple byte passthrough.
pub fn forward_passthrough(mut src: TcpStream, mut dst: TcpStream, label: &str) {
    let mut buf = [0u8; 65536];
    loop {
        match src.read(&mut buf) {
            Ok(0) => {
                eprintln!("{} [{}] EOF", ts(), label);
                break;
            }
            Ok(n) => {
                if let Err(e) = dst.write_all(&buf[..n]) {
                    eprintln!("{} [{}] write error: {}", ts(), label, e);
                    break;
                }
            }
            Err(e) => {
                eprintln!("{} [{}] read error: {}", ts(), label, e);
                break;
            }
        }
    }
}

/// Forward HQP→T8: frame-level processing with metadata injection.
/// For now, just passthrough — the state machine comes in Task 5.
pub fn forward_hqp_to_naa(
    src: TcpStream,
    dst: TcpStream,
    _shared: SharedMetadata,
) {
    forward_passthrough(src, dst, "HQP->NAA");
}

/// Log XML messages (skip keepalive).
pub fn log_xml(label: &str, data: &[u8]) {
    if data.is_empty() || data[0] != b'<' {
        return;
    }
    if data.windows(9).any(|w| w == b"keepalive") {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        eprintln!("{} [{}] {}", ts(), label, text.trim());
    }
}
```

- [ ] **Step 2: Update src/main.rs with TCP listener**

Replace the `loop { thread::park(); }` at the end of main with:

```rust
    let listener = std::net::TcpListener::bind(("0.0.0.0", NAA_PORT)).unwrap();
    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        NAA_PORT,
        NAA_HOST,
        NAA_PORT
    );

    let shared = metadata::SharedMetadata::new();

    // Hardcoded test metadata for Stage 1
    shared.set(metadata::Metadata {
        title: "Test Title".into(),
        artist: "Test Artist".into(),
        album: "Test Album".into(),
        ..Default::default()
    });

    for stream in listener.incoming() {
        let client = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} accept error: {}", ts(), e);
                continue;
            }
        };
        let addr = client.peer_addr().unwrap();
        eprintln!("{} HQP connected from {}", ts(), addr);

        let naa = match std::net::TcpStream::connect((NAA_HOST, NAA_PORT)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} NAA connect failed: {}", ts(), e);
                continue;
            }
        };

        let client_r = client.try_clone().unwrap();
        let naa_r = naa.try_clone().unwrap();
        let client_w = client;
        let naa_w = naa;
        let shared_clone = shared.clone();

        let t1 = thread::Builder::new()
            .name("hqp-to-naa".into())
            .spawn(move || proxy::forward_hqp_to_naa(client_r, naa_w, shared_clone))
            .unwrap();

        let t2 = thread::Builder::new()
            .name("naa-to-hqp".into())
            .spawn(move || proxy::forward_passthrough(naa_r, client_w, "NAA->HQP"))
            .unwrap();

        t1.join().unwrap();
        t2.join().unwrap();
        eprintln!("{} Session ended", ts());
    }
```

Also remove the duplicate `eprintln!` for "NAA proxy" that was in the earlier stub, and remove the `loop { thread::park(); }`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles cleanly

- [ ] **Step 4: Commit**

```bash
git add src/proxy.rs src/main.rs
git commit -m "feat: TCP listener with bidirectional passthrough"
```

---

### Task 5: Frame-level state machine (the core)

**Files:**
- Modify: `src/proxy.rs`

Replace the passthrough `forward_hqp_to_naa` with the full frame-level state machine. This is the largest task — it implements the Header/Pass/Skip phases, XML detection, and all injection/stripping logic.

Reference: `proxy/naa_proxy.py` lines 76–321, function `forward_hqp_to_t8`.

- [ ] **Step 1: Replace forward_hqp_to_naa with state machine**

```rust
use std::io::{Read, Write};
use std::net::TcpStream;

use crate::frame::{
    self, build_meta_section, build_meta_template, is_corrupt, parse_header, parse_start_message,
    serialize_header, FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC,
};
use crate::metadata::SharedMetadata;
use crate::ts;

#[derive(PartialEq)]
enum Phase {
    Header,
    Pass,
    Skip,
}

/// Forward HQP→NAA with frame-level metadata injection.
///
/// First frame after start: inject Roon metadata (with cover art).
/// Gapless track change: when metadata reports new track, inject into next frame.
/// Subsequent META frames from HQPlayer: strip them.
/// Every 300 frames: periodic refresh (metadata only, no cover art).
pub fn forward_hqp_to_naa(mut src: TcpStream, mut dst: TcpStream, shared: SharedMetadata) {
    let mut phase = Phase::Header;
    let mut pass_remaining: usize = 0;
    let mut skip_remaining: usize = 0;
    let mut pending_inject: Option<Vec<u8>> = None;
    let mut header_buf = Vec::with_capacity(FRAME_HEADER_SIZE);
    let mut stream_params = StreamParams {
        bits: 32,
        rate: 44100,
        is_dsd: false,
        bytes_per_sample: 4,
    };
    let mut meta_template = build_meta_template(&stream_params);
    let mut injected_this_start = false;
    let mut last_injected_title: Option<String> = None;
    let mut frame_count: u64 = 0;

    let mut read_buf = [0u8; 65536];

    loop {
        let n = match src.read(&mut read_buf) {
            Ok(0) => {
                eprintln!("{} [HQP->NAA] EOF", ts());
                break;
            }
            Ok(n) => n,
            Err(e) => {
                eprintln!("{} [HQP->NAA] read error: {}", ts(), e);
                break;
            }
        };
        let data = &read_buf[..n];

        // Top-of-buffer XML check
        if phase == Phase::Header && header_buf.is_empty() {
            if let Some(first) = data.iter().position(|&b| b != b' ' && b != b'\t' && b != b'\n' && b != b'\r') {
                if data[first] == b'<' {
                    log_xml("HQP->NAA", data);
                    if let Some(params) = parse_start_message(data) {
                        stream_params = params;
                        meta_template = build_meta_template(&stream_params);
                        injected_this_start = false;
                        last_injected_title = None;
                        frame_count = 0;
                        header_buf.clear();
                        pass_remaining = 0;
                        skip_remaining = 0;
                        pending_inject = None;
                        eprintln!(
                            "{} [HQP->NAA] start: {} bytes/sample, {} {}Hz",
                            ts(),
                            stream_params.bytes_per_sample,
                            if stream_params.is_dsd { "dsd" } else { "pcm" },
                            stream_params.rate
                        );
                    }
                    if let Err(e) = dst.write_all(data) {
                        eprintln!("{} [HQP->NAA] write error: {}", ts(), e);
                        break;
                    }
                    continue;
                }
            }
        }

        // Binary frame processing
        let mut pos = 0;
        let mut out = Vec::with_capacity(n + 4096);

        while pos < n {
            match phase {
                Phase::Header => {
                    // Mid-buffer XML detection
                    if header_buf.is_empty() && data[pos] == b'<' {
                        if !out.is_empty() {
                            if let Err(e) = dst.write_all(&out) {
                                eprintln!("{} [HQP->NAA] write error: {}", ts(), e);
                                return;
                            }
                            out.clear();
                        }
                        let xml_data = &data[pos..];
                        log_xml("HQP->NAA", xml_data);
                        if let Some(params) = parse_start_message(xml_data) {
                            stream_params = params;
                            meta_template = build_meta_template(&stream_params);
                            injected_this_start = false;
                            last_injected_title = None;
                            frame_count = 0;
                            pass_remaining = 0;
                            skip_remaining = 0;
                            pending_inject = None;
                            eprintln!(
                                "{} [HQP->NAA] start: {} bytes/sample, {} {}Hz",
                                ts(),
                                stream_params.bytes_per_sample,
                                if stream_params.is_dsd { "dsd" } else { "pcm" },
                                stream_params.rate
                            );
                        }
                        if let Err(e) = dst.write_all(xml_data) {
                            eprintln!("{} [HQP->NAA] write error: {}", ts(), e);
                            return;
                        }
                        break; // rest of buffer is XML
                    }

                    // Accumulate header bytes
                    let need = FRAME_HEADER_SIZE - header_buf.len();
                    let take = std::cmp::min(need, n - pos);
                    header_buf.extend_from_slice(&data[pos..pos + take]);
                    pos += take;

                    if header_buf.len() == FRAME_HEADER_SIZE {
                        let mut header = parse_header(&header_buf).unwrap();
                        header_buf.clear();

                        let pcm_bytes = header.pcm_len as usize * stream_params.bytes_per_sample as usize;
                        let has_meta = header.has_meta();
                        let meta = shared.get();
                        let title = &meta.title;
                        frame_count += 1;

                        if is_corrupt(&header) {
                            eprintln!("{} [CORRUPT] pcm_len={} pos_len={}", ts(), header.pcm_len, header.pos_len);
                        }

                        if !title.is_empty() && has_meta && !injected_this_start {
                            // INJECT: first META frame after start
                            let orig_meta_len = header.meta_len as usize;
                            let orig_pic_len = header.pic_len as usize;

                            let meta_section = build_meta_section(
                                &stream_params, title, &meta.artist, &meta.album,
                            );
                            let jpeg = shared.get_cover_art();

                            header.type_mask |= TYPE_META;
                            if jpeg.is_some() { header.type_mask |= TYPE_PIC; }
                            header.meta_len = meta_section.len() as u32;
                            header.pic_len = jpeg.as_ref().map_or(0, |j| j.len() as u32);

                            let mut inject_data = meta_section;
                            if let Some(j) = jpeg { inject_data.extend_from_slice(&j); }

                            pending_inject = Some(inject_data);
                            injected_this_start = true;
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            eprintln!("{} [INJECT] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, header.pic_len);
                        } else if !title.is_empty()
                            && injected_this_start
                            && last_injected_title.as_deref() != Some(title)
                        {
                            // GAPLESS: Roon reports new track
                            let orig_meta_len = header.meta_len as usize;
                            let orig_pic_len = header.pic_len as usize;

                            let meta_section = build_meta_section(
                                &stream_params, title, &meta.artist, &meta.album,
                            );
                            let jpeg = shared.get_cover_art();

                            header.type_mask |= TYPE_META;
                            if jpeg.is_some() { header.type_mask |= TYPE_PIC; }
                            header.meta_len = meta_section.len() as u32;
                            header.pic_len = jpeg.as_ref().map_or(0, |j| j.len() as u32);

                            let mut inject_data = meta_section;
                            if let Some(j) = jpeg { inject_data.extend_from_slice(&j); }

                            pending_inject = Some(inject_data);
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            eprintln!("{} [GAPLESS] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, header.pic_len);
                        } else if has_meta && injected_this_start {
                            // STRIP: remove HQPlayer's META refresh
                            let orig_meta_len = header.meta_len as usize;
                            let orig_pic_len = header.pic_len as usize;
                            header.type_mask &= !(TYPE_META | TYPE_PIC);
                            header.meta_len = 0;
                            header.pic_len = 0;
                            pending_inject = None;
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;
                            eprintln!(
                                "{} [STRIP] META refresh stripped (frame {})",
                                ts(),
                                frame_count
                            );
                        } else if injected_this_start
                            && !title.is_empty()
                            && frame_count % 300 == 0
                        {
                            // REFRESH: periodic metadata re-injection (~30s)
                            let meta_section = build_meta_section(
                                &stream_params,
                                title,
                                &meta.artist,
                                &meta.album,
                            );
                            let orig_meta_len = header.meta_len as usize;
                            let orig_pic_len = header.pic_len as usize;
                            header.type_mask |= TYPE_META;
                            header.meta_len = meta_section.len() as u32;
                            pending_inject = Some(meta_section);
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;
                            eprintln!(
                                "{} [REFRESH] {} (frame {})",
                                ts(),
                                title,
                                frame_count
                            );
                        } else {
                            // PASSTHROUGH
                            pending_inject = None;
                            pass_remaining = pcm_bytes
                                + header.pos_len as usize
                                + header.meta_len as usize
                                + header.pic_len as usize;
                            skip_remaining = 0;
                        }

                        out.extend_from_slice(&serialize_header(&header));
                        phase = Phase::Pass;
                    }
                }

                Phase::Pass => {
                    let take = std::cmp::min(n - pos, pass_remaining);
                    out.extend_from_slice(&data[pos..pos + take]);
                    pos += take;
                    pass_remaining -= take;

                    if pass_remaining == 0 {
                        if let Some(inject) = pending_inject.take() {
                            out.extend_from_slice(&inject);
                        }
                        if skip_remaining > 0 {
                            phase = Phase::Skip;
                        } else {
                            phase = Phase::Header;
                        }
                    }
                }

                Phase::Skip => {
                    let take = std::cmp::min(n - pos, skip_remaining);
                    pos += take;
                    skip_remaining -= take;
                    if skip_remaining == 0 {
                        phase = Phase::Header;
                    }
                }
            }
        }

        if !out.is_empty() {
            if let Err(e) = dst.write_all(&out) {
                eprintln!("{} [HQP->NAA] write error: {}", ts(), e);
                break;
            }
        }
    }
}

/// Forward NAA→HQP: simple byte passthrough.
pub fn forward_passthrough(mut src: TcpStream, mut dst: TcpStream, label: &str) {
    let mut buf = [0u8; 65536];
    loop {
        match src.read(&mut buf) {
            Ok(0) => {
                eprintln!("{} [{}] EOF", ts(), label);
                break;
            }
            Ok(n) => {
                if buf[0] == b'<' {
                    log_xml(label, &buf[..n]);
                }
                if let Err(e) = dst.write_all(&buf[..n]) {
                    eprintln!("{} [{}] write error: {}", ts(), label, e);
                    break;
                }
            }
            Err(e) => {
                eprintln!("{} [{}] read error: {}", ts(), label, e);
                break;
            }
        }
    }
}

/// Log XML messages (skip keepalive).
pub fn log_xml(label: &str, data: &[u8]) {
    if data.is_empty() || data[0] != b'<' {
        return;
    }
    if data.windows(9).any(|w| w == b"keepalive") {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        eprintln!("{} [{}] {}", ts(), label, text.trim());
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles cleanly

- [ ] **Step 3: Run existing tests**

Run: `cargo test`
Expected: All frame.rs tests still pass

- [ ] **Step 4: Commit**

```bash
git add src/proxy.rs
git commit -m "feat: frame-level state machine with metadata injection"
```

---

### Task 6: Clean up old code and test on hardware

**Files:**
- Delete: `audio-bridge/` (entire directory)
- Delete: `metadata-extension/` (entire directory)
- Delete: `alsa-loopback/` (entire directory)
- Delete: `naa6-audio-bridge.service`
- Delete: `roon-metadata-extension.service`
- Delete: `config.json.example`
- Delete: `RooNAA6.iml`

- [ ] **Step 1: Remove obsolete files**

```bash
rm -rf audio-bridge metadata-extension alsa-loopback
rm -f naa6-audio-bridge.service roon-metadata-extension.service config.json.example RooNAA6.iml
```

- [ ] **Step 2: Build release binary**

Run: `cargo build --release`
Expected: Binary at `target/release/RooNAA6`

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove obsolete ALSA loopback code, keep Rust proxy"
```

- [ ] **Step 4: Deploy and test**

```bash
scp target/release/RooNAA6 conrad@<hqplayer_host>:/tmp/RooNAA6
ssh conrad@<hqplayer_host> 'fuser -k 43210/tcp 43210/udp 2>/dev/null; sleep 1'
ssh conrad@<hqplayer_host> 'nohup /tmp/RooNAA6 > /tmp/roonaa6.log 2>&1 &'
```

**Manual test checklist:**
- Play a PCM track → "Test Title" should appear on the NAA endpoint
- Wait 30s → REFRESH should fire in the log
- Play a DSD track → "Test Title" should appear, no corruption
- Gapless track change → title stays as "Test Title" (same hardcoded value)
- Manual track switch → "Test Title" re-appears after stop/start

- [ ] **Step 5: Commit test results (if fixes needed)**

---

## Stage 2: Roon WebSocket Integration

### Task 7: Roon metadata listener

**Files:**
- Create: `src/roon.rs`
- Modify: `Cargo.toml`
- Modify: `src/main.rs`

- [ ] **Step 1: Add Stage 2 dependencies to Cargo.toml**

Add to `[dependencies]`:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ureq = "3"
tungstenite = "0.26"
```

- [ ] **Step 2: Create src/roon.rs**

```rust
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::metadata::{Metadata, SharedMetadata};
use crate::ts;

const ROON_HOST: &str = "192.168.30.23";
const ROON_PORT: u16 = 9330;
const ZONE_NAME: &str = "Einstein";
const TOKEN_FILE: &str = "/tmp/roon_token.json";

pub fn run(shared: SharedMetadata) {
    loop {
        match run_once(&shared) {
            Ok(()) => {}
            Err(e) => eprintln!("{} [roon] connection error: {}", ts(), e),
        }
        eprintln!("{} [roon] reconnecting in 5s...", ts());
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn run_once(shared: &SharedMetadata) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("ws://{}:{}/api", ROON_HOST, ROON_PORT);
    let tcp = TcpStream::connect((ROON_HOST, ROON_PORT))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
    let (mut ws, _) = tungstenite::client_with_config(
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
    let display = body.get("display_name").and_then(|v| v.as_str()).unwrap_or("?");
    let version = body.get("display_version").and_then(|v| v.as_str()).unwrap_or("?");
    eprintln!("{} [roon] core: {} v{}", ts(), display, version);

    // Register extension
    let token = load_token();
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
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        let data = match msg {
            Message::Binary(d) => d,
            Message::Text(t) => t.into_bytes(),
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
            save_token(t);
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
                let zone_name = zone
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if zone_name != ZONE_NAME {
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
                        cover_art = download_cover(&image_key);
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
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    reqid: &mut u64,
    name: &str,
    body: Option<Value>,
) -> Result<(), tungstenite::Error> {
    let rid = *reqid;
    *reqid += 1;
    let msg = if let Some(b) = body {
        let content = serde_json::to_vec(&b).unwrap();
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
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
) -> Result<(String, std::collections::HashMap<String, String>, Value), Box<dyn std::error::Error>>
{
    loop {
        let msg = ws.read()?;
        let data = match msg {
            Message::Binary(d) => d.to_vec(),
            Message::Text(t) => t.into_bytes(),
            _ => continue,
        };
        let (first, headers, body) = parse_moo_response(&data);
        return Ok((first, headers, body));
    }
}

fn send_complete_response(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
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
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    request_id: &str,
    body: &Value,
) -> Result<(), tungstenite::Error> {
    let content = serde_json::to_vec(body).unwrap();
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

fn load_token() -> Option<String> {
    let data = std::fs::read_to_string(TOKEN_FILE).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string())
}

fn save_token(token: &str) {
    let v = serde_json::json!({"token": token});
    if let Err(e) = std::fs::write(TOKEN_FILE, serde_json::to_string(&v).unwrap()) {
        eprintln!("{} [roon] failed to save token: {}", ts(), e);
    }
}

fn download_cover(image_key: &str) -> Option<Vec<u8>> {
    let url = format!(
        "http://{}:{}/api/image/{}?scale=fit&width=250&height=250&format=image/jpeg",
        ROON_HOST, ROON_PORT, image_key
    );
    match ureq::get(&url).call() {
        Ok(resp) => {
            let mut data = Vec::new();
            if resp.into_body().read_to_end(&mut data).is_ok()
                && data.len() > 100
                && data[0] == 0xFF
                && data[1] == 0xD8
            {
                eprintln!("{} [roon] cover art: {}b", ts(), data.len());
                Some(data)
            } else {
                None
            }
        }
        Err(e) => {
            eprintln!("{} [roon] cover download failed: {}", ts(), e);
            None
        }
    }
}
```

- [ ] **Step 3: Update src/main.rs to spawn Roon thread**

Add after the discovery thread spawn, before the TCP listener:

```rust
    let shared_roon = shared.clone();
    thread::Builder::new()
        .name("roon".into())
        .spawn(move || roon::run(shared_roon))
        .unwrap();
```

Remove the hardcoded test metadata `shared.set(...)` — live data will come from Roon now.

Add `mod roon;` to the module declarations at the top.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles cleanly

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/roon.rs src/main.rs
git commit -m "feat: Roon WebSocket metadata listener with cover art"
```

---

### Task 8: End-to-end test with Roon

**Files:** None (manual testing)

- [ ] **Step 1: Build and deploy**

```bash
cargo build --release
scp target/release/RooNAA6 conrad@<hqplayer_host>:/tmp/RooNAA6
ssh conrad@<hqplayer_host> 'fuser -k 43210/tcp 43210/udp 2>/dev/null; sleep 1'
ssh conrad@<hqplayer_host> 'nohup /tmp/RooNAA6 > /tmp/roonaa6.log 2>&1 &'
```

- [ ] **Step 2: Verify Roon connection**

Check log: `ssh conrad@<hqplayer_host> 'grep "roon" /tmp/roonaa6.log | head -5'`
Expected: `[roon] connected`, `[roon] paired!`, `[roon] <artist> — <title>`

- [ ] **Step 3: Test PCM playback**

Play a PCM track in Roon. Check:
- Title, artist, album display correctly on the NAA endpoint
- Cover art appears
- REFRESH fires every ~30s in the log

- [ ] **Step 4: Test DSD playback**

Play a DSD track. Check:
- Title displays correctly
- No CORRUPT warnings
- No title revert

- [ ] **Step 5: Test track transitions**

- Gapless track change → [GAPLESS] in log, metadata updates
- Manual track switch → [INJECT] in log after stop/start
- Both PCM and DSD

- [ ] **Step 6: Commit final state**

```bash
git add -A
git commit -m "feat: RooNAA6 Rust proxy — full Roon integration verified"
```
