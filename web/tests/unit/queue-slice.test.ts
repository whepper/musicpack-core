// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Stage 1 (vertical slice Phase D): locks the web-side contracts introduced
// with the Albums -> Album Detail -> Play slice.
//
// itemForTrack() is THE single QueueItem construction boundary; Domain Model
// Phase 3 will later introduce representation selection behind exactly this
// function. These tests pin today's behavior so that change stays safe.

import { describe, it, expect } from 'vitest';
import {
  createQueueStore,
  itemForTrack,
  itemsForRelease,
  tracksOfRelease,
} from '../../app/src/lib/state/queue';
import { codecLabel } from '../../app/src/lib/format';
import type { QueueItem } from '../../app/src/lib/state/queue';
import type { ReleaseDetail, Track } from '../../app/src/lib/api/types';

function track(id: number, number_: number, title: string, over: Partial<Track> = {}): Track {
  return {
    id,
    number: number_,
    title,
    artists: [],
    codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
    audio: { id, size: 1000 + id, url: `/api/v1/tracks/${id}/audio` },
    ...over,
  };
}

const t1 = track(11, 1, 'T1', {
  duration: 100,
  loudness: { lufs: -12.25, truePeakDb: -3 },
});
const t2 = track(12, 2, 'T2', {
  duration: 200,
  codec: { codec: 'flac', mimeType: 'audio/flac' },
});
const t3 = track(13, 1, 'T3', { duration: 300 });

function release(over: Partial<ReleaseDetail> & { id?: number } = {}): ReleaseDetail {
  return {
    id: 7,
    edition: '1986 Original CD',
    album: { id: 3, title: 'Slice Album', artists: [{ id: 1, name: 'Slice Artist' }] },
    media: [
      { disc: 1, tracks: [t1, t2] },
      { disc: 2, tracks: [t3] },
    ],
    artwork: [
      { id: 91, kind: 'artwork', role: 'front', mimeType: 'image/jpeg', url: '/api/v1/assets/91' },
    ],
    assets: [],
    loudness: { albumLufs: -9.5, albumTruePeakDb: -0.5 },
    ...over,
  };
}

describe('itemForTrack — the QueueItem construction boundary', () => {
  it('maps identity, source and policy metadata from the track/release', () => {
    const r = release();
    const item = itemForTrack(r, t3, 'Slice Album', 'Slice Artist');

    expect(item.id).toBe('t13');
    expect(item.trackId).toBe(13);
    expect(item.source).toEqual({
      kind: 'http-range',
      url: '/api/v1/tracks/13/audio',
      byteSize: 1013,
    });
    expect(item.durationHintSeconds).toBe(300);
    expect(item.title).toBe('T3');
    expect(item.artist).toBe('Slice Artist');
    expect(item.albumTitle).toBe('Slice Album');
    expect(item.edition).toBe('1986 Original CD');
    expect(item.artworkUrl).toBe('/api/v1/assets/91');
    expect(item.codec).toBe('musepack-sv8');
    expect(item.mimeType).toBe('audio/musepack');
    // web-specific passthroughs
    expect(item.track).toBe(t3);
    expect(item.releaseId).toBe(7);
    expect(item.albumId).toBe(3);
  });

  it('carries track loudness and album loudness verbatim', () => {
    const r = release();
    const item = itemForTrack(r, t1, 'x', 'y');
    expect(item.loudness).toEqual({ lufs: -12.25, truePeakDb: -3 });
    expect(item.albumLoudness).toEqual({ albumLufs: -9.5, albumTruePeakDb: -0.5 });
  });

  it('ignores representations: the default audio always feeds PlaybackItem', () => {
    // Phase 3 guard: a track carrying alternate representations must still
    // produce the byte-identical QueueItem — selection stays default-only
    // until a selection UI exists, and nothing representation-shaped leaks
    // into the player boundary.
    const withReps = track(14, 2, 'T4', {
      representations: [
        {
          id: 77,
          size: 999,
          url: '/api/v1/tracks/14/representations/77/audio',
          codec: { codec: 'flac', mimeType: 'audio/flac' },
          label: 'FLAC 24/96',
        },
      ],
    });
    const r = release();
    const plain = itemForTrack(r, track(14, 2, 'T4'), 'A', 'Artist');
    const item = itemForTrack(r, withReps, 'A', 'Artist');

    expect(item.source).toEqual(plain.source);
    expect(item.codec).toBe(plain.codec);
    expect(item.mimeType).toBe(plain.mimeType);
    expect(item.track.representations).toHaveLength(1); // carried verbatim
  });

  it('is pure: deterministic output, inputs never mutated', () => {    const r = release();
    const before = JSON.stringify({ r, t1, t2, t3 });
    const a = itemForTrack(r, t2, 'A', 'B');
    const b = itemForTrack(r, t2, 'A', 'B');
    expect(a).toEqual(b);
    expect(JSON.stringify({ r, t1, t2, t3 })).toBe(before);
  });
});

