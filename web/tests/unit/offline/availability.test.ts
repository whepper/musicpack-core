// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Availability + local-first source-rule tests (plan §6/§7.4, decision D1).

import { describe, expect, it } from 'vitest';
import { createAvailability } from '../../../app/src/lib/offline/availability';
import { memoryCatalog } from '../../../app/src/lib/offline/stores';
import type { InstalledPackage } from '../../../app/src/lib/offline/types';
import { itemForTrack, type OfflineContext } from '../../../app/src/lib/state/queue';
import type { ReleaseDetail, Track } from '../../../app/src/lib/api/types';

function installedFixture(): InstalledPackage {
  return {
    releaseId: 7,
    status: 'installed',
    createdAt: 1,
    updatedAt: 1,
    bytes: 100,
    assets: [
      { key: 't.101.primary', kind: 'audio-primary', state: 'ok', trackId: 101, size: 10 },
      {
        key: 't.101.r.201',
        kind: 'audio-representation',
        state: 'ok',
        trackId: 101,
        representationId: 201,
        size: 40,
      },
      // damaged alternate: excluded from availability
      {
        key: 't.102.r.202.dmg',
        kind: 'audio-representation',
        state: 'damaged',
        trackId: 102,
        representationId: 202,
        size: 4,
      },
      // R3.6: one healthy lyric and one damaged lyric document.
      {
        key: 't.101.lyr.501',
        kind: 'lyrics',
        state: 'ok',
        trackId: 101,
        lyricsId: 501,
        size: 12,
      },
      {
        key: 't.101.lyr.502.dmg',
        kind: 'lyrics',
        state: 'damaged',
        trackId: 101,
        lyricsId: 502,
        size: 8,
      },
    ],
    releaseDetail: { id: 7 } as unknown as InstalledPackage['releaseDetail'],
  };
}

function releaseDetailFixture(reps = true): ReleaseDetail {
  return {
    id: 7,
    album: { id: 3, title: 'Album', artists: [{ id: 1, name: 'Artist' }] },
    edition: 'Edition',
    media: [
      {
        disc: 1,
        tracks: [
          {
            id: 101,
            number: 1,
            title: 'One',
            artists: [],
            codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
            audio: { id: 1, size: 1000, url: '/api/v1/tracks/101/audio' },
            representations: reps
              ? [
                  {
                    id: 201,
                    size: 5000,
                    url: '/api/v1/tracks/101/representations/201/audio',
                    codec: { codec: 'flac', mimeType: 'audio/flac' },
                  },
                ]
              : undefined,
          },
          {
            id: 102,
            number: 2,
            title: 'Two',
            artists: [],
            codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
            audio: { id: 2, size: 2000, url: '/api/v1/tracks/102/audio' },
          },
        ],
      },
    ],
    artwork: [],
    assets: [],
  } as unknown as ReleaseDetail;
}

