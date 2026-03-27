# Requirements Document

## Introduction

The Roon-NAA6 Bridge is a software endpoint that acts as a protocol bridge between Roon's RaaT (Roon Advanced Audio Transport) protocol and the HQPlayer NAA 6 (Network Audio Adapter version 6) protocol. It presents itself to a Roon Core as a valid audio output zone, receives the audio stream via RaaT, and re-transmits that stream to an HQPlayer instance using the NAA 6 protocol. This enables Roon users to route audio through HQPlayer for DSP processing without requiring native NAA 6 support in Roon.

## Glossary

- **Bridge**: The roon-naa6-bridge software application described in this document
- **Roon_Core**: The Roon server software that manages the music library and distributes audio streams
- **RaaT**: Roon Advanced Audio Transport — the proprietary protocol used by Roon Core to stream audio to endpoints
- **Roon_Extension**: A software component that integrates with Roon Core via the Roon Extension API
- **Roon_Output**: A virtual audio output zone registered by the Bridge with the Roon Core
- **NAA6**: HQPlayer Network Audio Adapter protocol version 6 — the protocol used to stream audio to HQPlayer
- **HQPlayer**: The HQPlayer application that receives audio via NAA 6 for DSP processing and playback
- **Audio_Stream**: A continuous sequence of PCM or DSD audio samples with associated metadata
- **Sample_Rate**: The number of audio samples per second, expressed in Hz (e.g. 44100, 96000, 768000)
- **Bit_Depth**: The number of bits per audio sample (e.g. 16, 24, 32)
- **Channel_Count**: The number of audio channels in a stream (e.g. 2 for stereo)
- **Format_Descriptor**: A data structure describing the encoding, sample rate, bit depth, and channel count of an Audio_Stream
- **DSD**: Direct Stream Digital — a 1-bit pulse-density modulation audio encoding used in high-resolution audio
- **DSD_Rate**: The DSD clock frequency expressed as a multiple of the base DSD64 rate (2.8224 MHz); supported multiples are DSD64, DSD128, DSD256, and DSD512
- **DSD64**: The base DSD rate of 2.8224 MHz (64× the 44.1 kHz PCM base rate)
- **DSD128**: DSD at 5.6448 MHz (128× the 44.1 kHz PCM base rate)
- **DSD256**: DSD at 11.2896 MHz (256× the 44.1 kHz PCM base rate)
- **DSD512**: DSD at 22.5792 MHz (512× the 44.1 kHz PCM base rate)
- **DoP**: DSD over PCM — a method of encapsulating DSD data within PCM frames for transport over PCM-capable interfaces
- **Handshake**: The initial protocol negotiation sequence between two communicating parties
- **Keepalive**: A periodic message sent to maintain an active connection
- **Track_Metadata**: Descriptive information associated with the currently playing track, including track title, artist name, album name, and cover art image

## Requirements

### Requirement 1: Roon Endpoint Registration

**User Story:** As a Roon user, I want the Bridge to appear as a selectable audio output in Roon, so that I can route playback to HQPlayer via the Bridge.

#### Acceptance Criteria

1. THE Bridge SHALL register itself with the Roon_Core as a Roon_Extension using the Roon Extension API.
2. THE Bridge SHALL advertise a Roon_Output zone with a configurable display name.
3. WHEN the Roon_Core is discovered on the network, THE Bridge SHALL initiate the pairing and registration sequence automatically.
4. IF the connection to the Roon_Core is lost, THEN THE Bridge SHALL attempt to reconnect at intervals of no greater than 30 seconds.
5. WHEN the Bridge is shut down gracefully, THE Bridge SHALL deregister the Roon_Output from the Roon_Core.

---

### Requirement 2: RaaT Audio Stream Reception

**User Story:** As a Roon user, I want the Bridge to correctly receive the audio stream from Roon Core, so that audio data is not lost or corrupted in transit.

#### Acceptance Criteria

