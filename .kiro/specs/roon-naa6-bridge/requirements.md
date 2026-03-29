# Requirements Document

## Introduction

The Roon-NAA6 Bridge is a software system that bridges Roon's audio output to HQPlayer for DSP
processing. It consists of two processes running on a Linux host (the "Dirac VM"):

- **naa6-audio-bridge** (Rust): implements the HQPlayer control API on TCP port 4321 (so Roon can
  connect to it as if it were HQPlayer), reads PCM audio from an ALSA loopback capture device,
  serves raw PCM audio via HTTP on port 30001, and relays control messages to the real HQPlayer.
- **roon-metadata-extension** (Node.js): connects to Roon Core as a Roon Extension, subscribes to
  zone/transport state changes, and forwards track metadata to naa6-audio-bridge via a Unix IPC
  socket.

The signal flow is:

```
Roon Core → (RAAT) → Roon Bridge → ALSA loopback playback (hw:Loopback,0,0)
                                           ↓
                                    naa6-audio-bridge (Dirac VM)
                                    - reads PCM from ALSA loopback capture (hw:Loopback,1,0)
                                    - implements HQPlayer control API on port 4321 (TCP)
                                    - serves raw PCM audio via HTTP on port 30001
                                    - tells HQPlayer where to fetch audio via PlaylistAdd
                                    - forwards Roon metadata via HQPlayer control API
                                           ↓
                                    HQPlayer (einstein, 192.168.30.212)
                                    - upsamples/DSPs the audio
                                    - sends to downstream NAA renderer → DAC (Eversolo T8)
```

## Glossary

- **Bridge**: The roon-naa6-bridge software system described in this document
- **naa6-audio-bridge**: The Rust process that implements the HQPlayer control API, HTTP audio
  server, and ALSA capture
- **roon-metadata-extension**: The Node.js process that connects to Roon Core and forwards metadata
- **Roon_Core**: The Roon server software that manages the music library and distributes audio
  streams
- **RaaT**: Roon Advanced Audio Transport — the proprietary protocol used by Roon Core to stream
  audio to endpoints
- **Roon_Extension**: A software component that integrates with Roon Core via the Roon Extension API
- **Roon_Output**: A virtual audio output zone registered by the Bridge with the Roon Core
- **HQPlayer_Control_API**: The XML-over-TCP protocol on port 4321 that HQPlayer exposes and that
  the Bridge emulates toward Roon, and also speaks toward the real HQPlayer
- **XML_Message**: A newline-terminated UTF-8 string containing a well-formed XML document exchanged
  over the HQPlayer_Control_API
- **Control_Server**: The naa6-audio-bridge component that listens on TCP port 4321 and accepts
  Roon's connection, acting as HQPlayer
- **Control_Client**: The naa6-audio-bridge component that connects outbound to HQPlayer's TCP port
  4321, acting as Roon
- **HTTP_Audio_Server**: The naa6-audio-bridge component that listens on TCP port 30001 and serves
  raw PCM audio to HQPlayer via HTTP
- **Audio_Stream**: A continuous sequence of raw PCM audio samples served by the HTTP_Audio_Server
- **PlaylistAdd**: An XML_Message that carries a URI pointing to an audio stream; Roon sends it to
  the Control_Server, and the Bridge sends its own PlaylistAdd to HQPlayer pointing at the
  HTTP_Audio_Server
- **GetInfo**: An XML_Message used by Roon to query the engine identity; the Bridge responds with
  HQPlayer Embedded identity fields
- **SessionAuthentication**: An XML_Message used by Roon to authenticate; the Bridge accepts all
  authentication without verifying the cryptographic signature
- **Status**: An XML_Message used to report or subscribe to playback state
- **VolumeRange**: An XML_Message used to query the volume control range
- **State**: An XML_Message used to query the current playback state
- **PlaylistClear**: An XML_Message used to clear the current playlist
- **Stop**: An XML_Message used to stop playback
- **ALSA_Loopback**: The Linux `snd-aloop` virtual sound card used to route audio from Roon Bridge
  to naa6-audio-bridge
- **IPC_Socket**: A Unix domain socket used for inter-process communication between
  roon-metadata-extension and naa6-audio-bridge
- **Track_Metadata**: Descriptive information associated with the currently playing track, including
  track title, artist name, and album name
- **Sample_Rate**: The number of audio samples per second, expressed in Hz (e.g. 44100, 96000)
- **Bit_Depth**: The number of bits per audio sample (e.g. 16, 24, 32)
- **Channel_Count**: The number of audio channels in a stream (e.g. 2 for stereo)
- **Format_Descriptor**: A data structure describing the sample rate, bit depth, and channel count
  of an Audio_Stream, derived from the active Roon stream

## Requirements

### Requirement 1: Roon Endpoint Registration

