# Project Context

## What this project is
Bridges Roon audio output to HQPlayer so that track metadata (title, artist, album, cover art) displays on an Eversolo T8 DAC. The T8 supports NAA protocol v6 which carries metadata, but Roon only speaks NAA v5 to HQPlayer, so metadata doesn't flow through.

## Current state (as of 2026-04-03)
There is a working proof-of-concept with two components:
- **audio-bridge** (Rust) — emulates HQPlayer's control API toward Roon, captures ALSA loopback audio, serves it via HTTP to HQPlayer, relays control messages
- **metadata-extension** (Node.js) — connects to Roon Core, captures track metadata, forwards it over an IPC Unix socket

The architecture is: Roon -> ALSA loopback -> audio-bridge -> HTTP PCM -> HQPlayer -> NAA -> Eversolo T8

## Critical insight: the current architecture may be unnecessarily complex
The ALSA loopback approach was built on an **untested assumption** (from a Kiro AI session) that Roon's `secure_uri` in PlaylistAdd messages cannot be passed through a proxy to HQPlayer. **Nobody actually tested this.** Kiro assumed it wouldn't work and went straight to the complex loopback approach.

### Known problems with the ALSA loopback approach
- Loopback device closes when no audio is flowing — HQPlayer gets no sound
- Difficult to handle different on-demand bit rates
- Complex multi-component architecture (control server, HTTP server, ALSA capture, control client)

### Proposed simpler alternative: transparent TCP proxy
A thin TCP proxy on port 4321 that:
1. Roon connects to the proxy (thinking it's HQPlayer)
2. Proxy connects to real HQPlayer and forwards ALL messages bidirectionally
3. Proxy watches for metadata from Roon (via metadata-extension) and injects metadata XML messages into the HQPlayer-facing connection

No ALSA loopback, no HTTP server, no HQPlayer emulation. Audio flows directly from Roon to HQPlayer via the normal secure_uri mechanism.

## Proxy assumption: VALIDATED (2026-04-03)
A minimal transparent TCP proxy (`proxy/transparent_proxy.py`) was built and tested. Roon pointed at the proxy on the 192.168.30 subnet, proxy forwarded all traffic to HQPlayer at 192.168.30.212:4321. **Audio played perfectly on the T8**, identical to direct HQPlayer connection. Roon's `secure_uri` passes through a proxy without modification.

**The simple architecture is viable. The ALSA loopback approach should be replaced.**

## Next step: build the production Rust proxy

### Port 4321 challenge
HQPlayer's NAA control port (4321) **cannot be changed**. So the proxy and HQPlayer must run on different machines (proxy on one machine listening on 4321, forwarding to HQPlayer's real IP:4321).

### Metadata delivery
HTTP header metadata (X-HQPlayer-Raw-Title, etc.) was **proven working** on the T8 before a Kiro session removed it. With the proxy approach, metadata would instead be injected as XML messages on the control connection.

## Owner
Conrad — audiophile with Roon Server -> HQPlayer (host "einstein", 192.168.30.212) -> Eversolo T8 setup. Multiple subnets for HiFi equipment.