describe('offline availability', () => {
  it('allows locally held primary and healthy representations (D1)', async () => {
    const catalog = memoryCatalog();
    await catalog.putPackage(installedFixture());
    const av = createAvailability(catalog);
    await av.hydrate();
    expect(av.allows(101, { kind: 'primary' })).toBe(true);
    expect(av.allows(101, { kind: 'representation', representationId: 201 })).toBe(true);
    expect(av.allows(999, { kind: 'primary' })).toBe(false);
  });

  it('excludes damaged assets from availability', async () => {
    const catalog = memoryCatalog();
    await catalog.putPackage(installedFixture());
    const av = createAvailability(catalog);
    await av.hydrate();
    expect(av.allows(102, { kind: 'representation', representationId: 202 })).toBe(false);
  });

  it('resolves local keys matching availability', async () => {
    const catalog = memoryCatalog();
    await catalog.putPackage(installedFixture());
    const av = createAvailability(catalog);
    await av.hydrate();
    expect(av.localKeyFor(101, { kind: 'primary' })).toBe('t.101.primary');
    expect(av.localKeyFor(101, { kind: 'representation', representationId: 201 })).toBe('t.101.r.201');
    expect(av.localKeyFor(102, { kind: 'representation', representationId: 202 })).toBeNull();
  });

  it('resolves lyric keys only for healthy committed documents (R3.6)', async () => {
    const catalog = memoryCatalog();
    await catalog.putPackage(installedFixture());
    const av = createAvailability(catalog);
    await av.hydrate();
    expect(av.lyricKeyFor(101, 501)).toBe('t.101.lyr.501');
    // Damaged lyrics have no addressable bytes…
    expect(av.lyricKeyFor(101, 502)).toBeNull();
    // …and neither do lyric-less tracks or uninstalled releases.
    expect(av.lyricKeyFor(102, 501)).toBeNull();
    expect(av.lyricKeyFor(999, 501)).toBeNull();
  });

  it('maps installed releases to their album ids for the shelf', async () => {
    const catalog = memoryCatalog();
    // Empty catalog: no albums.
    const empty = createAvailability(catalog);
    await empty.hydrate();
    expect(empty.installedAlbumIds().size).toBe(0);

    const pkg = installedFixture();
    (pkg.releaseDetail as unknown as { album: { id: number } }).album = { id: 55 };
    await catalog.putPackage(pkg);
    const av = createAvailability(catalog);
    await av.hydrate();
    expect(av.installedAlbumIds()).toEqual(new Set([55]));

    // A second release of the SAME album must not duplicate the id.
    const pkg2: InstalledPackage = {
      ...pkg,
      releaseId: 8,
      releaseDetail: { ...pkg.releaseDetail, id: 8 },
    };
    await catalog.putPackage(pkg2);
    const av2 = createAvailability(catalog);
    await av2.hydrate();
    expect(av2.installedAlbumIds()).toEqual(new Set([55]));
  });
});

describe('itemForTrack local-first source rule (D1)', () => {
  const offlineCtx: OfflineContext = {
    localKeyFor: (trackId, candidate) => {
      if (trackId === 101 && candidate.kind === 'primary') return 't.101.primary';
      if (trackId === 101 && candidate.kind === 'representation') return `t.101.r.${candidate.id}`;
      return null; // track 102 not installed
    },
  };

  it('emits a local-file source for the selected candidate when installed', () => {
    // Default preference ⇒ primary is chosen; it is locally held (D1).
    const rel = releaseDetailFixture(false);
    const track = rel.media[0]!.tracks[0]!;
    const item = itemForTrack(rel, track, 'Album', 'Artist', { offline: offlineCtx });
    expect(item.source.kind).toBe('local-file');
    expect(item.source.url).toBe('t.101.primary');
  });

  it('keeps remote sources for non-installed tracks (mixed queue)', () => {
    const rel = releaseDetailFixture(false);
    const t2 = rel.media[0]!.tracks[1]!;
    const item = itemForTrack(rel, t2, 'Album', 'Artist', { offline: offlineCtx });
    expect(item.source.kind).toBe('http-range');
    expect(item.source.url).toBe('/api/v1/tracks/102/audio');
  });

  it('is byte-identical online-only when no offline context is given', () => {
    const rel = releaseDetailFixture(false);
    const t1 = rel.media[0]!.tracks[0]!;
    const item = itemForTrack(rel, t1, 'Album', 'Artist', {});
    expect(item.source).toEqual({
      kind: 'http-range',
      url: '/api/v1/tracks/101/audio',
      byteSize: 1000,
    });
    expect(item.id).toBe('t101');
  });

  it('local-first applies to whichever candidate selection chose', () => {
    // With a codec=flac preference AND installation of that rep, source is local.
    const rel = releaseDetailFixture(true);
    const track = rel.media[0]!.tracks[0]!;
    const item = itemForTrack(rel, track, 'Album', 'Artist', {
      preference: { mode: 'codec', codec: 'flac' },
      canPlay: () => true,
      offline: offlineCtx,
    });
    expect(item.representationId).toBe(201);
    expect(item.source.kind).toBe('local-file');
    expect(item.source.url).toBe('t.101.r.201');
    expect(item.id).toBe('t101r201');
  });

  it('uninstalled candidates stay remote even under an offline context', () => {
    const rel = releaseDetailFixture(true);
    const track = rel.media[0]!.tracks[0]!;
    const none: OfflineContext = { localKeyFor: () => null };
    const item = itemForTrack(rel, track, 'Album', 'Artist', {
      preference: { mode: 'lossless' },
      canPlay: () => true,
      offline: none,
    });
    expect(item.source.kind).toBe('http-range');
    expect(item.source.url).toContain('/representations/');
  });
});
