/**
 * Bridge configuration (shared with naa6-audio-bridge via /etc/roon-naa6-bridge/config.json)
 */
export interface Config {
  /** Roon Output display name (default: "HQPlayer via NAA6") */
  outputName: string;
  /** ALSA loopback capture device (default: "hw:Loopback,1,0") */
  alsaDevice: string;
  /** HQPlayer hostname or IP (default: "127.0.0.1") */
  hqplayerHost: string;
  /** NAA 6 TCP port (default: 10700) */
  hqplayerPort: number;
  /** Reconnect interval in ms (default: 5000) */
  reconnectBackoff: number;
  /** Log verbosity level (default: "info") */
  logLevel: 'info' | 'debug';
  /** Unix domain socket path for metadata IPC (default: "/run/roon-naa6-bridge/meta.sock") */
  ipcSocket: string;
}

/**
 * Track metadata from Roon
 */
export interface TrackMetadata {
  title?: string;
  artist?: string;
  album?: string;
  /** Raw image bytes, max 1024×1024 */
  coverArt?: Buffer;
}

/**
 * NAA 6 wire frame (internal representation)
 */
export interface Naa6Frame {
  /** Message type byte per NAA 6 spec */
  type: number;
  /** Payload length (uint32 LE) */
  length: number;
  payload: Buffer;
}

/**
 * NAA 6 message type constants
 */
export const NAA6_MSG_TYPE = {
  HANDSHAKE: 0x01,
  AUDIO: 0x02,
  FORMAT_CHANGE: 0x03,
  METADATA: 0x04,
  KEEPALIVE: 0x05,
  TERMINATION: 0x06,
} as const;

/**
 * Encode a NAA 6 frame to bytes: 1-byte type + uint32 LE length + payload
 */
export function encodeNaa6Frame(frame: Naa6Frame): Buffer {
  const header = Buffer.allocUnsafe(5);
  header.writeUInt8(frame.type, 0);
  header.writeUInt32LE(frame.payload.length, 1);
  return Buffer.concat([header, frame.payload]);
}
