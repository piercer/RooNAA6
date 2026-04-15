# RooNAA6

Transparent TCP proxy that sits between HQPlayer and an NAA v6 endpoint (e.g. Eversolo T8 DAC), injecting Roon track metadata and cover art into the audio stream so the DAC displays what's playing.

## Disclaimer

This software is an independent, community-developed project and is **not affiliated with, endorsed by, or supported by Signalyst** (the developer of HQPlayer) in any way. RooNAA6 is not an official Signalyst product or component.

**By using this software, you acknowledge and agree to the following:**

1. **No Signalyst support obligation.** Signalyst bears no responsibility for any issues arising from the use of this proxy. You must **not** contact Signalyst support for problems encountered while RooNAA6 is in use.

2. **Troubleshooting procedure.** If you experience any issues with HQPlayer or your NAA endpoint while using RooNAA6, you must **first** disable the proxy and connect HQPlayer directly to your NAA endpoint. Only if the issue **persists without the proxy in the signal chain** should you seek support from Signalyst.

3. **Use at your own risk.** This software is provided "as is", without warranty of any kind, express or implied. The authors accept no liability for any damage, data loss, or other adverse effects resulting from its use.

4. **Not a Signalyst product.** All trademarks, including "HQPlayer" and "NAA", are the property of their respective owners. Their use here is purely for descriptive and interoperability purposes.

---

## Why this exists

When using Roon with HQPlayer as an audio output, the signal chain is:

```
Roon Core → HQPlayer → NAA endpoint
```

HQPlayer communicates with NAA endpoints using the NAA v6 protocol, which supports rich metadata (track title, artist, album, cover art). However, Roon only sends audio to HQPlayer — it doesn't pass track metadata through HQPlayer's pipeline. The result: your DAC displays nothing useful (typically just "Roon" or silence) even though Roon knows exactly what's playing.

RooNAA6 solves this by sitting between HQPlayer and the NAA endpoint, independently fetching metadata from Roon Core via its WebSocket API, and injecting it into the NAA v6 audio stream so the DAC displays full track information.

## How it works

```
Roon Core ──> HQPlayer ──> RooNAA6 proxy ──> NAA endpoint
                              │
                              └── Roon Core WebSocket (metadata)
```

- HQPlayer connects to the proxy (thinking it's the NAA endpoint)
- Proxy forwards all traffic bidirectionally to the real NAA endpoint
- Proxy connects to Roon Core via WebSocket to get track metadata (title, artist, album, cover art)
- On the first audio frame after playback starts, proxy injects metadata + cover art into the NAA v6 binary stream
- Subsequent metadata from HQPlayer is stripped to keep the Roon metadata visible
- Periodic refresh every ~30s prevents the endpoint from reverting

Handles PCM and DSD. Gapless track changes are detected via Roon zone subscription and metadata is re-injected at track boundaries.

## Network requirements

The proxy typically runs on the **same machine** as HQPlayer. Both the proxy and the NAA endpoint use port 43210, so the proxy binds locally and forwards to the real NAA endpoint on the network. The proxy can also run on a separate machine if needed.

The proxy also handles NAA multicast discovery (224.0.0.199, 239.192.0.199) so HQPlayer sees it in the device dropdown.

## Configuration

Copy `config.toml.example` to `config.toml` and edit:

```toml
[naa]
host = "192.168.30.109"         # IP of the real NAA endpoint (e.g. T8)
mcast_iface = "192.168.30.212"  # Interface IP for multicast discovery

[roon]
host = "192.168.30.23"          # IP of the Roon Core
# port = 9330                   # Roon HTTP/WebSocket port (default: 9330)
zone = "Einstein"               # Roon zone name to monitor
# token_file = "/etc/roonaa6/roon_token.json"  # Auth token persistence (default)
```

## Install from .deb package

Download the `.deb` from the [releases page](https://github.com/piercer/RooNAA6/releases) and install:

```bash
sudo dpkg -i roonaa6_1.0.0-1_amd64.deb
```

This installs:
- `/usr/bin/RooNAA6` — the proxy binary
- `/etc/roonaa6/config.toml` — configuration (edit this)
- `/lib/systemd/system/roonaa6.service` — systemd service

Edit the config, then start:

```bash
sudo nano /etc/roonaa6/config.toml
sudo systemctl enable --now roonaa6
journalctl -u roonaa6 -f
```

The config file is preserved across package upgrades.

## Build from source

Requires a Rust toolchain.

```bash
# Build binary
cargo build --release

# Build .deb package (requires cargo-deb: cargo install cargo-deb)
cargo deb

# Or deploy manually
scp target/release/RooNAA6 config.toml user@proxy-host:~
ssh user@proxy-host './RooNAA6'                           # reads config.toml from cwd
ssh user@proxy-host './RooNAA6 /path/to/config.toml'      # or specify a path
```

## First run — Roon pairing

On first launch, the proxy registers as a Roon extension called "RooNAA6 Metadata". You need to authorise it in Roon:

1. Open Roon → Settings → Extensions
2. Find "RooNAA6 Metadata" and click Enable
3. The proxy saves the auth token to `/etc/roonaa6/roon_token.json` for subsequent runs (configurable via `token_file`)

## HQPlayer setup

1. In HQPlayer, select the proxy from the NAA device dropdown (shows as "RooNAA6 Proxy")
2. Point Roon at HQPlayer as normal
3. Play music — metadata and cover art should appear on the DAC

## Known issues

### T8 metadata revert

The Eversolo T8 sometimes enters a state where injected metadata appears briefly then reverts to showing "Roon". This is a T8 firmware issue, not a proxy bug — it affects the Python prototype identically.

Known mitigations:
- Stop playback, wait a few seconds, play again
- Power cycle the T8 (full power off, not just reboot)
- Leave the T8 idle for a while

The issue appears to be triggered by rapid proxy restarts or repeated connect/disconnect cycles.

### Playhead / progress bar

The DAC does not display a playhead or track progress. The NAA v6 protocol carries a position field in each frame, but the proxy does not currently interpret or inject position data. This is a known limitation.

### DSD output mode

HQPlayer's DSD output mode must be configured in HQPlayer settings. The proxy does not affect format negotiation — it passes all XML control messages through unchanged.

## Architecture

| File | Purpose |
|------|---------|
| `src/main.rs` | Entry point, TCP listener, thread spawning |
| `src/proxy.rs` | Frame-level state machine (INJECT/STRIP/REFRESH/GAPLESS/PASSTHROUGH) |
| `src/frame.rs` | NAA v6 header parsing, metadata section builder |
| `src/discovery.rs` | UDP multicast discovery responder |
| `src/roon.rs` | Roon Core WebSocket client (MOO/1 protocol) |
| `src/metadata.rs` | Thread-safe shared metadata store |
