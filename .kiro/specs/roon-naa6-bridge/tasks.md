# Implementation Plan: roon-naa6-bridge

## Overview

Rewrite the `audio-bridge` Rust binary from a TCP client with binary framing to a TCP server
implementing the real XML-over-TCP NAA protocol. Update `metadata-extension` to forward metadata
as XML over the IPC socket. ALSA loopback config and systemd service files already exist.

---

## Tasks

### Part A: Rewrite naa6-audio-bridge (Rust)

- [ ] 1. Rewrite types and config for the server model
  - Delete `src/naa6_client.rs`; remove `hqplayer_host` / `hqplayer_port` fields from `Config` and `types.rs`
  - Add `listen_port: u16` (default 43210) to `Config`
  - Redefine `FormatDescriptor` to hold `bits: u8`, `channels: u8`, `rate: u32`, `netbuftime: u32`, `stream: String`
  - Remove `Naa6Frame`, `Encoding`, `DsdRate`, `naa6_msg_type` — binary framing is gone
  - Add `TrackMetadata { title, artist, album }` (no cover art in XML path)
  - Update `Cargo.toml`: add `quick-xml` (XML parsing/serialisation); remove unused deps
  - _Requirements: 3.1, 4.1, 7.2_

- [ ] 2. Implement ConfigReader (audio-bridge)
  - [ ] 2.1 Rewrite `src/config_reader.rs`
    - Read `/etc/roon-naa6-bridge/config.json` falling back to `./config.json`; use defaults + warn if absent
    - Validate `listen_port` ∈ [1, 65535]; log error and `exit(1)` on invalid value
    - Warn on unknown keys; return frozen `Config`
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_

  - [ ]* 2.2 Write unit tests for ConfigReader
    - Missing file uses defaults (Req 7.3)
    - Invalid `listenPort` exits non-zero (Req 7.5)
    - Unknown key produces warning and startup continues (Req 7.4)
    - _Requirements: 7.3, 7.4, 7.5_

- [ ] 3. Implement XML message parsing and serialisation
  - [ ] 3.1 Create `src/xml_protocol.rs`
    - Parse newline-terminated UTF-8 XML strings from TCP using `quick-xml`
    - Recognise `type="keepalive"`, `type="start"`, `type="stop"` in `<operation>` elements
    - Extract `bits`, `channels`, `rate`, `netbuftime`, `stream` attributes from start messages
    - Serialise keepalive response: `<?xml version="1.0" encoding="utf-8"?><networkaudio><operation result="1" type="keepalive"/></networkaudio>\n`
    - Serialise start response: echo `bits`, `channels`, `rate`, `netbuftime`, `stream` plus `dsd="0"` and `result="1"`
    - Serialise metadata message: `type="metadata"` with `title`, `artist`, `album` attributes
    - All outgoing messages begin with `<?xml version="1.0" encoding="utf-8"?>` and end with `\n`
    - _Requirements: 3.3, 4.1, 4.2, 6.2, 11.1, 11.2, 11.3_

  - [ ]* 3.2 Write unit tests for xml_protocol
    - Keepalive response matches exact expected string (Req 11.2)
    - Start response echoes all five attributes plus `dsd="0"` and `result="1"` (Req 11.3)
    - Unsupported `stream` value produces `result="0"` response (Req 4.5)
    - Metadata message contains `type="metadata"` and all three text attributes (Req 6.2)
    - _Requirements: 4.2, 4.5, 6.2, 11.2, 11.3_

  - [ ]* 3.3 Write property test — Property 1: XML round-trip consistency
    - **Property 1: XML round-trip consistency**
    - **Validates: Requirements 11.5**
    - Generate random valid XML messages (keepalive, start, stop); parse then re-serialise; assert semantic equivalence
    - `proptest!` with 100 cases; comment: `// Feature: roon-naa6-bridge, Property 1: XML round-trip consistency`

