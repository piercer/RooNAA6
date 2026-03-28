# Implementation Plan: roon-naa6-bridge

## Overview

Implement a two-process bridge between Roon's audio output and HQPlayer's NAA 6 protocol.

**Process 1 — naa6-audio-bridge** (C or Rust): reads raw PCM/DSD frames from the ALSA loopback capture device and forwards them to HQPlayer via the NAA 6 protocol over TCP.

**Process 2 — roon-metadata-extension** (Node.js): connects to Roon Core as a Roon Extension (control plane only) and forwards track metadata to HQPlayer via NAA 6 metadata messages.

Both processes run as separate systemd services on Ubuntu 24.04 LTS and share a single JSON config file.

---

## Tasks

### Part A: System Prerequisites

- [ ] 1. Configure ALSA loopback
  - Create `/etc/modules-load.d/snd-aloop.conf` to load `snd-aloop` at boot
  - Create `/etc/modprobe.d/snd-aloop.conf` with `options snd-aloop enable=1 index=2`
  - Document how to configure Roon Bridge to output to `hw:Loopback,0,0`
  - _Requirements: 2.1_

---

### Part B: naa6-audio-bridge (C or Rust binary)

- [ ] 2. Initialise naa6-audio-bridge project
  - Create `audio-bridge/` directory
  - If Rust: initialise `Cargo.toml` with dependencies (`alsa`, `serde_json`, `proptest`)
  - If C: create `Makefile`, declare dependencies (`libasound2-dev`, `cmocka`, `theft`)
  - Create `src/types.{h,rs}` defining `Config`, `FormatDescriptor`, `NAA6Frame` structs
  - _Requirements: 2.2, 2.3, 2.4, 2.5, 6.2_

- [ ] 3. Implement ConfigReader (audio-bridge)
  - [ ] 3.1 Implement config loading
    - Read `/etc/roon-naa6-bridge/config.json`, fall back to `./config.json`; if absent, use defaults and log a warning
    - Validate `hqplayerPort` ∈ [1, 65535] and `reconnectBackoff` > 0; on invalid value log error and exit(1)
    - Warn on unknown keys; return a frozen/const `Config`
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

  - [ ]* 3.2 Write unit tests for ConfigReader
    - Missing file uses defaults (Req 6.3)
    - Invalid `hqplayerPort` exits non-zero (Req 6.5)
    - Unknown key produces warning and startup continues (Req 6.4)
    - _Requirements: 6.3, 6.4, 6.5_

  - [ ]* 3.3 Write property test — Property 9: Invalid config exits non-zero
    - **Property 9: Invalid config values cause non-zero exit**
    - **Validates: Requirements 6.5**
    - Generate random configs with out-of-range `hqplayerPort` or non-positive `reconnectBackoff`; assert exit(non-zero)
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 9: Invalid config values cause non-zero exit`

  - [ ]* 3.4 Write property test — Property 10: Unknown keys warn, continue
    - **Property 10: Unknown config keys produce a warning and do not halt startup**
    - **Validates: Requirements 6.4**
    - Generate valid configs augmented with random extra keys; assert warning logged and startup completes
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 10: Unknown config keys produce a warning and do not halt startup`

