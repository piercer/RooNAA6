# RooNAA6 Rust NAA Proxy — Design Spec

## Goal

Replace the Python NAA proxy (`proxy/naa_proxy.py`) with a production Rust binary that proxies NAA v6 traffic between HQPlayer and the Eversolo T8, injecting Roon track metadata (title, artist, album, cover art) into the binary PCM/DSD stream.

## Background

The Python proxy is a working proof-of-concept (commit 859f7a8). It handles PCM and DSD512 streams, gapless and manual track changes, periodic metadata refresh, and cover art injection. The Rust port is a direct 1:1 port — no new features, no architectural changes. The goal is a deployable, long-running binary on the HQPlayer host machine.

## Phasing

### Stage 1 — TCP proxy + discovery + frame state machine

Get the binary protocol handling working with hardcoded test metadata. No Roon integration. This stage is fully testable on real hardware — play music and see "Test Title" on the T8.

Includes:
- NAA multicast discovery responder (UDP `:43210`)
- TCP listener on `:43210`, one HQPlayer connection at a time
- Bidirectional forwarding (HQP→T8 with frame processing, T8→HQP passthrough)
- Frame-level state machine (Header/Pass/Skip phases)
- XML control message detection (top-of-buffer and mid-buffer)
- Start message parsing (bits, rate, stream) for dynamic meta_template
- Metadata injection (first frame, gapless, periodic refresh every 300 frames)
- META stripping (HQPlayer refresh frames)
- DSD support (bytes_per_sample = max(1, bits/8), pcm_len is already in bytes)
- Hardcoded test metadata: song="Test Title", artist="Test Artist", album="Test Album", no cover art

### Stage 2 — Roon WebSocket integration

Replace hardcoded metadata with live Roon data.

