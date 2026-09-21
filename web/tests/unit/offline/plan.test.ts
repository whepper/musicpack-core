// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it } from 'vitest';
import { planReleaseAssets } from '../../../app/src/lib/offline/plan';

function releaseFixture() {
  return {
    id: 7,
    media: [
      {
        disc: 1,
        tracks: [
          {
            id: 101,
            audio: { url: '/api/v1/tracks/101/audio', size: 1000, sha256: 'a'.repeat(64) },
            representations: [
              {
                id: 201,
                url: '/api/v1/tracks/101/representations/201/audio',
                size: 5000,
                sha256: 'b'.repeat(64),
              },
              {
                // Zero-size alternates cannot be staged (no declared byte
                // budget); the planner skips them.
                id: 202,
                url: '/api/v1/tracks/101/representations/202/audio',
                size: 0,
                sha256: 'c'.repeat(64),
              },
            ],
            waveform: { url: '/api/v1/tracks/101/waveform', points: 480, sha256: 'd'.repeat(64) },
          },
          {
            id: 102,
            audio: { url: '/api/v1/tracks/102/audio', size: 2000 },
            waveform: null,
          },
        ],
      },
    ],
    artwork: [{ id: 301, url: '/api/v1/assets/301', mimeType: 'image/jpeg' }],
  };
}

describe('planReleaseAssets (v1 download policy)', () => {
  it('plans primaries + all usable representations + waveforms + artwork', () => {
    const plan = planReleaseAssets(releaseFixture());
    // The zero-size alternate is deliberately absent (skipped by the planner).
    const keys = plan.map((a) => a.key);
    expect(keys).toEqual([
      't.101.primary',
      't.101.r.201',
      't.101.w',
      't.102.primary',
      'art.301',
    ]);
  });

  it('carries hashes and sizes for verification', () => {
    const plan = planReleaseAssets(releaseFixture());
    const rep = plan.find((a) => a.key === 't.101.r.201')!;
    expect(rep.sha256).toBe('b'.repeat(64));
    expect(rep.size).toBe(5000);
    const wf = plan.find((a) => a.key === 't.101.w')!;
    // peak-rms-u8: 2 bytes per bucket
    expect(wf.size).toBe(960);
  });

  it('skips zero-size assets but keeps undeclared-size artwork', () => {
    const plan = planReleaseAssets(releaseFixture());
    expect(plan.find((a) => a.key === 't.101.r.202')).toBeUndefined();
    expect(plan.find((a) => a.key === 'art.301')!.size).toBe(0);
  });
  it('handles releases without representations or waveforms (pre-Phase-3 packages)', () => {
    const rel = releaseFixture();
    rel.media[0]!.tracks[0]!.representations = [];
    rel.media[0]!.tracks[0]!.waveform = null;
    const keys = planReleaseAssets(rel).map((a) => a.key);
    expect(keys).toEqual(['t.101.primary', 't.102.primary', 'art.301']);
  });
});

// ---- track-linked lyrics (R3.6, docs/musicpack-lyrics-v1.md §7.4) --------

/** The release fixture plus a flat `(trackId, id)`-ordered lyric index. */
function lyricsFixture(
  entries: Array<{ trackId: number; id: number; size: number; sha256?: string; lang?: string }> = [
    { trackId: 101, id: 501, size: 42, sha256: 'e'.repeat(64), lang: 'en' },
    { trackId: 101, id: 503, size: 17, sha256: 'f'.repeat(64) },
    { trackId: 102, id: 502, size: 8, sha256: 'a'.repeat(64), lang: 'de' },
  ],
) {
  return {
    ...releaseFixture(),
    trackLyrics: entries.map((e) => ({
      url: `/api/v1/assets/${e.id}`,
      ...e,
    })),
  };
}

