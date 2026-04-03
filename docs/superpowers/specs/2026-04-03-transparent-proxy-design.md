# Transparent TCP Proxy — Design Spec

**Date:** 2026-04-03  
**Status:** Approved

## Goal

Validate the assumption that Roon's `secure_uri` (in `PlaylistAdd` messages) can be forwarded
transparently to HQPlayer without any modification. If audio plays through to the Eversolo T8, the
simpler proxy architecture is viable and the current ALSA loopback complexity can be eliminated.

## Background

The current `audio-bridge` was built on an untested assumption that Roon's `secure_uri` cannot be
proxied to HQPlayer. This proxy tests that assumption directly before committing to a full rewrite.

## Approach

A single Python script (`proxy/transparent_proxy.py`) that:

1. Binds on `0.0.0.0:4321` and accepts one Roon connection at a time
2. Opens a corresponding outbound connection to HQPlayer at `192.168.30.212:4321`
3. Spawns two threads forwarding bytes bidirectionally
4. Logs each XML line (newline-terminated) to stdout with timestamp and direction label
5. On disconnect (either side), closes both connections and loops back to accept the next connection

Roon is reconfigured to point at this laptop's IP instead of `192.168.30.212`.

## File layout

```
proxy/
  transparent_proxy.py   # the entire implementation
```

## Configuration (top of file)

```python
HQPLAYER_HOST = "192.168.30.212"
HQPLAYER_PORT = 4321
LISTEN_PORT   = 4321
```

## Forwarding logic

Each thread reads from its source socket line-by-line (`readline()`). For each line:
- Log to stdout: `[timestamp] [label] <line>`
- Write raw bytes to the destination socket

If a read returns bytes without a newline (e.g. non-line-framed data), log and forward immediately
without waiting. If either socket closes or errors, both threads exit.

## Logging format

```
2026-04-03 10:12:34 [Roon→HQP] <?xml version="1.0" encoding="utf-8"?><GetInfo/>
2026-04-03 10:12:34 [HQP→Roon] <?xml version="1.0" encoding="utf-8"?><GetInfo engine="5.35.6" .../>
```

## What this does NOT proxy

Audio data. Roon opens a separate HTTP/TCP connection directly to HQPlayer for the audio stream
(pointed to by `secure_uri`). That connection bypasses this proxy entirely — which is correct,
since we are only testing the control channel.

## Success criteria

- Audio plays on the Eversolo T8 with Roon pointing at the proxy
- XML messages are visible in the terminal log
- No modification to the XML content is required

## Next step (if validated)

Design a production Rust proxy that also injects metadata XML messages from
`roon-metadata-extension` into the HQPlayer-facing stream.