1. WHEN the Roon_Core begins streaming to the Roon_Output, THE Bridge SHALL accept the incoming RaaT Audio_Stream.
2. THE Bridge SHALL support PCM audio at sample rates of 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, and 768000 Hz.
3. THE Bridge SHALL support bit depths of 16, 24, and 32 bits per sample.
4. THE Bridge SHALL support stereo (2-channel) and multichannel audio streams of up to 8 channels.
5. THE Bridge SHALL support DSD audio at DSD_Rates of DSD64, DSD128, DSD256, and DSD512.
6. WHEN the Roon_Core sends a DSD stream, THE Bridge SHALL accept the DSD data in either native DSD or DoP encapsulation as indicated by the RaaT Format_Descriptor.
7. WHEN the Roon_Core sends a stream format change, THE Bridge SHALL update the active Format_Descriptor before forwarding subsequent audio data.
8. IF the RaaT stream contains malformed or unrecognised data, THEN THE Bridge SHALL log the error and discard the affected audio frames without terminating the session.

---

### Requirement 3: NAA 6 Connection Management

**User Story:** As a user, I want the Bridge to establish and maintain a reliable NAA 6 connection to HQPlayer, so that audio is delivered without interruption.

#### Acceptance Criteria

1. THE Bridge SHALL connect to HQPlayer using the NAA 6 protocol over TCP/IP at a configurable host address and port.
2. WHEN the Bridge starts, THE Bridge SHALL perform the NAA 6 Handshake with HQPlayer before transmitting any audio data.
3. WHILE a session is active, THE Bridge SHALL send NAA 6 Keepalive messages at the interval required by the NAA 6 protocol specification.
4. IF the NAA 6 connection to HQPlayer is lost during playback, THEN THE Bridge SHALL pause audio forwarding, attempt to reconnect, and resume forwarding upon successful reconnection.
5. IF the NAA 6 Handshake fails, THEN THE Bridge SHALL log a descriptive error message including the failure reason and retry after a configurable backoff interval.
6. WHEN the Bridge is shut down gracefully, THE Bridge SHALL send the NAA 6 session termination message before closing the TCP connection.

---

### Requirement 4: Audio Stream Bridging

**User Story:** As a user, I want the Bridge to faithfully forward audio from Roon to HQPlayer without altering the audio content, so that HQPlayer receives bit-perfect audio for DSP processing.

#### Acceptance Criteria

1. WHEN audio data is received from the Roon_Core, THE Bridge SHALL forward the audio samples to HQPlayer via the NAA 6 protocol without modification to the sample values.
2. WHEN the active Format_Descriptor changes, THE Bridge SHALL transmit the updated format parameters to HQPlayer via the NAA 6 protocol before sending subsequent audio frames.
3. THE Bridge SHALL preserve the original sample order and channel interleaving of the Audio_Stream when forwarding to HQPlayer.
4. THE Bridge SHALL introduce no more than 50 milliseconds of additional end-to-end latency between RaaT reception and NAA 6 transmission under normal operating conditions.
5. IF the NAA 6 send buffer is full, THEN THE Bridge SHALL apply backpressure to the RaaT receive path rather than dropping audio frames.

---

### Requirement 5: Format Negotiation

**User Story:** As a user, I want the Bridge to negotiate a compatible audio format between Roon and HQPlayer, so that playback succeeds across a range of source formats.

#### Acceptance Criteria

1. WHEN initiating a NAA 6 session, THE Bridge SHALL advertise the Format_Descriptor of the incoming RaaT stream to HQPlayer during the NAA 6 Handshake.
2. WHEN HQPlayer responds with an accepted Format_Descriptor, THE Bridge SHALL use that format for the duration of the session.
3. IF HQPlayer rejects the proposed Format_Descriptor, THEN THE Bridge SHALL log the rejection reason and surface an error to the Roon_Output indicating that the format is unsupported.
4. THE Bridge SHALL support format negotiation for all PCM sample rates and bit depths listed in Requirement 2.
5. THE Bridge SHALL support format negotiation for all DSD_Rates listed in Requirement 2.
6. WHEN negotiating a DSD session, THE Bridge SHALL advertise the DSD_Rate and the encapsulation method (native DSD or DoP) to HQPlayer as part of the NAA 6 Handshake.

---

### Requirement 6: Configuration

**User Story:** As a user, I want to configure the Bridge via a configuration file, so that I can adapt it to my network and naming preferences without modifying source code.

#### Acceptance Criteria

