import pino from 'pino';
import type { Logger as PinoLogger } from 'pino';
import type { Config } from './types';

export type Logger = PinoLogger;

let _logger: Logger | null = null;

/**
 * Create or return the singleton logger instance.
 * Must be called with a Config before first use.
 */
export function createLogger(config: Pick<Config, 'logLevel'>): Logger {
  _logger = pino({
    level: config.logLevel,
    transport: process.env['NODE_ENV'] !== 'production'
      ? { target: 'pino-pretty', options: { colorize: true } }
      : undefined,
  });
  return _logger;
}

/**
 * Get the current logger instance. Throws if not yet initialised.
 */
export function getLogger(): Logger {
  if (!_logger) {
    throw new Error('Logger not initialised; call createLogger() first');
  }
  return _logger;
}

/**
 * Reset the logger (for testing)
 */
export function resetLogger(): void {
  _logger = null;
}
