import type { Logger } from './Logger';

const SHUTDOWN_DEADLINE_MS = 5000;

export interface ShutdownHandlerOptions {
  /** Override setTimeout for testing */
  setTimeout?: typeof setTimeout;
  /** Override clearTimeout for testing */
  clearTimeout?: typeof clearTimeout;
}

/**
 * Handles graceful shutdown on SIGTERM/SIGINT.
 * Ensures shutdown completes within 5 seconds or force-exits with code 1.
 */
export class ShutdownHandler {
  private logger: Logger;
  private deregister: () => Promise<void> | void;
  private registered = false;
  private _setTimeout: typeof setTimeout;
  private _clearTimeout: typeof clearTimeout;

  constructor(logger: Logger, deregister: () => Promise<void> | void, opts: ShutdownHandlerOptions = {}) {
    this.logger = logger;
    this.deregister = deregister;
    this._setTimeout = opts.setTimeout ?? globalThis.setTimeout;
    this._clearTimeout = opts.clearTimeout ?? globalThis.clearTimeout;
  }

  /**
   * Register SIGTERM and SIGINT handlers
   */
  register(): void {
    if (this.registered) return;
    this.registered = true;

    const handler = (signal: string) => {
      this.logger.info({ signal }, 'Shutdown signal received; starting graceful shutdown');
      void this.shutdown();
    };

    process.on('SIGTERM', () => handler('SIGTERM'));
    process.on('SIGINT', () => handler('SIGINT'));

    this.logger.info('ShutdownHandler registered for SIGTERM and SIGINT');
  }

  /**
   * Execute the graceful shutdown sequence.
   * Sets a 5-second hard-kill timer, calls deregister(), then exits 0.
   */
  async shutdown(): Promise<void> {
    // Set hard-kill timer
    const hardKillTimer = this._setTimeout(() => {
      this.logger.error('Shutdown deadline exceeded (5s); forcing exit with code 1');
      process.exit(1);
    }, SHUTDOWN_DEADLINE_MS);

    try {
      this.logger.info('Calling RoonExtension.deregister()');
      await this.deregister();
      this.logger.info('Deregistration complete');
    } catch (e) {
      this.logger.warn({ err: e }, 'Error during deregistration; continuing shutdown');
    }

    this._clearTimeout(hardKillTimer);
    this.logger.info('Graceful shutdown complete; exiting with code 0');
    process.exit(0);
  }
}