- [ ] 4. Implement NAA TCP server and session handler
  - [ ] 4.1 Rewrite `src/main.rs` — TCP listener loop
    - Bind `TcpListener` on `config.listen_port`; log startup
    - Accept one connection at a time; on disconnect resume listening (Req 3.5, 3.6)
    - Pass accepted `TcpStream` to session handler
    - _Requirements: 3.1, 3.2, 3.5, 3.6_

  - [ ] 4.2 Create `src/session.rs` — per-connection session state machine
    - States: `Idle` → `Streaming` → `Idle`
    - In `Idle`: read newline-terminated XML lines; dispatch keepalive/start/stop
    - On keepalive: send keepalive response within 1 s (Req 3.4)
    - On start with `stream != "pcm"`: send `result="0"`, log error, stay `Idle` (Req 4.5)
    - On valid start: send start response, send 16-byte sync marker (all zeros), transition to `Streaming` (Req 4.2, 4.3)
    - In `Streaming`: spawn ALSA reader thread; forward PCM bytes to TCP; read and discard 32-byte position packets from HQPlayer (Req 5.5)
    - On stop: cease streaming, transition to `Idle` (Req 4.4)
    - Log all state transitions and XML messages at DEBUG (Req 8.4)
    - _Requirements: 3.3, 3.4, 4.2, 4.3, 4.4, 4.5, 5.5, 8.1, 8.4_

  - [ ]* 4.3 Write unit tests for session state machine
    - Keepalive in Idle → response sent, stays Idle
    - Start with `stream="pcm"` → response + sync marker sent, transitions to Streaming
    - Start with `stream="dsd"` → `result="0"` sent, stays Idle (Req 4.5)
    - Stop in Streaming → transitions to Idle
    - _Requirements: 3.4, 4.2, 4.3, 4.4, 4.5_

  - [ ]* 4.4 Write property test — Property 2: Sync marker always follows start response
    - **Property 2: Sync marker always follows start response**
    - **Validates: Requirements 4.3**
    - Generate random valid start messages; assert 16 zero bytes immediately follow the start response XML
    - `proptest!` with 100 cases; comment: `// Feature: roon-naa6-bridge, Property 2: Sync marker always follows start response`

- [ ] 5. Rewrite ALSA capture for the server model
  - [ ] 5.1 Rewrite `src/alsa_capture.rs`
    - Open ALSA PCM capture device from `config.alsa_device`; exit(1) if cannot open (Req 2.6)
    - Configure hw_params using `FormatDescriptor` received from start message (bits, channels, rate)
    - Read frames via `snd_pcm_readi`; on `-EPIPE` call `snd_pcm_prepare` and log warning (Req 2.5)
    - Write raw PCM bytes to a `std::sync::mpsc::SyncSender<Vec<u8>>` channel
    - Support sample rates: 44100, 48000, 88200, 96000, 176400, 192000 (Req 2.2)
    - Support bit depths: 16, 24, 32 (Req 2.3); channels: 2 (Req 2.4)
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6_

  - [ ] 5.2 Wire ALSA output to TCP in `src/session.rs`
    - Receive `Vec<u8>` from ALSA channel; write to `TcpStream` without modification (Req 5.1, 5.2)
    - On TCP write returning `WouldBlock`: block ALSA channel (backpressure) rather than dropping frames (Req 5.4)
    - Log audio streaming started / stopped events (Req 8.1)
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 8.1_

  - [ ]* 5.3 Write unit tests for AlsaCaptureReader
    - Overrun (`-EPIPE`) triggers `snd_pcm_prepare` and logs warning (Req 2.5)
    - Fatal open failure exits non-zero (Req 2.6)
    - _Requirements: 2.5, 2.6_

  - [ ]* 5.4 Write property test — Property 3: Audio bytes forwarded unmodified
    - **Property 3: Audio bytes forwarded unmodified**
    - **Validates: Requirements 5.1, 5.2**
    - Generate random byte buffers; assert bytes written to TCP equal bytes read from ALSA channel
    - `proptest!` with 100 cases; comment: `// Feature: roon-naa6-bridge, Property 3: Audio bytes forwarded unmodified`

- [ ] 6. Implement IPC socket listener for metadata
  - [ ] 6.1 Create `src/ipc_listener.rs`
    - Listen on Unix domain socket path from `config.ipc_socket`
    - Accept connections from `roon-metadata-extension`
    - Read newline-delimited JSON `{ "title": "...", "artist": "...", "album": "..." }` messages
    - Deserialise into `TrackMetadata`; send to session via `std::sync::mpsc::Sender<TrackMetadata>`
    - _Requirements: 6.1, 6.5_

  - [ ] 6.2 Forward metadata as XML in `src/session.rs`
    - When `TrackMetadata` arrives on the channel and session is in any state: serialise as XML metadata message and write to TCP (Req 6.2)
    - Transmit within 500 ms of receipt (Req 6.3)
    - Log metadata forwarded event at DEBUG (Req 8.4)
    - _Requirements: 6.2, 6.3, 6.5_

  - [ ]* 6.3 Write unit tests for IPC listener
    - Valid JSON metadata parsed and forwarded to session channel
    - Malformed JSON logged and discarded without crashing
    - _Requirements: 6.1, 6.5_

  - [ ]* 6.4 Write property test — Property 4: Metadata UTF-8 round-trip
    - **Property 4: Metadata UTF-8 round-trip**
    - **Validates: Requirements 6.5**
    - Generate random Unicode strings for title/artist/album; encode as XML; parse back; assert equality
    - `proptest!` with 100 cases; comment: `// Feature: roon-naa6-bridge, Property 4: Metadata UTF-8 round-trip`

