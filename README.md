# roon-naa6-bridge

Bridges Roon audio output to HQPlayer via the NAA 6 protocol.

Two systemd services work together:

- **naa6-audio-bridge** (Rust) — reads PCM/DSD frames from an ALSA loopback device and streams them to HQPlayer over TCP using the NAA 6 protocol.
- **roon-metadata-extension** (Node.js) — connects to Roon Core as an extension and forwards track metadata (title, artist, album, cover art) to HQPlayer via NAA 6 metadata messages.

---

## Requirements

- Ubuntu 24.04 LTS
- Roon Bridge installed and running
- HQPlayer running and reachable on the network
- Node.js 24 LTS
- Rust toolchain (for building from source)

---

## 1. ALSA Loopback Setup

The bridge uses the `snd-aloop` kernel module to create a virtual loopback sound card.

**Load the module at boot:**

```bash
sudo cp alsa-loopback/etc/modules-load.d/snd-aloop.conf /etc/modules-load.d/
sudo cp alsa-loopback/etc/modprobe.d/snd-aloop.conf     /etc/modprobe.d/
```

**Load it immediately (without rebooting):**

```bash
sudo modprobe snd-aloop enable=1 index=2
```

**Verify it loaded:**

```bash
aplay -l | grep Loopback
```

You should see two loopback devices: `hw:2,0` (playback) and `hw:2,1` (capture).

---

## 2. Configure Roon Bridge Output

In Roon → Settings → Audio, enable the ALSA device that corresponds to the loopback playback side:

- Device: `snd_aloop` / `Loopback, Loopback PCM` (card 2, device 0)
- Set it as the output zone you want to bridge to HQPlayer.

The bridge captures from `hw:Loopback,1,0` (the capture side of the loopback pair) by default.

---

## 3. Build

**naa6-audio-bridge (Rust):**

```bash
cd audio-bridge
cargo build --release
sudo cp target/release/naa6-audio-bridge /usr/local/bin/
```

**roon-metadata-extension (Node.js):**

```bash
cd metadata-extension
npm install --omit=dev
npm run build
sudo mkdir -p /usr/local/lib/roon-metadata-extension
sudo cp -r dist package.json node_modules /usr/local/lib/roon-metadata-extension/
```

---

## 4. Configuration

Copy the example config and edit it:

```bash
sudo mkdir -p /etc/roon-naa6-bridge
sudo cp config.json.example /etc/roon-naa6-bridge/config.json
sudo nano /etc/roon-naa6-bridge/config.json
```

### Config Reference

| Key | Default | Description |
|-----|---------|-------------|
| `outputName` | `"HQPlayer via NAA6"` | Roon Output display name |
| `alsaDevice` | `"hw:Loopback,1,0"` | ALSA capture device (loopback capture side) |
| `hqplayerHost` | `"127.0.0.1"` | HQPlayer hostname or IP address |
| `hqplayerPort` | `10700` | HQPlayer NAA 6 TCP port |
| `reconnectBackoff` | `5000` | Reconnect delay in milliseconds |
| `logLevel` | `"info"` | Log verbosity: `"info"` or `"debug"` |
| `ipcSocket` | `"/run/roon-naa6-bridge/meta.sock"` | Unix socket path for metadata IPC |

---

## 5. Service Installation

**Create a dedicated service user:**

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin --groups audio roon-bridge
```

**Install and enable the services:**

```bash
sudo cp naa6-audio-bridge.service        /etc/systemd/system/
sudo cp roon-metadata-extension.service  /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable --now naa6-audio-bridge
sudo systemctl enable --now roon-metadata-extension
```

**Check status:**

```bash
sudo systemctl status naa6-audio-bridge
sudo systemctl status roon-metadata-extension
```

**View logs:**

```bash
journalctl -u naa6-audio-bridge -f
journalctl -u roon-metadata-extension -f
```

---

## 6. Updating Node.js Path

If Node.js is installed via nvm rather than system packages, update the `ExecStart` path in `roon-metadata-extension.service`:

```bash
which node   # e.g. /home/youruser/.nvm/versions/node/v24.14.1/bin/node
```

Then edit the service file accordingly and run `sudo systemctl daemon-reload`.

---

## Troubleshooting

**No audio forwarded:**
- Check `aplay -l` to confirm the loopback device exists.
- Confirm Roon Bridge is outputting to the loopback playback device.
- Check `journalctl -u naa6-audio-bridge` for ALSA or connection errors.

**Metadata not appearing in HQPlayer:**
- Check `journalctl -u roon-metadata-extension` for Roon pairing errors.
- Ensure the Roon Core has authorised the extension (check Roon → Settings → Extensions).

**HQPlayer connection refused:**
- Verify `hqplayerHost` and `hqplayerPort` in the config match HQPlayer's NAA settings.
- Confirm HQPlayer is running and NAA is enabled.