**User Story:** As a Roon user, I want the Bridge to appear as a selectable audio output in Roon,
so that I can route playback through HQPlayer via the Bridge.

#### Acceptance Criteria

1. THE roon-metadata-extension SHALL register itself with the Roon_Core as a Roon_Extension using
   the Roon Extension API.
2. THE roon-metadata-extension SHALL advertise a Roon_Output zone with a configurable display name.
3. WHEN the Roon_Core is discovered on the network, THE roon-metadata-extension SHALL initiate the
   pairing and registration sequence automatically.
4. IF the connection to the Roon_Core is lost, THEN THE roon-metadata-extension SHALL attempt to
   reconnect at intervals defined by the configured reconnectBackoff value.
5. WHEN the roon-metadata-extension is shut down gracefully, THE roon-metadata-extension SHALL
   deregister the Roon_Output from the Roon_Core.

---

### Requirement 2: ALSA Audio Capture

**User Story:** As a user, I want the Bridge to correctly capture the audio stream from the ALSA
loopback device, so that audio data is not lost or corrupted before forwarding to HQPlayer.

#### Acceptance Criteria

1. THE naa6-audio-bridge SHALL open the ALSA loopback capture device specified in the configuration
   (default: `hw:Loopback,1,0`).
2. THE naa6-audio-bridge SHALL support PCM audio at sample rates of 44100, 48000, 88200, 96000,
   176400, and 192000 Hz.
3. THE naa6-audio-bridge SHALL support bit depths of 16, 24, and 32 bits per sample.
4. THE naa6-audio-bridge SHALL support stereo (2-channel) audio streams.
5. WHEN the ALSA capture device reports an overrun (`-EPIPE`), THE naa6-audio-bridge SHALL call
   `snd_pcm_prepare` to recover and log a warning without terminating the session.
6. IF the ALSA capture device cannot be opened at startup, THEN THE naa6-audio-bridge SHALL log a
   fatal error and exit with a non-zero exit code.

---

### Requirement 3: HQPlayer Control API — Control Server (Roon-facing)

**User Story:** As a user, I want the Bridge to emulate the HQPlayer control API so that Roon can
connect to it and control playback as if it were talking to HQPlayer directly.

#### Acceptance Criteria

1. THE Control_Server SHALL listen for incoming TCP connections on the port specified in the
   configuration (default: 4321).
2. WHEN Roon connects to the Control_Server, THE Control_Server SHALL accept the connection and
   begin processing XML_Messages.
3. THE Control_Server SHALL parse incoming XML_Messages by reading newline-terminated UTF-8 strings
   from the TCP connection.
4. WHEN Roon sends a GetInfo message, THE Control_Server SHALL respond with an XML_Message
   identifying the engine as `HQPlayerEmbedded` with `engine="5.35.6"`, `platform="Linux"`,
   `product="Signalyst HQPlayer Embedded"`, and `version="5"`.
5. WHEN Roon sends a SessionAuthentication message, THE Control_Server SHALL respond with an
   XML_Message containing `result="OK"` without verifying the cryptographic signature.
6. WHEN Roon sends a Stop message, THE Control_Server SHALL respond with
   `<Stop result="OK"/>`.
7. WHEN Roon sends a Status message with `subscribe="1"`, THE Control_Server SHALL respond with a
   Status XML_Message containing the current `active_bits`, `active_channels`, `active_rate`, and
   `state` fields.
8. WHEN Roon sends a VolumeRange message, THE Control_Server SHALL respond with
   `<VolumeRange adaptive="0" enabled="1" max="0.0" min="-60.0"/>`.
9. WHEN Roon sends a State message, THE Control_Server SHALL respond with a State XML_Message
   containing the current `active_mode` and `active_rate` fields.
10. WHEN Roon sends a PlaylistClear message, THE Control_Server SHALL respond with
    `<PlaylistClear result="OK"/>`.
11. WHEN Roon sends a PlaylistAdd message, THE Control_Server SHALL respond with
    `<PlaylistAdd result="OK"/>` and SHALL trigger the Bridge to send its own PlaylistAdd to
    HQPlayer pointing at the HTTP_Audio_Server stream URL.
12. WHEN Roon sends a VolumeRange or State message as a keepalive (approximately every 2 seconds),
    THE Control_Server SHALL respond with the current state within 1 second.
13. WHEN Roon disconnects, THE Control_Server SHALL close the connection and resume listening for a
    new connection.
14. THE Control_Server SHALL handle one Roon connection at a time.

---

### Requirement 4: HQPlayer Control API — Control Client (HQPlayer-facing)

**User Story:** As a user, I want the Bridge to connect to HQPlayer and send it a PlaylistAdd
pointing at the Bridge's HTTP audio stream, so that HQPlayer fetches and processes the audio.