Includes:
- WebSocket client to Roon Core (ws://<roon_host>:9330/api)
- MOO/1 protocol: register extension, handle pairing/ping, subscribe to transport zones
- Track metadata extraction (title, artist, album, image_key) from the configured zone
- Cover art download (250x250 JPEG via Roon HTTP API)
- Token persistence (/tmp/roon_token.json)
- Auto-reconnect on WebSocket errors

## Architecture

Single binary, `RooNAA6`. std threads, no async runtime.

### Threads

| Thread | Role |
|--------|------|
| Main | TCP listener on `:43210`, accepts connections, spawns forwarding threads |
| Discovery | UDP multicast responder on `:43210` (224.0.0.199, 239.192.0.199) |
| HQP→T8 | Frame-level state machine with metadata injection |
| T8→HQP | Simple byte passthrough |
| Roon (Stage 2) | WebSocket metadata listener, auto-reconnect |

### Shared state

`Arc<Mutex<Metadata>>` between Roon thread and HQP→T8 thread. Contains:
- `title: String`
- `artist: String`
- `album: String`
- `image_key: String`
- `cover_art: Option<Vec<u8>>` (JPEG bytes)

Same pattern as the Python `_meta_lock` + `_metadata` dict.

### Configuration

Constants at top of `main.rs` — easy to change per deployment:
- `NAA_HOST: &str` — NAA endpoint (proxy target)
- `NAA_PORT: u16 = 43210`
- `ROON_HOST: &str` — Roon Core IP (Stage 2)
- `ROON_PORT: u16 = 9330` (Stage 2)
- `ZONE_NAME: &str` — Roon zone to monitor (Stage 2)

## NAA v6 Frame Format

### Header (32 bytes, little-endian)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | type bitmask (0x01=PCM, 0x04=PIC, 0x08=META, 0x10=POS) |
| 4 | 4 | pcm_len (PCM: samples, DSD: bytes) |
| 8 | 4 | pos_len (bytes) |
| 12 | 4 | meta_len (bytes) |
| 16 | 4 | pic_len (bytes) |
| 20 | 12 | padding (zeros) |

### Body layout

`[PCM data][POS data][META section][PIC section]`

### PCM vs DSD sizing

- PCM (bits=32): `pcm_bytes = pcm_len * 4`
- DSD (bits=1): `pcm_bytes = pcm_len * 1` (pcm_len is already in bytes)
- Formula: `bytes_per_sample = max(1, bits / 8)`

### Metadata section format

```
[metadata]\n
key=value\n
...
\x00
```

### T8 metadata field whitelist

Only these fields are accepted. Any unknown field causes the T8 to reject the entire section:

`bitrate`, `bits`, `channels`, `float`, `samplerate`, `sdm`, `song`, `artist`, `album`

### T8 constraints

- Cover art > 100KB crashes T8
- meta_len and frame type can change mid-stream
- Excessive per-frame I/O disrupts DSD streaming
- CORRUPT threshold: pcm_len > 1,000,000 or pos_len > 10,000

## Frame Processing State Machine

Three phases, matching the Python implementation:

### Phase: Header
- Accumulate 32 bytes
- Check for XML at frame boundary (`<` as first byte when header buffer is empty)
- Parse header fields
- Decide action based on conditions (in priority order):
  1. **INJECT**: title exists, has_meta, not yet injected this start → inject metadata + cover
  2. **GAPLESS**: title exists, already injected, title differs from last → inject metadata + cover
  3. **STRIP**: has_meta, already injected → strip HQPlayer's META+PIC
  4. **REFRESH**: already injected, title exists, frame_count % 300 == 0 → inject metadata only
  5. **PASSTHROUGH**: forward everything unchanged

### Phase: Pass
- Copy `pass_remaining` bytes to output (PCM + POS data)
- When done: append `pending_inject` if any, transition to Skip or Header

### Phase: Skip
- Discard `skip_remaining` bytes (original META + PIC being replaced)
- When done: transition to Header

### XML handling

- Top-of-buffer: if data starts with `<`, treat as XML, forward, parse start messages
- Mid-buffer: if `<` found at frame boundary during Header phase, flush output, forward XML
- Start message: extract `bits`, `rate`, `stream` attributes; build meta_template; reset state
- DSD: `stream="dsd"` → `meta_rate = 2822400` (HQPlayer reports DSD64 base rate regardless)

## File Structure

```
RooNAA6/
├── Cargo.toml
└── src/
    ├── main.rs          — entry point, TCP listener, thread spawning
    ├── discovery.rs     — UDP multicast responder
    ├── proxy.rs         — HQP→T8 forwarding with frame state machine
    ├── frame.rs         — header parsing, metadata section building
    ├── roon.rs          — (Stage 2) WebSocket client, MOO/1 protocol
    └── metadata.rs      — shared metadata state, Arc<Mutex<>> wrapper
```

## Dependencies

### Stage 1
- `socket2` — multicast UDP setup for discovery

### Stage 2 (added)
- `serde` + `serde_json` — Roon JSON parsing
- `ureq` — blocking HTTP for cover art download
- `tungstenite` — blocking WebSocket for Roon MOO/1 protocol

No logging framework — `eprintln!` with timestamps. No XML parser — string matching for the simple XML messages.

## Testing

### Unit tests (in `frame.rs`)
- Header parsing: PCM frame, DSD frame, corrupt detection
- Metadata section building: PCM format fields, DSD format fields
- `bytes_per_sample` calculation: bits=32→4, bits=1→1
- XML start message parsing: extract bits/rate/stream

### Manual testing
- Deploy to HQPlayer host, play PCM and DSD tracks, verify metadata on the NAA endpoint
- Test gapless track changes, manual track switches, periodic refresh
- Same process as the Python proxy testing

## Deployment

1. `cargo build --release`
2. `scp target/release/RooNAA6 <hqplayer_host>:/tmp/RooNAA6`
3. Run the binary (replaces Python proxy)
4. Systemd service file for production (future)

## What this does NOT include

- No new features beyond what the Python proxy does
- No config file — hardcoded constants
- No async runtime
- No XML parser library
- No timing/position metadata (T8 rejects unknown fields)
