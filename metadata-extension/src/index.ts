import { loadConfig } from './ConfigReader';
import { createLogger } from './Logger';
import { RoonExtension } from './RoonExtension';
import { MetadataForwarder } from './MetadataForwarder';
import { ShutdownHandler } from './ShutdownHandler';
import type { TrackMetadata } from './types';

// 1. Load configuration
const config = loadConfig();

// 2. Initialise logger
const logger = createLogger(config);

logger.info({ outputName: config.outputName }, 'roon-metadata-extension starting up');

// 3. Instantiate components
const roonExtension = new RoonExtension(config, logger);
const metadataForwarder = new MetadataForwarder(config, logger);

// 4. Wire up shutdown handler
const shutdownHandler = new ShutdownHandler(logger, async () => {
  metadataForwarder.disconnect();
  roonExtension.deregister();
});
shutdownHandler.register();

// 5. Connect MetadataForwarder to IPC socket
try {
  metadataForwarder.connect();
} catch (e) {
  logger.warn({ err: e }, 'MetadataForwarder could not connect to IPC socket; metadata will not be forwarded until connection is established');
}

// 6. Wire RoonExtension metadataChanged events → MetadataForwarder
roonExtension.on('metadataChanged', (metadata: TrackMetadata) => {
  metadataForwarder.onMetadataChanged(metadata);
});

roonExtension.on('core_paired', (coreName: string) => {
  logger.info({ coreName }, 'Roon Core paired');
});

roonExtension.on('core_unpaired', (coreName: string) => {
  logger.info({ coreName }, 'Roon Core unpaired');
});

// 7. Start Roon extension (begins mDNS discovery)
roonExtension.start();

logger.info('roon-metadata-extension initialised; waiting for Roon Core');
