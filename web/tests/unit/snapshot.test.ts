// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, it, expect } from 'vitest';
import {
  clampIndex,
  decodeSnapshot,
  encodeSnapshot,
  SNAPSHOT_VERSION,
} from '../../player-core/src/snapshot';

/** A v1 payload exactly as PlayerController.persist() wrote it before the
 *  codec extraction (web QueueItems persisted verbatim). Items carry
 *  MODERN identity (`id` + `source`) because the restorer requires it:
 *  snapshots persisted by pre-c5bf447 bundles hold bare `{track,...}`
 *  shapes and are now rejected wholesale instead of restored (see the
 *  pre-modern rejection test below). Everything non-identity here is kept
 *  verbatim from that era. */
const V1_RAW = JSON.stringify({
  v: 1,
  items: [
    {
      id: 't1',
      source: { kind: 'http-range', url: '/api/v1/tracks/1/audio', byteSize: 1000 },
      track: {
        id: 1,
        title: 'T1',
        artists: [],
        duration: 10,
        codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
        audio: { id: 101, size: 1000, url: '/api/v1/tracks/1/audio' },
      },
      releaseId: 9,
      albumId: 1,
      albumTitle: 'A',
      artist: 'Artist',
    },
    {
      id: 't2',
      source: { kind: 'http-range', url: '/api/v1/tracks/2/audio', byteSize: 1000 },
      track: { id: 2, audio: { url: '/api/v1/tracks/2/audio' } },
      releaseId: 9,
      albumId: 1,
      albumTitle: 'A',
      artist: 'Artist',
    },
  ],
  index: 1,
  positionSeconds: 5.5,
  volume: 0.8,
  normalizeMode: 'album',
});

