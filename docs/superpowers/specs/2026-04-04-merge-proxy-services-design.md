# Merge Proxy Services Design

## Goal

Combine the two separate Python processes (naa_proxy.py + roon_metadata.py) into a single process, preserving identical behaviour to the baseline at commit 6ab0a63.

## Baseline

Commit `6ab0a63` — two-process proxy with working metadata + cover art on T8.

- `proxy/naa_proxy.py` (228 lines) — NAA TCP proxy + UDP discovery. Reads metadata from `/tmp/roon_now_playing.json` and cover art from `/tmp/roon_cover.jpg`.
- `proxy/roon_metadata.py` (199 lines) — Roon Core WebSocket listener. Writes metadata JSON and cover art JPEG to disk.

## Approach

Copy `RoonMetadata` class verbatim from roon_metadata.py into naa_proxy.py. Run it as a daemon thread with the existing reconnect loop. The file-based interface between metadata writer and proxy reader stays unchanged.

## What changes

1. **Imports added to naa_proxy.py:** `sys`, `time`, `urllib.request`, `websocket` (with try/except fallback)
2. **Constants added:** `ROON_HOST`, `ROON_PORT`, `TOKEN_FILE`
3. **`RoonMetadata` class** copied verbatim from roon_metadata.py, including `_download_cover`
4. **`roon_listener_thread()` function** — wraps `RoonMetadata.run()` with reconnect loop (same as roon_metadata.py's `__main__`)
5. **`__main__` block** — starts `roon_listener_thread` as daemon thread before the existing discovery thread and TCP listener

## What does NOT change

- `get_roon_metadata()` — reads from `/tmp/roon_now_playing.json`
- `load_cover_art()` — reads from `/tmp/roon_cover.jpg`
- `replace_metadata_section()` — same-size replacement, untouched
- `forward_hqp_to_t8()` — frame header patching + metadata injection, untouched
- `forward_t8_to_hqp()` — passthrough, untouched
- `discovery_responder()` — UDP multicast, untouched
- `patch_frame_header()` — untouched

## No shared mutable state

The Roon thread writes to disk. The proxy thread reads from disk. No threading locks, no shared dicts, no in-memory communication. This is deliberately identical to the two-process architecture, just in one OS process.

## Testing

1. Deploy merged naa_proxy.py to einstein (no separate roon_metadata.py needed)
2. Kill any running roon_metadata.py process
3. Play a track — verify title, artist, album, cover art on T8
4. Switch track — verify metadata updates
5. Switch album — verify new cover art appears
6. Compare logs to baseline to confirm identical behaviour

## Success criteria

- Single process to start and manage
- Identical T8 display to baseline (metadata + cover art)
- No regressions on track switching
