/**
 * Bridge configuration (shared with naa6-audio-bridge via /etc/roon-naa6-bridge/config.json)
 */
export interface Config {
  /** Roon Output display name (default: "HQPlayer via NAA6") */
  outputName: string;
  /** ALSA loopback capture device (default: "hw:Loopback,1,0") */
  alsaDevice: string;
  /** NAA daemon listen port (default: 43210) */
  listenPort: number;
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
}