- [ ] 4. Implement NAA6Client (audio-bridge)
  - [ ] 4.1 Implement NAA6Client
    - Manage raw TCP socket to HQPlayer
    - Implement NAA 6 frame encoding: 1-byte type + uint32 LE length + payload
    - Implement `naa6_connect`, `naa6_handshake`, `naa6_send_audio`, `naa6_send_format_change`, `naa6_send_metadata`, `naa6_send_keepalive`, `naa6_send_termination`, `naa6_disconnect`
    - Keepalive timer at 1 s interval while connected
    - On `send()` returning EAGAIN/EWOULDBLOCK, signal BackpressureController; resume when socket writable
    - Emit disconnect event on TCP error or close
    - _Requirements: 3.1, 3.2, 3.3, 3.5, 3.6, 4.5_

  - [ ]* 4.2 Write unit tests for NAA6Client
    - Handshake sent before audio (Req 3.2)
    - Termination message sent on shutdown (Req 3.6)
    - Error logged with description and code on handshake failure (Req 3.5, 7.2)
    - _Requirements: 3.2, 3.5, 3.6, 7.2_

  - [ ]* 4.3 Write property test — Property 5: Handshake precedes audio
    - **Property 5: Handshake precedes audio data**
    - **Validates: Requirements 3.2**
    - Simulate session start with random FormatDescriptors; assert no audio bytes written before handshake ACK
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 5: Handshake precedes audio data`

  - [ ]* 4.4 Write property test — Property 7: Handshake carries FormatDescriptor
    - **Property 7: Handshake carries the FormatDescriptor**
    - **Validates: Requirements 5.1, 5.6**
    - Generate random valid FormatDescriptors; assert handshake payload fields match
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 7: Handshake carries the FormatDescriptor`

  - [ ]* 4.5 Write property test — Property 11: Error log entries contain description and error code
    - **Property 11: Error log entries contain description and error code**
    - **Validates: Requirements 7.2**
    - Inject random TCP errors; assert each log entry has a description string and error code
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 11: Error log entries contain description and error code`

- [ ] 5. Implement AlsaCaptureReader and BackpressureController
  - [ ] 5.1 Implement AlsaCaptureReader
    - Open ALSA PCM capture device from `Config.alsaDevice`
    - Call `snd_pcm_hw_params` to configure format, sample rate, channels
    - Read frames in a loop via `snd_pcm_readi`; on `-EPIPE` (overrun) call `snd_pcm_prepare` and log warning
    - Pass raw frames to `NAA6Client.naa6_send_audio`
    - Honour pause signal from `BackpressureController`
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6_

  - [ ] 5.2 Implement FormatDetector
    - After each `snd_pcm_prepare` or hw_params change, read current params and build `FormatDescriptor`
    - On change, call `naa6_send_format_change` before next audio frame
    - _Requirements: 2.7_

  - [ ] 5.3 Implement BackpressureController
    - Maintain `paused` flag
    - On NAA6Client buffer-full signal: set `paused = true`, call `snd_pcm_pause(1)`
    - On socket drain: set `paused = false`, call `snd_pcm_pause(0)`
    - _Requirements: 4.5_

  - [ ]* 5.4 Write unit tests for AlsaCaptureReader
    - Overrun recovery: `-EPIPE` triggers `snd_pcm_prepare` and logs warning
    - Format change detected and forwarded before next audio frame
    - _Requirements: 2.7, 2.8_

  - [ ]* 5.5 Write property test — Property 1: Valid FormatDescriptor acceptance
    - **Property 1: Valid FormatDescriptor acceptance**
    - **Validates: Requirements 2.2, 2.3, 2.4, 2.5, 2.6**
    - Generate random valid FormatDescriptors; assert no error and frame forwarded
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 1: Valid FormatDescriptor acceptance`

  - [ ]* 5.6 Write property test — Property 4: Audio bytes unmodified
    - **Property 4: Audio bytes are forwarded unmodified**
    - **Validates: Requirements 4.1, 4.3**
    - Generate random byte buffers; assert bytes written to NAA6Client equal received bytes
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 4: Audio bytes are forwarded unmodified`

  - [ ]* 5.7 Write property test — Property 6: Format change message precedes subsequent audio
    - **Property 6: Format change message precedes subsequent audio**
    - **Validates: Requirements 2.7, 4.2**
    - Generate random sequences of format changes interleaved with audio frames; assert format-change message precedes first frame of each new format
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 6: Format change message precedes subsequent audio`

  - [ ]* 5.8 Write property test — Property 8: Backpressure propagates to ALSA capture
    - **Property 8: Backpressure propagates from NAA6 socket to ALSA capture**
    - **Validates: Requirements 4.5**
    - Simulate `send()` returning EAGAIN; assert ALSA capture is paused and resumes only after socket drain
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 8: Backpressure propagates from NAA6 socket to ALSA capture`

  - [ ]* 5.9 Write property test — Property 16: Malformed frames do not terminate the session
    - **Property 16: Malformed ALSA frames do not terminate the session**
    - **Validates: Requirements 2.8**
    - Generate sequences mixing valid and malformed/error frames; assert bridge remains running and valid frames are forwarded
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 16: Malformed ALSA frames do not terminate the session`

