import { readFileSync, existsSync } from 'node:fs';
import type { Config } from './types';

const SYSTEM_CONFIG_PATH = '/etc/roon-naa6-bridge/config.json';
const LOCAL_CONFIG_PATH = './config.json';

const KNOWN_KEYS: ReadonlySet<string> = new Set([
  'outputName',
  'alsaDevice',
  'hqplayerHost',
  'hqplayerPort',
  'reconnectBackoff',
  'logLevel',
  'ipcSocket',
]);

const DEFAULTS: Config = {
  outputName: 'HQPlayer via NAA6',
  alsaDevice: 'hw:Loopback,1,0',
  hqplayerHost: '127.0.0.1',
  hqplayerPort: 10700,
  reconnectBackoff: 5000,
  logLevel: 'info',
  ipcSocket: '/run/roon-naa6-bridge/meta.sock',
};

/**
 * Load and validate configuration from the well-known locations.
 * Falls back to defaults if no config file is found.
 * Exits with code 1 on invalid required parameter values.
 */
export function loadConfig(
  warn: (msg: string) => void = (m) => console.warn('[WARN]', m),
  errorAndExit: (msg: string) => never = (m) => { console.error('[ERROR]', m); process.exit(1); }
): Config {
  const raw = readConfigFile(warn);
  if (raw === null) {
    warn(`No config file found at ${SYSTEM_CONFIG_PATH} or ${LOCAL_CONFIG_PATH}; using defaults`);
    return Object.freeze({ ...DEFAULTS });
  }

  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(raw) as Record<string, unknown>;
  } catch (e) {
    errorAndExit(`Failed to parse config file: ${(e as Error).message}`);
  }

  // Warn on unknown keys
  for (const key of Object.keys(parsed)) {
    if (!KNOWN_KEYS.has(key)) {
      warn(`Unknown config key '${key}'; ignoring`);
    }
  }

  // Merge with defaults
  const config: Config = {
    outputName: (parsed['outputName'] as string) ?? DEFAULTS.outputName,
    alsaDevice: (parsed['alsaDevice'] as string) ?? DEFAULTS.alsaDevice,
    hqplayerHost: (parsed['hqplayerHost'] as string) ?? DEFAULTS.hqplayerHost,
    hqplayerPort: (parsed['hqplayerPort'] as number) ?? DEFAULTS.hqplayerPort,
    reconnectBackoff: (parsed['reconnectBackoff'] as number) ?? DEFAULTS.reconnectBackoff,
    logLevel: (parsed['logLevel'] as 'info' | 'debug') ?? DEFAULTS.logLevel,
    ipcSocket: (parsed['ipcSocket'] as string) ?? DEFAULTS.ipcSocket,
  };

  validateConfig(config, errorAndExit);

  return Object.freeze(config);
}

function readConfigFile(warn: (msg: string) => void): string | null {
  if (existsSync(SYSTEM_CONFIG_PATH)) {
    try {
      return readFileSync(SYSTEM_CONFIG_PATH, 'utf-8');
    } catch (e) {
      warn(`Failed to read ${SYSTEM_CONFIG_PATH}: ${(e as Error).message}`);
    }
  }
  if (existsSync(LOCAL_CONFIG_PATH)) {
    try {
      return readFileSync(LOCAL_CONFIG_PATH, 'utf-8');
    } catch (e) {
      warn(`Failed to read ${LOCAL_CONFIG_PATH}: ${(e as Error).message}`);
    }
  }
  return null;
}

function validateConfig(
  config: Config,
  errorAndExit: (msg: string) => never
): void {
  if (!Number.isInteger(config.hqplayerPort) || config.hqplayerPort < 1 || config.hqplayerPort > 65535) {
    errorAndExit(
      `Invalid config value: hqplayerPort=${config.hqplayerPort} is out of range [1, 65535]`
    );
  }

  if (!Number.isFinite(config.reconnectBackoff) || config.reconnectBackoff <= 0) {
    errorAndExit(
      `Invalid config value: reconnectBackoff=${config.reconnectBackoff} must be greater than 0`
    );
  }

  const validLogLevels = ['info', 'debug'];
  if (!validLogLevels.includes(config.logLevel)) {
    errorAndExit(
      `Invalid config value: logLevel='${config.logLevel}' must be one of ${JSON.stringify(validLogLevels)}`
    );
  }
}
