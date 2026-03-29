import { createConnection, Socket } from 'node:net';
import type { Config, TrackMetadata } from './types';
import type { Logger } from './Logger';

const RECONNECT_DELAY_MS = 1000;

/**
 * Forwards track metadata to naa6-audio-bridge via a Unix IPC socket.
 * Sends newline-delimited JSON: {"title":"...","artist":"...","album":"..."}\n
 * Reconnects automatically on socket error.
 */
export class MetadataForwarder {
  private config: Config;
  private logger: Logger;
  private socket: Socket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private connected = false;

  constructor(config: Config, logger: Logger) {
    this.config = config;
    this.logger = logger;
  }

  /**
   * Connect to the IPC socket
   */
  connect(): void {
    this.logger.info({ ipcSocket: this.config.ipcSocket }, 'MetadataForwarder connecting to IPC socket');
    this._doConnect();
  }

  private _doConnect(): void {
    if (this.socket) {
      this.socket.destroy();
      this.socket = null;
    }

    const socket = createConnection(this.config.ipcSocket);

    socket.on('connect', () => {
      this.connected = true;
      this.logger.info('MetadataForwarder connected to IPC socket');
    });

    socket.on('error', (err) => {
      this.connected = false;
      this.logger.warn({ err }, 'MetadataForwarder IPC socket error; will reconnect');
      this._scheduleReconnect();
    });

    socket.on('close', () => {
      this.connected = false;
      this.logger.debug('MetadataForwarder IPC socket closed; will reconnect');
      this._scheduleReconnect();
    });

    this.socket = socket;
  }

  private _scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.logger.info('MetadataForwarder reconnecting to IPC socket');
      this._doConnect();
    }, RECONNECT_DELAY_MS);
  }

  /**
   * Disconnect from the IPC socket
   */
  disconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.socket) {
      this.socket.destroy();
      this.socket = null;
    }
    this.connected = false;
  }

  /**
   * Handle a metadata changed event from RoonExtension.
   * If no title/artist/album, logs DEBUG and returns without sending.
   * Serialises {title, artist, album} as JSON + \n and writes to socket.
   */
  onMetadataChanged(metadata: TrackMetadata): void {
    if (!metadata.title && !metadata.artist && !metadata.album) {
      this.logger.debug('No metadata available; skipping transmission');
      return;
    }

    const payload: { title?: string; artist?: string; album?: string } = {};
    if (metadata.title) payload.title = metadata.title;
    if (metadata.artist) payload.artist = metadata.artist;
    if (metadata.album) payload.album = metadata.album;

    const json = JSON.stringify(payload) + '\n';

    if (!this.socket || !this.connected || this.socket.destroyed) {
      this.logger.warn('MetadataForwarder: IPC socket not connected; cannot send metadata');
      return;
    }

    this.socket.write(json, 'utf-8', (err) => {
      if (err) {
        this.logger.warn({ err }, 'MetadataForwarder: failed to send metadata');
      } else {
        this.logger.debug({ title: metadata.title }, 'Metadata sent successfully');
      }
    });
  }
}

/**
 * Encode TrackMetadata as a JSON string (for testing/verification).
 * Returns the JSON string without the trailing newline.
 */
export function encodeMetadataJson(metadata: TrackMetadata): string {
  const payload: { title?: string; artist?: string; album?: string } = {};
  if (metadata.title) payload.title = metadata.title;
  if (metadata.artist) payload.artist = metadata.artist;
  if (metadata.album) payload.album = metadata.album;
  return JSON.stringify(payload);
}

/**
 * Decode a JSON metadata string back into TrackMetadata (for testing/verification).
 */
export function decodeMetadataJson(json: string): TrackMetadata {
  return JSON.parse(json) as TrackMetadata;
}