- [ ] 6. Implement ShutdownHandler (audio-bridge)
  - [ ] 6.1 Implement ShutdownHandler
    - Register `SIGTERM` and `SIGINT` handlers
    - On signal: set 5-second hard-kill alarm, call `naa6_send_termination()` then `naa6_disconnect()`, close ALSA device, exit(0)
    - Log each shutdown step; log but do not block on errors during shutdown
    - _Requirements: 8.1, 8.2, 8.3_

  - [ ]* 6.2 Write unit tests for ShutdownHandler
    - Force-exit with non-zero code when 5-second timeout exceeded (Req 8.3)
    - _Requirements: 8.3_

  - [ ]* 6.3 Write property test — Property 17: Shutdown completes within 5 seconds
    - **Property 17: Shutdown completes within 5 seconds**
    - **Validates: Requirements 8.2, 8.3**
    - Simulate slow shutdown components with random delays; assert exit within 5000 ms or force-exit non-zero
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 17: Shutdown completes within 5 seconds`

- [ ] 7. Checkpoint — Ensure all audio-bridge tests pass
  - Ensure all tests pass; ask the user if questions arise.

---

### Part C: roon-metadata-extension (Node.js)

- [ ] 8. Initialise roon-metadata-extension project
  - Create `metadata-extension/` directory
  - Create `package.json` with `type: "module"`, dependencies (`node-roon-api`, `node-roon-api-transport`, `pino`) and dev dependencies (`vitest`, `fast-check`, `typescript`, `@types/node`)
  - Create `tsconfig.json` targeting Node 20, `moduleResolution: "bundler"`, `strict: true`
  - Create `vitest.config.ts` with `globals: true`
  - Create `src/types.ts` defining `Config`, `TrackMetadata` interfaces
  - _Requirements: 6.2_

- [ ] 9. Implement ConfigReader (metadata-extension)
  - [ ] 9.1 Implement `src/ConfigReader.ts`
    - Read `/etc/roon-naa6-bridge/config.json`, fall back to `./config.json`; if absent, use defaults and log a warning
    - Validate `hqplayerPort` ∈ [1, 65535] and `reconnectBackoff` > 0; on invalid value log error and `process.exit(1)`
    - Warn on unknown keys; return a frozen `Config` object
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

  - [ ]* 9.2 Write unit tests for ConfigReader
    - Missing file uses defaults (Req 6.3)
    - Invalid `hqplayerPort` exits non-zero (Req 6.5)
    - Unknown key produces warning and startup continues (Req 6.4)
    - _Requirements: 6.3, 6.4, 6.5_

- [ ] 10. Implement Logger
  - [ ] 10.1 Implement `src/Logger.ts`
    - Thin wrapper around `pino`; accept `logLevel` from `Config`; export a singleton factory
    - _Requirements: 7.1, 7.3_

  - [ ]* 10.2 Write property test — Property 12: DEBUG log level gates verbose output
    - **Property 12: DEBUG log level gates verbose output**
    - **Validates: Requirements 7.3, 7.4**
    - For INFO level assert no DEBUG entries; for DEBUG level assert FormatDescriptor change entries present
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 12: DEBUG log level gates verbose output`

- [ ] 11. Implement RoonExtension
  - [ ] 11.1 Implement `src/RoonExtension.ts`
    - Wrap `node-roon-api` and `node-roon-api-transport`
    - Discover Roon Core via mDNS (SDK-managed), pair and register extension
    - Subscribe to zone/transport state changes; emit `metadataChanged` events
    - Emit `core_paired` / `core_unpaired` events
    - On `core_unpaired`, schedule reconnect after `Config.reconnectBackoff` ms
    - On graceful shutdown, call `roon.stop()` to deregister
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5_

  - [ ]* 11.2 Write unit tests for RoonExtension
    - Pairing initiated on core discovery (Req 1.3)
    - Deregistration called on shutdown (Req 1.5)
    - _Requirements: 1.3, 1.5_

  - [ ]* 11.3 Write property test — Property 2: Output name matches configuration
    - **Property 2: Roon Output name matches configuration**
    - **Validates: Requirements 1.2**
    - Generate random non-empty `outputName` strings; assert registered display name equals config value
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 2: Roon Output name matches configuration`

  - [ ]* 11.4 Write property test — Property 3: Reconnect within configured backoff
    - **Property 3: Reconnect interval is within configured backoff**
    - **Validates: Requirements 1.4**
    - Generate random `reconnectBackoff` values ≤ 30000 ms; simulate disconnection; assert reconnect scheduled ≤ backoff ms
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 3: Reconnect interval is within configured backoff`

