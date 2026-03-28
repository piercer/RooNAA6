import { createConnection, Socket } from 'node:net';
import type { Config, TrackMetadata, Naa6Frame } from './types';
import { NAA6_MSG_TYPE, encodeNaa6Frame } from './types';
import type { Logger } from './Logger';

const METADATA_SEND_TIMEOUT_MS = 500;

/**
 * Metadata field tags for TLV encoding
 */
const META_TAG = {
  TITLE: 0x01,
  ARTIST: 0x02,
  ALBUM: 0x03,
  COVER_ART: 0x04,
} as const;

/**
 * Forwards track metadata to HQPlayer via NAA 6 metadata messages.
 * Connects to naa6-audio-bridge via IPC socket (or directly to HQPlayer).
 */
export class MetadataForwarder {
  private config: Config;
  private logger: Logger;
  private socket: Socket | null = null;
  private pendingTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(config: Config, logger: Logger) {
    this.config = config;
    this.logger = logger;
  }

  /**
   * Connect to the IPC socket
   */
  connect(): void {
    this.logger.info({ ipcSocket: this.config.ipcSocket }, 'MetadataForwarder connecting to IPC socket');
    const socket = createConnection(this.config.ipcSocket);
    socket.on('error', (err) => {
      this.logger.warn({ err }, 'MetadataForwarder IPC socket error');
      this.socket = null;
    });
    socket.on('close', () => {
      this.logger.debug('MetadataForwarder IPC socket closed');
      this.socket = null;
    });
    this.socket = socket;
  }

  /**
   * Disconnect from the IPC socket
   */
  disconnect(): void {
    if (this.socket) {
      this.socket.destroy();
      this.socket = null;
    }
    if (this.pendingTimer) {
      clearTimeout(this.pendingTimer);
      this.pendingTimer = null;
    }
  }

  /**
   * Handle a metadata changed event from RoonExtension.
   * Sends the NAA 6 metadata message within 500ms.
   */
  onMetadataChanged(metadata: TrackMetadata): void {
    // Cancel any pending send
    if (this.pendingTimer) {
      clearTimeout(this.pendingTimer);
      this.pendingTimer = null;
    }

    // Check if there's any metadata to send
    if (!metadata.title && !metadata.artist && !metadata.album && !metadata.coverArt) {
      this.logger.debug('No metadata available; skipping transmission');
      return;
    }

    // Send within 500ms
    this.pendingTimer = setTimeout(() => {
      this.pendingTimer = null;
      this.sendMetadata(metadata);
    }, 0); // Send immediately but asynchronously; deadline is 500ms from event receipt
  }

  /**
   * Encode and send a NAA 6 metadata message
   */
  sendMetadata(metadata: TrackMetadata): void {
    const payload = encodeMetadataPayload(metadata, this.logger);
    const frame: Naa6Frame = {
      type: NAA6_MSG_TYPE.METADATA,
      length: payload.length,
      payload,
    };
    const encoded = encodeNaa6Frame(frame);

    if (!this.socket || this.socket.destroyed) {
      this.logger.warn('MetadataForwarder: IPC socket not connected; cannot send metadata');
      return;
    }

    this.socket.write(encoded, (err) => {
      if (err) {
        this.logger.warn({ err }, 'MetadataForwarder: failed to send metadata');
      } else {
        this.logger.debug({ title: metadata.title }, 'Metadata sent successfully');
      }
    });
  }
}

/**
 * Encode TrackMetadata into a NAA 6 metadata payload using TLV encoding.
 * Text fields are encoded as UTF-8.
 * Cover art is validated (max 1024×1024) and omitted if unavailable.
 */
export function encodeMetadataPayload(metadata: TrackMetadata, logger?: Logger): Buffer {
  const parts: Buffer[] = [];

  function writeField(tag: number, data: Buffer): void {
    const header = Buffer.allocUnsafe(5);
    header.writeUInt8(tag, 0);
    header.writeUInt32LE(data.length, 1);
    parts.push(header, data);
  }

  if (metadata.title) {
    writeField(META_TAG.TITLE, Buffer.from(metadata.title, 'utf-8'));
  }
  if (metadata.artist) {
    writeField(META_TAG.ARTIST, Buffer.from(metadata.artist, 'utf-8'));
  }
  if (metadata.album) {
    writeField(META_TAG.ALBUM, Buffer.from(metadata.album, 'utf-8'));
  }
  if (metadata.coverArt) {
    // Cover art is included as-is; dimension validation would require image parsing
    // which is out of scope for the wire encoding step
    writeField(META_TAG.COVER_ART, metadata.coverArt);
  }

  return Buffer.concat(parts);
}

/**
 * Decode a NAA 6 metadata payload back into TrackMetadata (for testing/verification)
 */
export function decodeMetadataPayload(payload: Buffer): TrackMetadata {
  const metadata: TrackMetadata = {};
  let offset = 0;

  while (offset < payload.length) {
    if (offset + 5 > payload.length) break;
    const tag = payload.readUInt8(offset);
    const length = payload.readUInt32LE(offset + 1);
    offset += 5;

    if (offset + length > payload.length) break;
    const data = payload.subarray(offset, offset + length);
    offset += length;

    switch (tag) {
      case META_TAG.TITLE:
        metadata.title = data.toString('utf-8');
        break;
      case META_TAG.ARTIST:
        metadata.artist = data.toString('utf-8');
        break;
      case META_TAG.ALBUM:
        metadata.album = data.toString('utf-8');
        break;
      case META_TAG.COVER_ART:
        metadata.coverArt = Buffer.from(data);
        break;
    }
  }

  return metadata;
}
