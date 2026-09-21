// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Unit tests for the pure derived-facts modules the album and track pages
// render from (R2 god-component refactor). The fixtures mirror the API v1
// payloads; nothing here touches stores, the network, or the DOM.

import { describe, expect, it } from 'vitest';

import {
  albumNormalizationGain,
  heroArtwork,
  releaseFacts,
  waveformPeekTrack,
} from '../../app/src/lib/ui/album-facts';
import {
  representationRows,
  seekTimeline,
  trackFacts,
  trackNormalizationGains,
} from '../../app/src/lib/ui/track-facts';
import type { ReleaseDetail, Track, TrackDetail } from '../../app/src/lib/api/types';

function track(over: Partial<Track> = {}): Track {
  return {
    id: 1,
    number: 1,
    title: 'T1',
    artists: [{ id: 1, name: 'A' }],
    duration: 100,
    codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack', streamVersion: 8, sampleRate: 44100, channels: 2 },
    audio: { id: 1, size: 1000, url: '/x' },
    ...over,
  };
}

function release(over: Partial<ReleaseDetail> = {}): ReleaseDetail {
  return {
    id: 7,
    album: { id: 3, title: 'Album', artists: [{ id: 1, name: 'A' }] },
    media: [{ disc: 1, format: 'CD', tracks: [track()] }],
    artwork: [],
    assets: [],
    ...over,
  };
}

describe('releaseFacts', () => {
  it('counts tracks, discs, sizes and representations across media', () => {
    const rel = release({
      media: [
        {
          disc: 1,
          format: 'CD',
          tracks: [
            track({ id: 1, representations: [{ id: 9, size: 500, url: '/r', codec: { codec: 'flac', mimeType: 'audio/flac' }, label: 'FLAC' }] }),
            track({ id: 2, number: 2, title: 'T2', waveform: { version: 1, intervalMs: 100, encoding: 'peak-rms-u8', floorDb: -60, points: 10, url: '/w' } }),
          ],
        },
        { disc: 2, format: 'SACD', tracks: [track({ id: 3, number: 1, title: 'T3', audio: { id: 3, size: 2000, url: '/x3' } })] },
      ],
    });
    const f = releaseFacts(rel);
    expect(f.trackCount).toBe(3);
    expect(f.discCount).toBe(2);
    expect(f.allTracks.map((t) => t.id)).toEqual([1, 2, 3]);
    expect(f.totalDuration).toBe(300);
    expect(f.waveCount).toBe(1);
    expect(f.repCount).toBe(1);
    expect(f.primary?.id).toBe(1);
    expect(f.isMpc).toBe(true);
    expect(f.primaryTileSub).toBe('SV8');
    // 1000 + 1000 + 2000 primary, +500 representation (formatBytes rounds
    // to whole kB below 1 MB, matching the original page rendering).
    expect(f.totalSize).toBe('4 kB');
    expect(f.primarySize).toBe('4 kB');
    expect(f.mediums).toBe('CD, SACD');
    expect(f.repGroups).toEqual([
      { key: 'flac', title: 'FLAC', codec: 'flac', sub: expect.any(String) },
    ]);
  });

  it('handles the empty-release edge without inventing values', () => {
    const f = releaseFacts(null);
    expect(f.trackCount).toBe(0);
    expect(f.primary).toBeUndefined();
    expect(f.mediums).toBe('');
    expect(f.primaryTileSub).toBe('');
  });

  it('uses the quality line for non-musepack primaries', () => {
    const rel = release({
      media: [{ disc: 1, tracks: [track({ codec: { codec: 'flac', mimeType: 'audio/flac', sampleRate: 44100, channels: 2 } })] }],
    });
    expect(releaseFacts(rel).primaryTileSub).toContain('44.1 kHz');
  });

  it('collects ISRC rows in payload order', () => {
    const rel = release({
      media: [{
        disc: 1,
        tracks: [
          track({ id: 1, isrc: 'ISRC1' }),
          track({ id: 2, number: 2, title: 'T2' }),
          track({ id: 3, number: 3, title: 'T3', isrc: 'ISRC3' }),
        ],
      }],
    });
    expect(releaseFacts(rel).trackIsrcs).toEqual([
      { title: 'T1', isrc: 'ISRC1' },
      { title: 'T3', isrc: 'ISRC3' },
    ]);
  });
});