describe('album construction funnels through itemForTrack', () => {
  it('itemsForRelease builds disc-major, track-number order using one builder', () => {
    // Input order deliberately unsorted; the builder must normalize.
    const shuffled = release({
      media: [
        { disc: 2, tracks: [t3] },
        { disc: 1, tracks: [t2, t1] },
      ],
    });

    const items = itemsForRelease(shuffled, 'Slice Album', 'Slice Artist');
    expect(items.map((i) => i.title)).toEqual(['T1', 'T2', 'T3']);
    expect(items.map((i) => i.track)).toEqual(tracksOfRelease(shuffled));
    // every item came from the shared builder, in order
    expect(items).toEqual(
      tracksOfRelease(shuffled).map((t) => itemForTrack(shuffled, t, 'Slice Album', 'Slice Artist')),
    );
  });

  it('addAlbum appends builder output in order and leaves playback untouched', () => {
    const store = createQueueStore();
    const playing = store.playAlbum(release(), 'Slice Album', 'Slice Artist');
    const before = store.get();

    const extra = release({ id: 8, edition: 'Bonus Disc' });
    extra.album = { id: 4, title: 'Other Album', artists: [] };
    store.addAlbum(extra, 'Other Album', 'Someone');

    const after = store.get();
    expect(after.items).toHaveLength(before.items.length + 3);
    expect(after.items.slice(before.items.length).map((i) => i.title))
      .toEqual(['T1', 'T2', 'T3']);
    expect(after.items.slice(before.items.length).map((i) => i.releaseId))
      .toEqual([8, 8, 8]);
    // cursor and current item did not move
    expect(after.index).toBe(before.index);
    expect(store.get().items[after.index]?.id).toBe(playing.id);
  });

  it('addItem appends exactly one item after the current playback position state', () => {
    const store = createQueueStore();
    store.playAlbum(release(), 'Slice Album', 'Slice Artist');
    const before = store.get();

    const single = track(21, 1, 'S1', { duration: 50 });
    const other = release({
      id: 8,
      media: [{ disc: 1, tracks: [single] }],
    });
    store.addItem(itemForTrack(other, single, 'Other', 'Someone'));

    const after = store.get();
    expect(after.items).toHaveLength(before.items.length + 1);
    expect(after.items.at(-1)?.track.id).toBe(single.id);
    expect(after.index).toBe(before.index);
    expect(after.items[after.index]?.track.title).toBe('T1'); // still playing T1
  });

  it('enqueueing into an idle store keeps index at -1 until something plays', () => {
    const store = createQueueStore();
    const r = release({ id: 8 });
    store.addAlbum(r, 'Other Album', 'Someone');
    expect(store.get().items).toHaveLength(3);
    expect(store.get().index).toBe(-1);
  });

  it('addAlbum with an empty release is a safe no-op', () => {
    const store = createQueueStore();
    store.playAlbum(release(), 'Slice Album', 'Slice Artist');
    const n = store.get().items.length;
    const empty = release({ id: 9, media: [] });
    expect(() => store.addAlbum(empty, 'Empty', 'Nobody')).not.toThrow();
    expect(store.get().items).toHaveLength(n);
  });
});