describe('planReleaseAssets — track lyrics (R3.6)', () => {
  it('a release without the index plans exactly as before (no lyric requests)', () => {
    const before = planReleaseAssets(releaseFixture());
    const absent = planReleaseAssets({ ...releaseFixture(), trackLyrics: undefined });
    const empty = planReleaseAssets({ ...releaseFixture(), trackLyrics: [] });
    const keys = before.map((a) => a.key);
    expect(keys).toEqual(['t.101.primary', 't.101.r.201', 't.101.w', 't.102.primary', 'art.301']);
    expect(absent.map((a) => a.key)).toEqual(keys);
    expect(empty.map((a) => a.key)).toEqual(keys);
    expect(planReleaseAssets(lyricsFixture()).some((a) => a.kind === 'lyrics')).toBe(true);
  });

  it('keys lyrics by (trackId, assetId) with server values, in index order', () => {
    const plan = planReleaseAssets(lyricsFixture());
    const keys = plan.filter((a) => a.kind === 'lyrics').map((a) => a.key);
    expect(keys).toEqual(['t.101.lyr.501', 't.101.lyr.503', 't.102.lyr.502']);
    const first = plan.find((a) => a.key === 't.101.lyr.501')!;
    expect(first).toMatchObject({
      kind: 'lyrics',
      trackId: 101,
      lyricsId: 501,
      url: '/api/v1/assets/501',
      size: 42,
      sha256: 'e'.repeat(64),
    });
    // Lyrics stage after the per-track assets and before artwork; the
    // index order is preserved.
    expect(plan.map((a) => a.key)).toEqual([
      't.101.primary',
      't.101.r.201',
      't.101.w',
      't.102.primary',
      't.101.lyr.501',
      't.101.lyr.503',
      't.102.lyr.502',
      'art.301',
    ]);
  });

  it('never collides lyrics across tracks or within one track', () => {
    const plan = planReleaseAssets(lyricsFixture());
    const lyricKeys = plan.filter((a) => a.kind === 'lyrics').map((a) => a.key);
    expect(new Set(lyricKeys).size).toBe(lyricKeys.length);
  });

  it('keeps keys stable when ordering, neighbours or language change', () => {
    const base = lyricsFixture();
    const shuffled = lyricsFixture([
      { trackId: 102, id: 502, size: 8, sha256: 'a'.repeat(64), lang: 'de' },
      { trackId: 101, id: 503, size: 17, sha256: 'f'.repeat(64) },
      { trackId: 101, id: 501, size: 42, sha256: 'e'.repeat(64), lang: 'nl' },
    ]);
    const keyOf = (rel: ReturnType<typeof lyricsFixture>) =>
      planReleaseAssets(rel)
        .filter((a) => a.kind === 'lyrics')
        .map((a) => `${a.key}|${a.sha256}|${a.size}`)
        .sort();
    // Array position, neighbours and language tags carry no identity: the
    // keyed, hashed asset set is identical.
    expect(keyOf(shuffled)).toEqual(keyOf(base));
  });

  it('re-plans a replaced lyric under the same key with the new hash', () => {
    const replaced = lyricsFixture([
      { trackId: 101, id: 501, size: 99, sha256: '9'.repeat(64), lang: 'en' },
    ]);
    const asset = planReleaseAssets(replaced).find((a) => a.key === 't.101.lyr.501')!;
    expect(asset.sha256).toBe('9'.repeat(64));
    expect(asset.size).toBe(99);
  });

  it('drops a removed lyric from the new plan (no stale addressability)', () => {
    const without = lyricsFixture([
      { trackId: 101, id: 501, size: 42, sha256: 'e'.repeat(64), lang: 'en' },
    ]);
    const keys = planReleaseAssets(without).map((a) => a.key);
    expect(keys).toContain('t.101.lyr.501');
    expect(keys).not.toContain('t.101.lyr.503');
    expect(keys).not.toContain('t.102.lyr.502');
  });

  it('skips a hashless entry but keeps a legal zero-byte document', () => {
    const plan = planReleaseAssets(
      lyricsFixture([
        { trackId: 101, id: 501, size: 42, lang: 'en' }, // no authoritative hash → unstageable
        { trackId: 101, id: 503, size: 0, sha256: 'f'.repeat(64) }, // empty document: legal
      ]),
    );
    const keys = plan.filter((a) => a.kind === 'lyrics').map((a) => a.key);
    expect(keys).toEqual(['t.101.lyr.503']);
    expect(plan.find((a) => a.key === 't.101.lyr.503')!.size).toBe(0);
  });

  it('carries no language onto the asset record (it lives on the release index)', () => {
    const plan = planReleaseAssets(lyricsFixture());
    for (const asset of plan.filter((a) => a.kind === 'lyrics')) {
      expect(Object.keys(asset)).not.toContain('lang');
    }
  });
});
