import { EventEmitter } from 'node:events';
import type { Config, TrackMetadata } from './types';
import type { Logger } from './Logger';

// node-roon-api is a CommonJS module; use dynamic import or require
// We use a type-only approach and dynamic require for compatibility
type RoonApi = {
  new(opts: RoonApiOptions): RoonApiInstance;
};

interface RoonApiOptions {
  extension_id: string;
  display_name: string;
  display_version: string;
  publisher: string;
  email: string;
  required_services: unknown[];
  optional_services: unknown[];
  provided_services: unknown[];
  core_paired: (core: RoonCore) => void;
  core_unpaired: (core: RoonCore) => void;
}

interface RoonCore {
  display_name: string;
  services: {
    RoonApiTransport?: RoonApiTransport;
  };
}

interface RoonApiTransport {
  subscribe_zones: (cb: (cmd: string, data: ZoneData) => void) => void;
  unsubscribe_zones: () => void;
}

interface ZoneData {
  zones?: Zone[];
  zones_changed?: Zone[];
  zones_seek_changed?: unknown[];
}

interface Zone {
  zone_id: string;
  display_name: string;
  now_playing?: NowPlaying;
  state?: string;
}

interface NowPlaying {
  one_line?: { line1?: string };
  two_line?: { line1?: string; line2?: string };
  three_line?: { line1?: string; line2?: string; line3?: string };
  image_key?: string;
}

interface RoonApiInstance {
  init_services: (opts: { required_services: unknown[]; optional_services: unknown[] }) => void;
  start_discovery: () => void;
  stop: () => void;
}

/**
 * Events emitted by RoonExtension:
 * - 'metadataChanged': (metadata: TrackMetadata) => void
 * - 'core_paired': (coreName: string) => void
 * - 'core_unpaired': (coreName: string) => void
 */
export class RoonExtension extends EventEmitter {
  private config: Config;
  private logger: Logger;
  private roon: RoonApiInstance | null = null;
  private transport: RoonApiTransport | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private paired = false;

  constructor(config: Config, logger: Logger) {
    super();
    this.config = config;
    this.logger = logger;
  }

  /**
   * Start the Roon extension: discover Core, pair, subscribe to zones
   */
  start(): void {
    this.logger.info('RoonExtension starting; discovering Roon Core via mDNS');

    // Dynamically require node-roon-api (CommonJS module)
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const RoonApiCtor = (require('node-roon-api') as { default?: RoonApi } | RoonApi);
    const RoonApiClass = ('default' in RoonApiCtor ? RoonApiCtor.default : RoonApiCtor) as RoonApi;

    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const RoonApiTransportCtor = require('node-roon-api-transport') as unknown;

    const roon = new RoonApiClass({
      extension_id: 'com.roon-naa6-bridge.metadata',
      display_name: this.config.outputName,
      display_version: '1.0.0',
      publisher: 'roon-naa6-bridge',
      email: 'noreply@roon-naa6-bridge',
      required_services: [RoonApiTransportCtor],
      optional_services: [],
      provided_services: [],
      core_paired: (core: RoonCore) => this.onCorePaired(core),
      core_unpaired: (core: RoonCore) => this.onCoreUnpaired(core),
    });

    roon.init_services({
      required_services: [RoonApiTransportCtor],
      optional_services: [],
    });

    roon.start_discovery();
    this.roon = roon;
    this.logger.info('Roon Core discovery started');
  }

  /**
   * Deregister from Roon Core and stop discovery
   */
  deregister(): void {
    this.logger.info('Deregistering from Roon Core');
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.transport) {
      try {
        this.transport.unsubscribe_zones();
      } catch (e) {
        this.logger.warn({ err: e }, 'Error unsubscribing from zones during deregister');
      }
      this.transport = null;
    }
    if (this.roon) {
      try {
        this.roon.stop();
      } catch (e) {
        this.logger.warn({ err: e }, 'Error stopping Roon API during deregister');
      }
      this.roon = null;
    }
    this.paired = false;
    this.logger.info('Roon deregistration complete');
  }

  private onCorePaired(core: RoonCore): void {
    this.logger.info({ coreName: core.display_name }, 'Roon Core paired');
    this.paired = true;
    this.emit('core_paired', core.display_name);

    const transport = core.services.RoonApiTransport;
    if (!transport) {
      this.logger.warn('RoonApiTransport not available on paired core');
      return;
    }
    this.transport = transport;

    transport.subscribe_zones((cmd: string, data: ZoneData) => {
      this.handleZoneUpdate(cmd, data);
    });

    this.logger.info('Subscribed to Roon zone/transport events');
  }

  private onCoreUnpaired(core: RoonCore): void {
    this.logger.info({ coreName: core.display_name }, 'Roon Core unpaired');
    this.paired = false;
    this.transport = null;
    this.emit('core_unpaired', core.display_name);

    // Schedule reconnect
    this.logger.info(
      { backoffMs: this.config.reconnectBackoff },
      'Scheduling Roon Core reconnect'
    );
    this.reconnectTimer = setTimeout(() => {
      this.logger.info('Attempting Roon Core reconnect');
      // The Roon SDK handles re-discovery automatically; we just log
    }, this.config.reconnectBackoff);
  }

  private handleZoneUpdate(cmd: string, data: ZoneData): void {
    const zones = data.zones ?? data.zones_changed ?? [];
    for (const zone of zones) {
      if (!zone.now_playing) continue;

      const metadata = this.extractMetadata(zone.now_playing);
      this.logger.debug({ zoneId: zone.zone_id, metadata }, 'Zone metadata changed');
      this.emit('metadataChanged', metadata);
    }
  }

  private extractMetadata(nowPlaying: NowPlaying): TrackMetadata {
    const metadata: TrackMetadata = {};

    // three_line: line1=title, line2=artist, line3=album
    if (nowPlaying.three_line) {
      if (nowPlaying.three_line.line1) metadata.title = nowPlaying.three_line.line1;
      if (nowPlaying.three_line.line2) metadata.artist = nowPlaying.three_line.line2;
      if (nowPlaying.three_line.line3) metadata.album = nowPlaying.three_line.line3;
    } else if (nowPlaying.two_line) {
      if (nowPlaying.two_line.line1) metadata.title = nowPlaying.two_line.line1;
      if (nowPlaying.two_line.line2) metadata.artist = nowPlaying.two_line.line2;
    } else if (nowPlaying.one_line) {
      if (nowPlaying.one_line.line1) metadata.title = nowPlaying.one_line.line1;
    }

    // Cover art is fetched separately via image_key; omit if not available
    // (image fetching would require additional Roon API calls)

    return metadata;
  }

  isPaired(): boolean {
    return this.paired;
  }
}
