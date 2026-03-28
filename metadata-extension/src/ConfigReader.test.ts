import { describe, it, expect, vi, beforeEach } from 'vitest';
import { loadConfig } from './ConfigReader';

// Mock fs module
vi.mock('node:fs', () => ({
  existsSync: vi.fn(() => false),
  readFileSync: vi.fn(() => '{}'),
}));

import { existsSync, readFileSync } from 'node:fs';

const mockExistsSync = vi.mocked(existsSync);
const mockReadFileSync = vi.mocked(readFileSync);

describe('ConfigReader', () => {
  const warnings: string[] = [];
  const errors: string[] = [];

  const warn = (msg: string) => { warnings.push(msg); };
  const errorAndExit = (msg: string): never => {
    errors.push(msg);
    throw new Error(`exit(1): ${msg}`);
  };

  beforeEach(() => {
    warnings.length = 0;
    errors.length = 0;
    mockExistsSync.mockReturnValue(false);
  });

  describe('missing config file', () => {
    it('uses defaults when no config file exists', () => {
      mockExistsSync.mockReturnValue(false);
      const config = loadConfig(warn, errorAndExit);

      expect(config.outputName).toBe('HQPlayer via NAA6');
      expect(config.alsaDevice).toBe('hw:Loopback,1,0');
      expect(config.hqplayerHost).toBe('127.0.0.1');
      expect(config.hqplayerPort).toBe(10700);
      expect(config.reconnectBackoff).toBe(5000);
      expect(config.logLevel).toBe('info');
      expect(config.ipcSocket).toBe('/run/roon-naa6-bridge/meta.sock');
    });

    it('logs a warning when no config file exists', () => {
      mockExistsSync.mockReturnValue(false);
      loadConfig(warn, errorAndExit);
      expect(warnings.some(w => w.includes('using defaults'))).toBe(true);
    });
  });

  describe('valid config file', () => {
    it('reads and parses a valid config file', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({
        outputName: 'My Bridge',
        hqplayerHost: '192.168.1.100',
        hqplayerPort: 10700,
        reconnectBackoff: 3000,
        logLevel: 'debug',
      }));

      const config = loadConfig(warn, errorAndExit);
      expect(config.outputName).toBe('My Bridge');
      expect(config.hqplayerHost).toBe('192.168.1.100');
      expect(config.logLevel).toBe('debug');
    });

    it('uses defaults for missing optional fields', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ outputName: 'Test' }));

      const config = loadConfig(warn, errorAndExit);
      expect(config.outputName).toBe('Test');
      expect(config.hqplayerPort).toBe(10700);
      expect(config.reconnectBackoff).toBe(5000);
    });

    it('returns a frozen config object', () => {
      mockExistsSync.mockReturnValue(false);
      const config = loadConfig(warn, errorAndExit);
      expect(Object.isFrozen(config)).toBe(true);
    });
  });

  describe('unknown keys', () => {
    it('warns on unknown keys and continues startup', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({
        outputName: 'Test',
        unknownKey: 'value',
        anotherUnknown: 42,
      }));

      const config = loadConfig(warn, errorAndExit);
      expect(config.outputName).toBe('Test');
      expect(warnings.some(w => w.includes('unknownKey'))).toBe(true);
      expect(warnings.some(w => w.includes('anotherUnknown'))).toBe(true);
    });
  });

  describe('invalid config values', () => {
    it('exits non-zero for hqplayerPort = 0', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ hqplayerPort: 0 }));

      expect(() => loadConfig(warn, errorAndExit)).toThrow('exit(1)');
      expect(errors.some(e => e.includes('hqplayerPort'))).toBe(true);
    });

    it('exits non-zero for hqplayerPort > 65535', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ hqplayerPort: 99999 }));

      expect(() => loadConfig(warn, errorAndExit)).toThrow('exit(1)');
      expect(errors.some(e => e.includes('hqplayerPort'))).toBe(true);
    });

    it('exits non-zero for reconnectBackoff = 0', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ reconnectBackoff: 0 }));

      expect(() => loadConfig(warn, errorAndExit)).toThrow('exit(1)');
      expect(errors.some(e => e.includes('reconnectBackoff'))).toBe(true);
    });

    it('exits non-zero for negative reconnectBackoff', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ reconnectBackoff: -100 }));

      expect(() => loadConfig(warn, errorAndExit)).toThrow('exit(1)');
    });

    it('exits non-zero for invalid logLevel', () => {
      mockExistsSync.mockReturnValue(true);
      mockReadFileSync.mockReturnValue(JSON.stringify({ logLevel: 'verbose' }));

      expect(() => loadConfig(warn, errorAndExit)).toThrow('exit(1)');
      expect(errors.some(e => e.includes('logLevel'))).toBe(true);
    });
  });
});
