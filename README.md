# RooNAA6

Transparent TCP proxy that sits between HQPlayer and an NAA v6 endpoint (e.g. Eversolo T8 DAC), injecting Roon track metadata and cover art into the audio stream so the DAC displays what's playing.

## How it works

```
Roon Core ──> HQPlayer ──> RooNAA6 proxy ──> NAA endpoint (T8)
                              │
                              └── Roon Core WebSocket (metadata)
```

- HQPlayer connects to the proxy (thinking it's the NAA endpoint)
- Proxy forwards all traffic bidirectionally to the real NAA endpoint
- Proxy connects to Roon Core via WebSocket to get track metadata (title, artist, album, cover art)
- On the first audio frame after playback starts, proxy injects metadata + cover art into the NAA v6 binary stream
- Subsequent metadata from HQPlayer is stripped to keep the Roon metadata visible
- Periodic refresh every ~30s prevents the DAC from reverting

Handles PCM (up to 768kHz) and DSD (up to DSD512). Gapless track changes are detected via Roon zone subscription and metadata is re-injected at track boundaries.

## Network requirements

The proxy and HQPlayer must run on **different machines** because the NAA control port (43210) is fixed and cannot be changed. The proxy listens on 43210 and forwards to HQPlayer's NAA endpoint on another IP.

The proxy also handles NAA multicast discovery (224.0.0.199, 239.192.0.199) so HQPlayer sees it in the device dropdown.

## Configuration

Edit the constants in `src/main.rs`:

| Constant | Description | Example |
|----------|-------------|---------|
| `NAA_HOST` | IP of the real NAA endpoint (e.g. T8) | `192.168.30.109` |
| `NAA_PORT` | NAA port (always 43210) | `43210` |
| `MCAST_IFACE` | Interface IP for multicast discovery | `192.168.30.212` |

Edit the constants in `src/roon.rs`:

| Constant | Description | Example |
|----------|-------------|---------|
| `ROON_HOST` | IP of the Roon Core | `192.168.30.23` |
| `ROON_PORT` | Roon Core HTTP/WebSocket port | `9330` |
| `ZONE_NAME` | Roon zone name to monitor | `Einstein` |

## Build and deploy

```bash
# Build
cargo build --release

# Copy to the proxy host
scp target/release/RooNAA6 user@proxy-host:/usr/local/bin/

# Run (logs to stderr)
RooNAA6
```

## First run — Roon pairing

On first launch, the proxy registers as a Roon extension called "RooNAA6 Metadata". You need to authorise it in Roon:

1. Open Roon → Settings → Extensions
2. Find "RooNAA6 Metadata" and click Enable
3. The proxy saves the auth token to `/tmp/roon_token.json` for subsequent runs

## Running as a systemd service

Create `/etc/systemd/system/roonaa6.service`:

```ini
[Unit]
Description=RooNAA6 NAA metadata proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/RooNAA6
Restart=always
RestartSec=5
User=your-user

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now roonaa6
journalctl -u roonaa6 -f
```

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