- [ ] 7. Rewrite ShutdownHandler (audio-bridge)
  - [ ] 7.1 Rewrite `src/shutdown.rs`
    - Register `SIGTERM` and `SIGINT` via `signal-hook`
    - On signal: set 5-second hard-kill alarm, stop ALSA capture, close TCP connection, close IPC socket, exit(0)
    - If 5 seconds exceeded: exit(1) (Req 9.4)
    - Log each shutdown step (Req 8.1)
    - _Requirements: 9.1, 9.3, 9.4, 8.1_

  - [ ]* 7.2 Write unit tests for ShutdownHandler
    - Force-exit with non-zero code when 5-second timeout exceeded (Req 9.4)
    - _Requirements: 9.4_

- [ ] 8. Checkpoint — Ensure all audio-bridge tests pass
  - Ensure all tests pass; ask the user if questions arise.

---

### Part B: Update roon-metadata-extension (Node.js)

- [ ] 9. Update types and remove binary NAA6 framing
  - In `src/types.ts`: remove `Naa6Frame`, `NAA6_MSG_TYPE`, `encodeNaa6Frame`; remove `hqplayerHost` / `hqplayerPort` from `Config`; add `ipcSocket: string` if not present; remove `coverArt` from `TrackMetadata`
  - _Requirements: 6.1, 7.2_

- [ ] 10. Rewrite MetadataForwarder to send JSON over IPC
  - [ ] 10.1 Rewrite `src/MetadataForwarder.ts`
    - Connect to Unix domain socket at `config.ipcSocket`
    - On `onMetadataChanged(metadata)`: if no title/artist/album, log DEBUG and return (Req 6.4)
    - Serialise `{ title, artist, album }` as UTF-8 JSON followed by `\n`
    - Write to IPC socket within 500 ms of event receipt (Req 6.3)
    - Reconnect on socket error with backoff
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

  - [ ]* 10.2 Write unit tests for MetadataForwarder
    - No message sent when all metadata fields absent (Req 6.4)
    - JSON payload contains title, artist, album fields (Req 6.1)
    - Message sent within 500 ms deadline (Req 6.3)
    - _Requirements: 6.1, 6.3, 6.4_

  - [ ]* 10.3 Write property test — Property 5: Metadata JSON round-trip
    - **Property 5: Metadata JSON round-trip**
    - **Validates: Requirements 6.5**
    - Generate random Unicode strings; serialise to JSON; parse back; assert equality
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 5: Metadata JSON round-trip`

- [ ] 11. Verify remaining metadata-extension components need no changes
  - Confirm `ConfigReader.ts` already handles `ipcSocket` and `listenPort` fields correctly
  - Confirm `RoonExtension.ts` emits metadata events with `title`, `artist`, `album` (no cover art needed)
  - Confirm `ShutdownHandler.ts` deregisters Roon Output on SIGTERM/SIGINT (Req 9.2)
  - Make any minor fixes found during review
  - _Requirements: 1.5, 7.1, 9.2_

- [ ] 12. Checkpoint — Ensure all metadata-extension tests pass
  - Ensure all tests pass; ask the user if questions arise.

---

### Part C: Integration wiring

- [ ] 13. Wire IPC socket between the two processes
  - Confirm `config.ipcSocket` default path (`/run/roon-naa6-bridge/meta.sock`) is consistent in both `audio-bridge` and `metadata-extension`
  - Confirm `naa6-audio-bridge.service` creates the socket directory (`RuntimeDirectory=roon-naa6-bridge`)
  - Confirm `roon-metadata-extension.service` has `After=naa6-audio-bridge.service`
  - _Requirements: 6.1, 10.3, 10.4_

- [ ] 14. Final checkpoint — Ensure all tests pass
  - Run `cargo test` in `audio-bridge/` and `npm test -- --run` in `metadata-extension/`
  - Ensure all tests pass; ask the user if questions arise.

---

## Notes

- Tasks marked with `*` are optional and can be skipped for a faster MVP
- Property tests in `audio-bridge` use `proptest` crate with 100 cases minimum
- Property tests in `metadata-extension` use `fast-check` with `numRuns: 100`
- The old binary NAA6 framing (type byte + uint32 LE length + payload) is entirely replaced by XML-over-TCP
- The IPC between the two processes is newline-delimited JSON (not binary NAA6 frames)
- Position packets (32 bytes) from HQPlayer during streaming must be read and discarded, not parsed as XML