- [ ] 12. Implement MetadataForwarder
  - [ ] 12.1 Implement `src/MetadataForwarder.ts`
    - Listen for `metadataChanged` events from `RoonExtension`
    - Encode title, artist, album as UTF-8 buffers
    - Validate cover art dimensions ≤ 1024×1024; omit if unavailable without error
    - Send NAA 6 metadata message (via IPC socket to naa6-audio-bridge, or directly to HQPlayer's NAA 6 port) within 500 ms of receiving the event
    - If no metadata available, log at DEBUG and skip transmission
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7_

  - [ ]* 12.2 Write unit tests for MetadataForwarder
    - No message sent when metadata absent (Req 9.5)
    - Cover art absent: remaining fields transmitted, no error (Req 9.4)
    - _Requirements: 9.4, 9.5_

  - [ ]* 12.3 Write property test — Property 13: Metadata extraction round-trip
    - **Property 13: Metadata extraction round-trip**
    - **Validates: Requirements 9.1**
    - Generate random metadata payloads; assert extracted `TrackMetadata` fields equal source fields
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 13: Metadata extraction round-trip`

  - [ ]* 12.4 Write property test — Property 14: Text metadata is valid UTF-8
    - **Property 14: Text metadata is valid UTF-8**
    - **Validates: Requirements 9.6**
    - Generate random Unicode strings; assert `decode(encode(s)) === s`
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 14: Text metadata is valid UTF-8`

  - [ ]* 12.5 Write property test — Property 15: Missing cover art omits only cover art
    - **Property 15: Missing cover art does not suppress other metadata fields**
    - **Validates: Requirements 9.4**
    - Generate metadata without cover art; assert NAA 6 message contains title/artist/album and no coverArt field
    - `numRuns: 100`; comment: `// Feature: roon-naa6-bridge, Property 15: Missing cover art does not suppress other metadata fields`

- [ ] 13. Implement ShutdownHandler (metadata-extension)
  - [ ] 13.1 Implement `src/ShutdownHandler.ts`
    - Register `process.on('SIGTERM')` and `process.on('SIGINT')`
    - On signal: set 5-second hard-kill timer (`process.exit(1)`), call `RoonExtension.deregister()`, clear timer and call `process.exit(0)`
    - Log each shutdown step; log but do not block on errors during shutdown
    - _Requirements: 8.1, 8.2, 8.3_

  - [ ]* 13.2 Write unit tests for ShutdownHandler
    - Force-exit with code 1 when 5-second timeout exceeded (Req 8.3)
    - _Requirements: 8.3_

- [ ] 14. Wire components together in entry point
  - [ ] 14.1 Implement `src/index.ts`
    - Instantiate `ConfigReader`, `Logger`, `RoonExtension`, `MetadataForwarder`, `ShutdownHandler` in dependency order
    - Connect `RoonExtension` `metadataChanged` events → `MetadataForwarder`
    - Log startup event (Req 7.1)
    - _Requirements: 1.1, 7.1_

- [ ] 15. Checkpoint — Ensure all metadata-extension tests pass
  - Ensure all tests pass; ask the user if questions arise.

---

### Part D: Deployment

- [ ] 16. Create systemd service units
  - Create `naa6-audio-bridge.service` with `Type=simple`, `Restart=on-failure`, `KillMode=mixed`, `TimeoutStopSec=10`; `ExecStart` points to the compiled binary
  - Create `roon-metadata-extension.service` with `Type=simple`, `Restart=on-failure`, `KillMode=mixed`, `TimeoutStopSec=10`; `ExecStart` points to `node /path/to/dist/index.js`
  - Create `/etc/roon-naa6-bridge/config.json` with documented defaults
  - Create `README.md` with installation instructions: ALSA loopback setup, Roon Bridge ALSA output configuration, service installation, config reference
  - _Requirements: 10.3, 10.4, 10.5_

- [ ] 17. Final checkpoint — Ensure all tests pass
  - Ensure all tests pass; ask the user if questions arise.

---

## Notes

- Tasks marked with `*` are optional and can be skipped for a faster MVP
- Each task references specific requirements for traceability
- Property tests in naa6-audio-bridge use `proptest` (Rust) or `theft` (C) with minimum 100 iterations
- Property tests in roon-metadata-extension use `fast-check` with `numRuns: 100`
- Each property test includes a comment: `// Feature: roon-naa6-bridge, Property N: <title>`
- Unit tests in naa6-audio-bridge use `#[test]` (Rust) or `cmocka` (C)
- Unit tests in roon-metadata-extension use `vitest`
