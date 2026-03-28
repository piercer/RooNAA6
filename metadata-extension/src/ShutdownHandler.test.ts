import { describe, it, expect, vi } from 'vitest';
import { ShutdownHandler } from './ShutdownHandler';
import pino from 'pino';

function makeLogger() {
  return pino({ level: 'silent' });
}

describe('ShutdownHandler', () => {
  it('registers SIGTERM and SIGINT handlers', () => {
    const onSpy = vi.spyOn(process, 'on');
    const logger = makeLogger();
    const deregister = vi.fn().mockResolvedValue(undefined);
    const handler = new ShutdownHandler(logger, deregister);
    handler.register();

    const calls = onSpy.mock.calls.map(c => c[0]);
    expect(calls).toContain('SIGTERM');
    expect(calls).toContain('SIGINT');

    onSpy.mockRestore();
  });

  it('calls deregister during shutdown', async () => {
    const logger = makeLogger();
    const deregister = vi.fn().mockResolvedValue(undefined);
    const exitSpy = vi.spyOn(process, 'exit').mockImplementation(() => { throw new Error('exit'); });

    const handler = new ShutdownHandler(logger, deregister);

    await expect(handler.shutdown()).rejects.toThrow('exit');
    expect(deregister).toHaveBeenCalledOnce();

    exitSpy.mockRestore();
  });

  it('force-exits with code 1 when 5-second timeout exceeded', async () => {
    const logger = makeLogger();
    // deregister that never resolves
    const deregister = vi.fn(() => new Promise<void>(() => {}));
    const exitSpy = vi.spyOn(process, 'exit').mockImplementation((() => {}) as () => never);

    let timerCallback: (() => void) | null = null;
    const fakeSetTimeout = (cb: () => void, _ms: number) => {
      timerCallback = cb;
      return 999 as unknown as ReturnType<typeof setTimeout>;
    };
    const fakeClearTimeout = vi.fn();

    const handler = new ShutdownHandler(logger, deregister, {
      setTimeout: fakeSetTimeout as unknown as typeof setTimeout,
      clearTimeout: fakeClearTimeout as unknown as typeof clearTimeout,
    });

    // Start shutdown (don't await — deregister never resolves)
    void handler.shutdown();

    // Fire the hard-kill timer manually
    expect(timerCallback).not.toBeNull();
    timerCallback!();

    expect(exitSpy).toHaveBeenCalledWith(1);

    exitSpy.mockRestore();
  });

  it('exits with code 0 on successful shutdown', async () => {
    const logger = makeLogger();
    const deregister = vi.fn().mockResolvedValue(undefined);
    const exitSpy = vi.spyOn(process, 'exit').mockImplementation(() => { throw new Error('exit'); });

    const handler = new ShutdownHandler(logger, deregister);

    await expect(handler.shutdown()).rejects.toThrow('exit');
    expect(exitSpy).toHaveBeenCalledWith(0);

    exitSpy.mockRestore();
  });
});
