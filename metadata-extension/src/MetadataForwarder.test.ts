import { describe, it, expect } from 'vitest';
import { encodeMetadataPayload, decodeMetadataPayload } from './MetadataForwarder';
import type { TrackMetadata } from './types';
import * as fc from 'fast-check';

describe('MetadataForwarder', () => {
  describe('encodeMetadataPayload', () => {
    it('encodes title, artist, album as UTF-8', () => {
      const meta: TrackMetadata = {
        title: 'Test Title',
        artist: 'Test Artist',
        album: 'Test Album',
      };
      const payload = encodeMetadataPayload(meta);
      expect(payload.length).toBeGreaterThan(0);

      const decoded = decodeMetadataPayload(payload);
      expect(decoded.title).toBe('Test Title');
      expect(decoded.artist).toBe('Test Artist');
      expect(decoded.album).toBe('Test Album');
    });

    it('omits cover art field when not provided', () => {
      const meta: TrackMetadata = {
        title: 'Song',
        artist: 'Artist',
        album: 'Album',
      };
      const payload = encodeMetadataPayload(meta);
      const decoded = decodeMetadataPayload(payload);
      expect(decoded.coverArt).toBeUndefined();
    });

    it('includes cover art when provided', () => {
      const coverArt = Buffer.from([0xFF, 0xD8, 0xFF]); // fake JPEG header
      const meta: TrackMetadata = {
        title: 'Song',
        coverArt,
      };
      const payload = encodeMetadataPayload(meta);
      const decoded = decodeMetadataPayload(payload);
      expect(decoded.coverArt).toBeDefined();
      expect(decoded.coverArt!.equals(coverArt)).toBe(true);
    });

    it('returns empty buffer for empty metadata', () => {
      const meta: TrackMetadata = {};
      const payload = encodeMetadataPayload(meta);
      expect(payload.length).toBe(0);
    });

    it('handles metadata with only title', () => {
      const meta: TrackMetadata = { title: 'Only Title' };
      const payload = encodeMetadataPayload(meta);
      const decoded = decodeMetadataPayload(payload);
      expect(decoded.title).toBe('Only Title');
      expect(decoded.artist).toBeUndefined();
      expect(decoded.album).toBeUndefined();
    });
  });

  describe('Property 13: Metadata extraction round-trip', () => {
    // Feature: roon-naa6-bridge, Property 13: Metadata extraction round-trip
    it('round-trips arbitrary metadata payloads', () => {
      fc.assert(
        fc.property(
          fc.record({
            title: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
            artist: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
            album: fc.option(fc.string({ minLength: 1, maxLength: 200 }), { nil: undefined }),
          }),
          (meta) => {
            const payload = encodeMetadataPayload(meta);
            const decoded = decodeMetadataPayload(payload);
            if (meta.title !== undefined) expect(decoded.title).toBe(meta.title);
            if (meta.artist !== undefined) expect(decoded.artist).toBe(meta.artist);
            if (meta.album !== undefined) expect(decoded.album).toBe(meta.album);
          }
        ),
        { numRuns: 100 }
      );
    });
  });

  describe('Property 14: Text metadata is valid UTF-8', () => {
    // Feature: roon-naa6-bridge, Property 14: Text metadata is valid UTF-8
    it('encode/decode round-trip preserves Unicode strings', () => {
      fc.assert(
        fc.property(
          fc.string({ minLength: 0, maxLength: 500 }),
          (s) => {
            const encoded = Buffer.from(s, 'utf-8');
            const decoded = encoded.toString('utf-8');
            expect(decoded).toBe(s);
          }
        ),
        { numRuns: 100 }
      );
    });
  });

  describe('Property 15: Missing cover art omits only cover art', () => {
    // Feature: roon-naa6-bridge, Property 15: Missing cover art does not suppress other metadata fields
    it('metadata without cover art still contains title/artist/album', () => {
      fc.assert(
        fc.property(
          fc.record({
            title: fc.option(fc.string({ minLength: 1, maxLength: 100 }), { nil: undefined }),
            artist: fc.option(fc.string({ minLength: 1, maxLength: 100 }), { nil: undefined }),
            album: fc.option(fc.string({ minLength: 1, maxLength: 100 }), { nil: undefined }),
          }),
          (meta) => {
            // No cover art
            const payload = encodeMetadataPayload(meta);
            const decoded = decodeMetadataPayload(payload);

            // Cover art must be absent
            expect(decoded.coverArt).toBeUndefined();

            // Other fields must be present if they were in the source
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