1. THE Bridge SHALL read its configuration from a file in a well-known location at startup.
2. THE Bridge SHALL support the following configurable parameters: Roon_Output display name, HQPlayer host address, HQPlayer NAA 6 port, reconnection backoff interval, and log verbosity level.
3. IF the configuration file is absent at startup, THEN THE Bridge SHALL use documented default values for all parameters and log a warning indicating that defaults are in use.
4. IF the configuration file contains an unrecognised key, THEN THE Bridge SHALL log a warning identifying the unrecognised key and continue startup using the remaining valid configuration.
5. IF the configuration file contains an invalid value for a required parameter, THEN THE Bridge SHALL log a descriptive error identifying the parameter and its invalid value, and terminate with a non-zero exit code.

---

### Requirement 7: Logging and Observability

**User Story:** As a developer or operator, I want structured logs from the Bridge, so that I can diagnose connection issues and monitor audio stream health.

#### Acceptance Criteria

1. THE Bridge SHALL emit log entries for the following events: startup, Roon_Core discovery, Roon_Output registration, RaaT stream start, RaaT stream stop, NAA 6 connection established, NAA 6 connection lost, format negotiation result, and graceful shutdown.
2. WHEN an error occurs, THE Bridge SHALL include a human-readable description and, where applicable, the underlying system error code in the log entry.
3. THE Bridge SHALL support at least two log verbosity levels: INFO and DEBUG.
4. WHILE operating at DEBUG verbosity, THE Bridge SHALL log the Format_Descriptor of each audio format change event.

---

### Requirement 9: Track Metadata Forwarding

**User Story:** As a user, I want the Bridge to forward the currently playing track's metadata to HQPlayer alongside the audio stream, so that HQPlayer can display track information such as title, artist, album, and cover art.

#### Acceptance Criteria

1. WHEN Roon_Core provides track metadata for the active Roon_Output, THE Bridge SHALL extract the track title, artist name, album name, and cover art from the RaaT metadata payload.
2. WHEN a new track begins playing, THE Bridge SHALL transmit the track metadata to HQPlayer using the NAA 6 metadata message type before or alongside the first audio frame of that track.
3. WHEN the track metadata changes during playback (e.g. on a track transition), THE Bridge SHALL send an updated metadata message to HQPlayer within 500 milliseconds of receiving the updated metadata from Roon_Core.
4. IF cover art is not available for the current track, THEN THE Bridge SHALL transmit the remaining available metadata fields (title, artist, album) and omit the cover art field without treating the absence as an error.
5. IF Roon_Core provides no metadata for the current track, THEN THE Bridge SHALL not transmit a metadata message and SHALL log a DEBUG-level entry indicating that metadata is unavailable.
6. THE Bridge SHALL encode all text metadata fields as UTF-8 before transmission via the NAA 6 protocol.
7. THE Bridge SHALL support cover art images up to 1024 × 1024 pixels as permitted by the NAA 6 protocol specification.

---

### Requirement 10: Platform Compatibility

**User Story:** As an operator, I want the Bridge to run on Ubuntu 24.04 LTS and later Ubuntu releases, so that I can deploy it on a current, long-term-supported Linux platform.

#### Acceptance Criteria

1. THE Bridge SHALL run on Ubuntu 24.04 LTS and all subsequent Ubuntu LTS and interim releases.
2. THE Bridge SHALL declare all runtime dependencies explicitly so that they can be satisfied from the Ubuntu 24.04 LTS package repositories or bundled with the application.
3. THE Bridge SHALL be installable and operable as a systemd service unit on Ubuntu 24.04 LTS and later.
4. WHEN installed as a systemd service, THE Bridge SHALL support the standard systemd lifecycle commands: start, stop, restart, and status.
5. WHEN installed as a systemd service, THE Bridge SHALL respond to SIGTERM issued by systemd in accordance with the graceful shutdown sequence defined in Requirement 8.

---

### Requirement 8: Graceful Startup and Shutdown

**User Story:** As an operator, I want the Bridge to start and stop cleanly, so that it does not leave stale registrations or open connections behind.

#### Acceptance Criteria

1. WHEN the Bridge receives a SIGTERM or SIGINT signal, THE Bridge SHALL complete the graceful shutdown sequence defined in Requirements 1.5, 3.6, and 7.1.
2. THE Bridge SHALL complete the graceful shutdown sequence within 5 seconds of receiving a termination signal.
3. IF the graceful shutdown sequence cannot be completed within 5 seconds, THEN THE Bridge SHALL forcibly close all connections and exit with a non-zero exit code.
