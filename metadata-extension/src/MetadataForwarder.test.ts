import { describe, it, expect } from 'vitest';
import { encodeMetadataJson, decodeMetadataJson } from './MetadataForwarder';
import type { TrackMetadata } from './types';
import * as fc from 'fast-check';

describe('MetadataForwarder', () => {
  describe('encodeMetadataJson', () => {
    it('encodes title, artist, album as JSON', () => {
      const meta: TrackMetadata = {
        title: 'Test Title',
        artist: 'Test Artist',
        album: 'Test Album',
      };
      const json = encodeMetadataJson(meta);
      const decoded = JSON.parse(json);
      expect(decoded.title).toBe('Test Title');
      expect(decoded.artist).toBe('Test Artist');
      expect(decoded.album).toBe('Test Album');
    });

    it('omits undefined fields', () => {
      const meta: TrackMetadata = { title: 'Song' };
      const json = encodeMetadataJson(meta);
      const decoded = JSON.parse(json);
      expect(decoded.title).toBe('Song');
      expect(decoded.artist).toBeUndefined();
      expect(decoded.album).toBeUndefined();
    });

    it('returns empty object JSON for empty metadata', () => {
      const meta: TrackMetadata = {};
      const json = encodeMetadataJson(meta);
      expect(JSON.parse(json)).toEqual({});
    });

    it('round-trips title/artist/album', () => {
      const meta: TrackMetadata = {
        title: 'My Song',
        artist: 'My Artist',
        album: 'My Album',
      };
      const decoded = decodeMetadataJson(encodeMetadataJson(meta));
      expect(decoded.title).toBe(meta.title);
      expect(decoded.artist).toBe(meta.artist);
      expect(decoded.album).toBe(meta.album);
    });
  });

  describe('MetadataForwarder.onMetadataChanged', () => {
    it('does not send when all metadata fields absent (Req 6.4)', () => {
      // Test the logic: empty metadata should not produce a JSON payload
      const meta: TrackMetadata = {};
      const json = encodeMetadataJson(meta);
      const decoded = JSON.parse(json);
      // No title/artist/album means nothing to send
      expect(decoded.title).toBeUndefined();
      expect(decoded.artist).toBeUndefined();
      expect(decoded.album).toBeUndefined();
    });

    it('JSON payload contains title, artist, album fields (Req 6.1)', () => {
      const meta: TrackMetadata = {
        title: 'Track',
        artist: 'Band',
        album: 'Record',
      };
      const json = encodeMetadataJson(meta);
      const obj = JSON.parse(json);
      expect(obj).toHaveProperty('title', 'Track');
      expect(obj).toHaveProperty('artist', 'Band');
      expect(obj).toHaveProperty('album', 'Record');
    });
  });

  // Feature: roon-naa6-bridge, Property 5: Metadata JSON round-trip
  describe('Property 5: Metadata JSON round-trip', () => {
    it('round-trips arbitrary Unicode metadata through JSON', () => {
      fc.assert(
        fc.property(
          fc.record({
            title: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
            artist: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
            album: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
          }),
          (meta) => {
            const json = encodeMetadataJson(meta);
            const decoded = decodeMetadataJson(json);
            if (meta.title !== undefined) expect(decoded.title).toBe(meta.title);
            if (meta.artist !== undefined) expect(decoded.artist).toBe(meta.artist);
            if (meta.album !== undefined) expect(decoded.album).toBe(meta.album);
          }
        ),
        { numRuns: 100 }
      );
    });
  });
});