describe('heroArtwork + waveformPeekTrack + albumNormalizationGain', () => {
  it('prefers the front cover', () => {
    const artwork = [
      { id: 2, url: '/back', role: 'back' },
      { id: 1, url: '/front', role: 'front' },
    ];
    expect(heroArtwork(artwork)?.url).toBe('/front');
    expect(heroArtwork([{ id: 5, url: '/only' }])?.url).toBe('/only');
    expect(heroArtwork(undefined)).toBeUndefined();
  });

  it('peeks the playing track when it belongs to the release, else the first waveform', () => {
    const rel = release();
    const withWave = track({ id: 2, number: 2, title: 'T2', waveform: { version: 1, intervalMs: 100, encoding: 'e', floorDb: -60, points: 1, url: '/w' } });
    const rel2 = release({ media: [{ disc: 1, tracks: [track(), withWave] }] });
    expect(waveformPeekTrack(rel2, [track(), withWave], 1)?.id).toBe(1);
    expect(waveformPeekTrack(rel2, [track(), withWave], 99)?.id).toBe(2);
    expect(waveformPeekTrack(null, [], undefined)).toBeNull();
    const _ = rel;
  });

  it('computes the album gain preview only when loudness exists', () => {
    expect(albumNormalizationGain(release())).toBeNull();
    const rel = release({ loudness: { albumLufs: -10, albumTruePeakDb: 0.5, algorithm: 'bs1770' } });
    const gain = albumNormalizationGain(rel);
    expect(gain).not.toBeNull();
    expect(gain).toBeLessThanOrEqual(-6); // −16 LUFS target vs −10 LUFS program
  });
});

describe('trackFacts', () => {
  const detail: TrackDetail = {
    track: track({ id: 2, isrc: 'ISRC-X', artists: [{ id: 1, name: 'A' }, { id: 2, name: 'B', role: 'featuring' }] }),
    context: { disc: 1, albumId: 3, albumTitle: 'Album', releaseId: 7 },
  };
  const rel = release({ media: [{ disc: 1, title: 'The disc', tracks: [track({ id: 1 }), track({ id: 2, number: 2, title: 'T2', isrc: 'ISRC-X', representations: [{ id: 9, size: 1, url: '/r', codec: { codec: 'flac', mimeType: 'audio/flac' } }] })] }] });

  it('locates the track within the release and its disc', () => {
    const f = trackFacts(detail, rel);
    expect(f.trackIndexInRelease).toBe(1);
    expect(f.trackDisc?.title).toBe('The disc');
    expect(f.positionLine).toEqual({ disc: 1, of: 1, pos: 2, of2: 2, title: 'The disc' });
    expect(f.leadArtist?.name).toBe('A');
    expect(f.identifiers).toEqual([['ISRC', 'ISRC-X']]);
    expect(f.repCount).toBe(1);
  });

  it('reports an unknown index and null position without a release', () => {
    const f = trackFacts(detail, null);
    expect(f.trackIndexInRelease).toBe(-1);
    expect(f.positionLine).toBeNull();
    expect(f.trackDisc).toBeNull();
  });
});

describe('representationRows', () => {
  it('lists the primary first, then labelled alternates', () => {
    const t = track({
      representations: [{ id: 9, size: 500, url: '/r', codec: { codec: 'flac', mimeType: 'audio/flac', sampleRate: 44100, channels: 2 }, label: 'FLAC' }],
    });
    const rows = representationRows(t);
    expect(rows.map((r) => r.id)).toEqual(['primary', '9']);
    expect(rows[0]?.label).toBe('Musepack');
    expect(rows[0]?.sub).toBe('SV8');
    expect(rows[1]?.label).toBe('FLAC');
    expect(rows[1]?.size).toBe(500);
  });

  it('is empty without a track', () => {
    expect(representationRows(null)).toEqual([]);
  });
});

describe('seekTimeline', () => {
  const t = track({ duration: 100 });

  it('clamps the position into the track and computes the fill', () => {
    const s = seekTimeline(
      { currentTrackId: 1, currentTrackStartSeconds: 50, currentTrackDurationSeconds: 100, positionSeconds: 100 },
      t,
    );
    expect(s.isCurrent).toBe(true);
    expect(s.trackStart).toBe(50);
    expect(s.withinPos).toBe(50);
    expect(s.rangeFill).toBe(50);
  });

  it('falls back to the API duration for non-current tracks', () => {
    // Replicates the original page formula verbatim: for a non-current
    // track the album-absolute position is clamped against the API
    // duration (position 5000 → full bar), which is the historical
    // rendering of an idle track page's seek input.
    const s = seekTimeline(
      { currentTrackId: 99, currentTrackStartSeconds: 0, currentTrackDurationSeconds: 0, positionSeconds: 5000 },
      t,
    );
    expect(s.isCurrent).toBe(false);
    expect(s.trackDur).toBe(100);
    expect(s.withinPos).toBe(100);
    expect(s.rangeFill).toBe(100);
  });

  it('fills nothing when the duration is unknown', () => {
    const s = seekTimeline(
      { currentTrackId: 1, currentTrackStartSeconds: 0, currentTrackDurationSeconds: 0, positionSeconds: 10 },
      track({ duration: undefined }),
    );
    expect(s.rangeFill).toBe(0);
  });
});

describe('trackNormalizationGains', () => {
  it('returns nulls without measurements and gains with them', () => {
    expect(trackNormalizationGains(track(), release())).toEqual({ album: null, track: null });
    const rel = release({ loudness: { albumLufs: -16, albumTruePeakDb: -1, algorithm: 'bs1770' } });
    const g = trackNormalizationGains(track({ loudness: { lufs: -20, truePeakDb: -2 } }), rel);
    expect(g.track).toBeGreaterThan(0); // quieter than the target → positive gain
    expect(g.album).toBe(0); // already at target
  });
});