describe('snapshot codec (v1 compatibility)', () => {
  it('decodes a real v1 payload and clamps the index into range', () => {
    const s = decodeSnapshot(V1_RAW);
    expect(s).not.toBeNull();
    expect(s!.v).toBe(SNAPSHOT_VERSION); // normalized to the current version
    expect(s!.items).toHaveLength(2);
    expect(s!.index).toBe(1);
    expect(s!.positionSeconds).toBe(5.5);
    expect(s!.volume).toBe(0.8);
    expect(s!.normalizeMode).toBe('album');
    // v1 carries no policy: decode applies the documented defaults
    expect(s!.repeat).toBe('off');
    expect(s!.shuffle).toBe(false);
    // richer item fields survive verbatim
    expect((s!.items[1] as Record<string, unknown>).releaseId).toBe(9);
  });

  it('decodes a v2 payload and preserves its policy', () => {
    const v2 = JSON.stringify({
      v: 2,
      items: [{ id: 't7', source: { kind: 'http-range', url: '/x', byteSize: 1 }, track: { id: 7, audio: { url: '/api/v1/tracks/7/audio' }, duration: 20 } }],
      index: 0,
      positionSeconds: 3,
      volume: 0.9,
      normalizeMode: 'track',
      repeat: 'all',
      shuffle: true,
    });
    const s = decodeSnapshot(v2);
    expect(s!.v).toBe(2);
    expect(s!.repeat).toBe('all');
    expect(s!.shuffle).toBe(true);
    // invalid policy values fall back to defaults
    const bad = decodeSnapshot(
      JSON.stringify({ v: 2, items: [{ id: 't7', source: { kind: 'http-range', url: '/x' }, track: { id: 7, audio: { url: '/x' } } }], index: 0, positionSeconds: 0, volume: 1, normalizeMode: 'off', repeat: 'sometimes', shuffle: 'yes' }),
    );
    expect(bad!.repeat).toBe('off');
    expect(bad!.shuffle).toBe(false);
  });

  it('rejects corrupt JSON, wrong versions, and non-array items', () => {
    expect(decodeSnapshot(null)).toBeNull();
    expect(decodeSnapshot('')).toBeNull();
    expect(decodeSnapshot('not json {')).toBeNull();
    expect(decodeSnapshot(JSON.stringify({ v: 2, items: [] }))).toBeNull();
    expect(decodeSnapshot(JSON.stringify({ v: SNAPSHOT_VERSION, items: {} }))).toBeNull();
  });

  // Restoration eligibility (post duration-repair hardening): items must
  // carry MODERN identity (id + source.url), not merely a track reference.
  // Pre-c5bf447 bundles persisted bare {track,...} shapes; restoring those
  // produced unplayable queue entries that the next persist then clobbered
  // good storage with. Their payload now drops wholesale (no restore).
  const PRE_MODERN_BARE = JSON.stringify({
    v: 1,
    items: [
      {
        track: {
          id: 1,
          title: 'T1',
          artists: [],
          duration: 10,
          codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
          audio: { id: 101, size: 1000, url: '/api/v1/tracks/1/audio' },
        },
        releaseId: 9,
        albumId: 1,
        albumTitle: 'A',
        artist: 'Artist',
      },
      {
        track: { id: 2, audio: { url: '/api/v1/tracks/2/audio' } },
        releaseId: 9,
        albumId: 1,
        albumTitle: 'A',
        artist: 'Artist',
      },
    ],
    index: 1,
    positionSeconds: 5.5,
    volume: 0.8,
    normalizeMode: 'album',
  });

  it('rejects pre-modern bare item shapes instead of restoring unplayable entries', () => {
    expect(decodeSnapshot(PRE_MODERN_BARE)).toBeNull();
  });

  it('keeps modern-shaped entries from mixed payloads, dropping only bare ones', () => {
    const mixed = JSON.parse(PRE_MODERN_BARE) as { items: unknown[] };
    mixed.items[1] = {
      id: 't2',
      source: { kind: 'http-range', url: '/api/v1/tracks/2/audio', byteSize: 100 },
      track: { id: 2, audio: { url: '/api/v1/tracks/2/audio' }, duration: 20 },
    };
    const s = decodeSnapshot(JSON.stringify(mixed))!;
    expect(s).not.toBeNull();
    expect(s.items).toHaveLength(1);
    // The sole survivor is the modern-identity entry, kept verbatim.
    const survivor = s.items[0] as Record<string, unknown>;
    expect(survivor.id).toBe('t2');
    expect((survivor.track as { id?: number } | undefined)?.id).toBe(2);
  });

  it('filters out entries without a usable identity and drops empty results', () => {
    const broken = JSON.stringify({
      v: 1,
      items: [
        { releaseId: 9 }, // no track
        { track: { id: 3, audio: {} } }, // no audio url
        { track: { audio: { url: '/x' } } }, // no numeric id
      ],
      index: 0,
      positionSeconds: 0,
    });
    expect(decodeSnapshot(broken)).toBeNull(); // nothing restorable

    const partial = decodeSnapshot(
      JSON.stringify({
        v: 1,
        items: [{ id: 't7', source: { kind: 'http-range', url: '/api/v1/tracks/7/audio', byteSize: 10 }, track: { id: 7, audio: { url: '/api/v1/tracks/7/audio' }, duration: 15 } }],
        index: 99, // out of range
        positionSeconds: -4,
        volume: 5,
      }),
    );
    expect(partial).not.toBeNull();
    expect(partial!.index).toBe(0); // clamped
  });

  it('round-trips through encodeSnapshot', () => {
    const s = decodeSnapshot(V1_RAW)!;
    expect(decodeSnapshot(encodeSnapshot(s))).toEqual(s);
  });
});

describe('clampIndex', () => {
  it('mirrors the historical restore() clamp', () => {
    expect(clampIndex(0, 3)).toBe(0);
    expect(clampIndex(2, 3)).toBe(2);
    expect(clampIndex(99, 3)).toBe(2);
    expect(clampIndex(-5, 3)).toBe(0);
    expect(clampIndex(NaN, 3)).toBe(0); // Math.floor(NaN)||0 -> 0
    expect(clampIndex(1.7, 3)).toBe(1); // floored
    expect(clampIndex(0, 0)).toBe(0); // degenerate, stays in range
  });
});