#### Acceptance Criteria

1. WHEN the Control_Server receives a PlaylistAdd from Roon, THE Control_Client SHALL connect to
   HQPlayer on the host and port specified in the configuration (default: `192.168.30.212:4321`).
2. WHEN connected to HQPlayer, THE Control_Client SHALL send a PlaylistAdd XML_Message whose URI
   points to the HTTP_Audio_Server stream URL (e.g.
   `http://{bridge_ip}:{httpPort}/{token}/stream.raw`).
3. IF the connection to HQPlayer is lost, THEN THE Control_Client SHALL attempt to reconnect at
   intervals defined by the configured reconnectBackoff value.
4. THE Control_Client SHALL send all outgoing XML_Messages as newline-terminated UTF-8 strings
   beginning with `<?xml version="1.0" encoding="utf-8"?>`.

---

### Requirement 5: HTTP Audio Server

**User Story:** As a user, I want the Bridge to serve raw PCM audio over HTTP so that HQPlayer can
fetch it using its standard audio source mechanism.

#### Acceptance Criteria

1. THE HTTP_Audio_Server SHALL listen for incoming TCP connections on the port specified in the
   configuration (default: 30001).
2. WHEN HQPlayer sends an HTTP HEAD request to `/{token}/stream.raw`, THE HTTP_Audio_Server SHALL
   respond with HTTP 200 and the following headers:
   - `Content-Length: 0`
   - `Content-Type: application/x-hqplayer-raw`
   - `X-HQPlayer-Raw-Title: Roon NAA6 Bridge`
   - `X-HQPlayer-Raw-SampleRate: {rate}`
   - `X-HQPlayer-Raw-Channels: {channels}`
   - `X-HQPlayer-Raw-Format: int16le`
3. WHEN HQPlayer sends an HTTP GET request to `/{token}/stream.raw`, THE HTTP_Audio_Server SHALL
   respond with HTTP 200, the same headers as the HEAD response (excluding `Content-Length`), and a
   streaming body of raw PCM bytes read from the ALSA loopback capture device.
4. THE HTTP_Audio_Server SHALL stream PCM bytes to HQPlayer without modification to the sample
   values or channel interleaving.
5. THE HTTP_Audio_Server SHALL introduce no more than 50 milliseconds of additional end-to-end
   latency between ALSA capture and HTTP transmission under normal operating conditions.
6. IF the HTTP send buffer is full, THEN THE HTTP_Audio_Server SHALL apply backpressure to the ALSA
   capture path rather than dropping audio frames.
7. WHEN HQPlayer disconnects from the HTTP_Audio_Server, THE HTTP_Audio_Server SHALL close the
   connection and remain ready to accept a new connection.

---

### Requirement 6: Metadata Forwarding

**User Story:** As a user, I want the Bridge to forward the currently playing track's metadata to
HQPlayer, so that HQPlayer can display track information such as title, artist, and album.

#### Acceptance Criteria

1. WHEN roon-metadata-extension receives track metadata from Roon_Core for the active Roon_Output,
   THE roon-metadata-extension SHALL forward the track title, artist name, and album name to
   naa6-audio-bridge via the IPC_Socket as a newline-delimited JSON object
   `{ "title": "...", "artist": "...", "album": "..." }`.
2. WHEN naa6-audio-bridge receives metadata from the IPC_Socket, THE naa6-audio-bridge SHALL send a
   metadata XML_Message to HQPlayer via the Control_Client connection.
3. WHEN a new track begins playing, THE Bridge SHALL transmit the track metadata to HQPlayer within
   500 milliseconds of receiving the updated metadata from Roon_Core.
4. IF Roon_Core provides no metadata for the current track, THEN THE roon-metadata-extension SHALL
   not transmit a metadata message and SHALL log a DEBUG-level entry indicating that metadata is
   unavailable.
5. THE Bridge SHALL encode all text metadata fields as UTF-8 before transmission.

---

### Requirement 7: Configuration

**User Story:** As a user, I want to configure the Bridge via a configuration file, so that I can
adapt it to my network and naming preferences without modifying source code.

#### Acceptance Criteria

1. THE Bridge SHALL read its configuration from `/etc/roon-naa6-bridge/config.json`, falling back
   to `./config.json` if the primary path is absent.
2. THE Bridge SHALL support the following configurable parameters:
   - `outputName`: Roon Output display name
   - `alsaDevice`: ALSA capture device (default: `hw:Loopback,1,0`)
   - `hqplayerHost`: HQPlayer IP address (default: `192.168.30.212`)
   - `hqplayerPort`: HQPlayer control port (default: `4321`)
   - `listenPort`: Bridge control API listen port (default: `4321`)
   - `httpPort`: HTTP audio stream port (default: `30001`)
   - `reconnectBackoff`: reconnect delay in milliseconds (default: `5000`)
   - `logLevel`: `"info"` or `"debug"`
   - `ipcSocket`: Unix socket path for metadata IPC