describe('representation selection at the itemForTrack boundary (Phase 4)', () => {
  const flacRep = {
    id: 77,
    size: 999,
    url: '/api/v1/tracks/14/representations/77/audio',
    codec: { codec: 'flac', mimeType: 'audio/flac' },
    label: 'FLAC 24/96',
  };
  const wavRep = {
    id: 78,
    size: 4000,
    url: '/api/v1/tracks/14/representations/78/audio',
    codec: { codec: 'wav', mimeType: 'audio/wav' },
  };
  const withReps = track(14, 2, 'T4', { representations: [flacRep, wavRep] });
  const browser = {
    preference: { mode: 'lossless' } as const,
    canPlay: () => true,
  };

  it('omitted context is byte-identical to the plain default item', () => {
    const r = release();
    // The carried-verbatim `track` differs by design (one has representations);
    // every PLAYER-facing field must be identical.
    const strip = (q: QueueItem): Omit<QueueItem, 'track'> => {
      const { track: _t, ...rest } = q;
      return rest;
    };
    expect(strip(itemForTrack(r, withReps, 'A', 'Artist'))).toEqual(
      strip(itemForTrack(r, track(14, 2, 'T4'), 'A', 'Artist')),
    );
  });

  it('lossless preference selects the first lossless rep in manifest order', () => {
    const item = itemForTrack(release(), withReps, 'A', 'Artist', browser);
    expect(item.id).toBe('t14r77'); // representation-aware identity
    expect(item.representationId).toBe(77);
    expect(item.source).toEqual({ kind: 'http-range', url: flacRep.url, byteSize: flacRep.size });
    expect(item.codec).toBe('flac');
    expect(item.mimeType).toBe('audio/flac');
    // track-level metadata is representation-independent
    expect(item.trackId).toBe(14);
    expect(item.title).toBe('T4');
    expect(item.track).toBe(withReps);
  });

  it('unplayable primary is rescued by a playable alternate', () => {
    const item = itemForTrack(release(), withReps, 'A', 'Artist', {
      preference: { mode: 'default' },
      canPlay: (c) => c.codec !== 'musepack-sv8',
    });
    expect(item.representationId).toBe(77);
    expect(item.codec).toBe('flac');
  });

  it('explicit representation preference resolves by id', () => {
    const item = itemForTrack(release(), withReps, 'A', 'Artist', {
      preference: { mode: 'representation', id: 78 },
      canPlay: () => true,
    });
    expect(item.id).toBe('t14r78');
    expect(item.source.url).toBe(wavRep.url);
  });

  it('the same track under different selections yields distinct identities', () => {
    const r = release();
    const def = itemForTrack(r, withReps, 'A', 'B');
    const flac = itemForTrack(r, withReps, 'A', 'B', browser);
    const wav = itemForTrack(r, withReps, 'A', 'B', {
      preference: { mode: 'representation', id: 78 },
      canPlay: () => true,
    });
    expect(new Set([def.id, flac.id, wav.id]).size).toBe(3);
    expect(def.id).toBe('t14');
  });

  it('itemsForRelease funnels every track through the same policy', () => {
    const t5 = track(15, 3, 'T5');
    const rel = release({ media: [{ disc: 1, tracks: [withReps, t5] }] });
    const items = itemsForRelease(rel, 'A', 'B', browser);
    expect(items.map((i) => i.id)).toEqual(['t14r77', 't15']); // rep-aware + default
  });

  it('store playAlbum/addAlbum honor an injected selection context', () => {
    let calls = 0;
    const store = createQueueStore({
      selection: () => {
        calls++;
        return browser;
      },
    });
    const playing = store.playAlbum(release(), 'Slice Album', 'Slice Artist');
    expect(playing.id).toBe('t11'); // t1 has no representations
    store.addAlbum(release({ id: 8, media: [{ disc: 1, tracks: [withReps] }] }), 'Other', 'X');
    const last = store.get().items.at(-1);
    expect(last?.id).toBe('t14r77');
    expect(calls).toBeGreaterThanOrEqual(2);
  });

  it('selection stays deterministic and never mutates its inputs', () => {
    const r = release();
    const before = JSON.stringify({ r, withReps });
    const a = itemForTrack(r, withReps, 'A', 'B', browser);
    const b = itemForTrack(r, withReps, 'A', 'B', browser);
    expect(a).toEqual(b);
    expect(JSON.stringify({ r, withReps })).toBe(before);
  });
});

describe('codecLabel', () => {
  it.each([
    ['musepack-sv8', 'MPC'],
    ['musepack-sv7', 'MPC'],
    ['musepack', 'MPC'],
    ['flac', 'FLAC'],
    ['wav', 'WAV'],
    ['aiff', 'AIFF'],
    ['aif', 'AIFF'],
    ['mp3', 'MP3'],
    ['aac', 'AAC'],
    ['m4a', 'AAC'],
    ['ogg', 'OGG'],
    ['vorbis', 'OGG'],
    ['opus', 'Opus'],
  ])('%s -> %s', (input, expected) => {
    expect(codecLabel(input)).toBe(expected);
  });

  it('is case-insensitive', () => {
    expect(codecLabel('FLAC')).toBe('FLAC');
    expect(codecLabel('MusePack-SV8')).toBe('MPC');
  });

  it('returns empty for missing codec and uppercases unknown ones', () => {
    expect(codecLabel(undefined)).toBe('');
    expect(codecLabel('')).toBe('');
    expect(codecLabel('monkeysaudio')).toBe('MONKEYSAUDIO');
  });
});