3. IF the configuration file is absent at startup, THEN THE Bridge SHALL use documented default
   values for all parameters and log a warning indicating that defaults are in use.
4. IF the configuration file contains an unrecognised key, THEN THE Bridge SHALL log a warning
   identifying the unrecognised key and continue startup using the remaining valid configuration.
5. IF the configuration file contains an invalid value for a required parameter, THEN THE Bridge
   SHALL log a descriptive error identifying the parameter and its invalid value, and terminate with
   a non-zero exit code.

---

### Requirement 8: Logging and Observability

**User Story:** As a developer or operator, I want structured logs from the Bridge, so that I can
diagnose connection issues and monitor audio stream health.

#### Acceptance Criteria

1. THE Bridge SHALL emit log entries for the following events: startup, Roon_Core discovery,
   Roon_Output registration, ALSA capture start, Roon connection accepted on Control_Server,
   PlaylistAdd received from Roon, Control_Client connected to HQPlayer, PlaylistAdd sent to
   HQPlayer, HTTP audio stream started, HTTP audio stream stopped, HQPlayer disconnected, and
   graceful shutdown.
2. WHEN an error occurs, THE Bridge SHALL include a human-readable description and, where
   applicable, the underlying system error code in the log entry.
3. THE Bridge SHALL support at least two log verbosity levels: INFO and DEBUG.
4. WHILE operating at DEBUG verbosity, THE Bridge SHALL log each XML_Message exchanged on both the
   Control_Server and Control_Client connections.

---

### Requirement 9: Graceful Startup and Shutdown

**User Story:** As an operator, I want the Bridge to start and stop cleanly, so that it does not
leave stale registrations or open connections behind.

#### Acceptance Criteria

1. WHEN the naa6-audio-bridge receives a SIGTERM or SIGINT signal, THE naa6-audio-bridge SHALL
   cease audio streaming, close the HQPlayer TCP connection, close the ALSA capture device, and
   exit with code 0.
2. WHEN the roon-metadata-extension receives a SIGTERM or SIGINT signal, THE roon-metadata-extension
   SHALL deregister the Roon_Output from Roon_Core and exit with code 0.
3. THE Bridge SHALL complete the graceful shutdown sequence within 5 seconds of receiving a
   termination signal.
4. IF the graceful shutdown sequence cannot be completed within 5 seconds, THEN THE Bridge SHALL
   forcibly close all connections and exit with a non-zero exit code.

---

### Requirement 10: Platform Compatibility

**User Story:** As an operator, I want the Bridge to run on Ubuntu 24.04 LTS and later Ubuntu
releases, so that I can deploy it on a current, long-term-supported Linux platform.

#### Acceptance Criteria

1. THE Bridge SHALL run on Ubuntu 24.04 LTS and all subsequent Ubuntu LTS and interim releases.
2. THE Bridge SHALL declare all runtime dependencies explicitly so that they can be satisfied from
   the Ubuntu 24.04 LTS package repositories or bundled with the application.
3. THE Bridge SHALL be installable and operable as a systemd service unit on Ubuntu 24.04 LTS and
   later.
4. WHEN installed as a systemd service, THE Bridge SHALL support the standard systemd lifecycle
   commands: start, stop, restart, and status.
5. WHEN installed as a systemd service, THE Bridge SHALL respond to SIGTERM issued by systemd in
   accordance with the graceful shutdown sequence defined in Requirement 9.

---

### Requirement 11: HQPlayer Control API Protocol Correctness

**User Story:** As a developer, I want the Bridge to implement the HQPlayer control API exactly as
observed in live traffic captures, so that Roon and HQPlayer interoperate correctly with the Bridge.

#### Acceptance Criteria

1. THE naa6-audio-bridge SHALL format all outgoing XML_Messages as newline-terminated UTF-8 strings
   beginning with `<?xml version="1.0" encoding="utf-8"?>`.
2. THE Control_Server SHALL respond to messages in the session setup sequence in the order: GetInfo,
   SessionAuthentication, Stop, Status, VolumeRange, State, PlaylistClear, PlaylistAdd — matching
   the order observed in live Roon traffic.
3. WHEN the Control_Server sends a Status response, THE Control_Server SHALL include at minimum the
   fields `active_bits`, `active_channels`, `active_rate`, and `state`.
4. WHEN the Control_Server sends a State response, THE Control_Server SHALL include at minimum the
   fields `active_mode` and `active_rate`.
5. FOR ALL valid XML_Messages received from Roon, parsing the message and re-serialising it SHALL
   produce an XML document with equivalent semantic content (round-trip property).
