import { describe, it, expect } from 'vitest';
import { createQueueStore, tracksOfRelease } from '../../app/src/lib/state/queue';
import type { ReleaseDetail, Track } from '../../app/src/lib/api/types';

function track(id: number, number: number, title: string): Track {
  return {
    id,
    number,
    title,
    artists: [],
    codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
    audio: { id, size: 100, url: `/api/v1/tracks/${id}/audio` },
  };
}

function release(id: number, discs: Array<{ disc: number; tracks: Track[] }>): ReleaseDetail {
  return {
    id,
    album: { id: 1, title: 'A', artists: [] },
    media: discs.map((d) => ({ disc: d.disc, tracks: d.tracks })),
    artwork: [],
    assets: [],
  };
}

describe('queue', () => {
  it('playAlbum builds disc-major, track-number order', () => {
    const q = createQueueStore();
    const r = release(9, [
      { disc: 2, tracks: [track(3, 1, 'D2T1'), track(4, 2, 'D2T2')] },
      { disc: 1, tracks: [track(1, 2, 'D1T2'), track(2, 1, 'D1T1')] },
    ]);
    const first = q.playAlbum(r, 'A', 'Artist');
    expect(q.get().items.map((i) => i.track.title)).toEqual(['D1T1', 'D1T2', 'D2T1', 'D2T2']);
    expect(first.track.title).toBe('D1T1');
  });

  it('playAlbum startIndex picks a later track but keeps the full album', () => {
    const q = createQueueStore();
    const r = release(9, [{ disc: 1, tracks: [track(1, 1, 'T1'), track(2, 2, 'T2'), track(3, 3, 'T3')] }]);
    const first = q.playAlbum(r, 'A', 'Artist', 1);
    expect(first.track.title).toBe('T2');
    expect(q.get().items).toHaveLength(3);
    expect(q.get().index).toBe(1);
  });

  it('next/previous advance and stop at the boundaries', () => {
    const q = createQueueStore();
    q.playAlbum(release(9, [{ disc: 1, tracks: [track(1, 1, 'T1'), track(2, 2, 'T2')] }]), 'A', 'A');
    expect(q.next()?.track.title).toBe('T2');
    expect(q.next()).toBeNull(); // at the end
    expect(q.previous()?.track.title).toBe('T1');
    expect(q.previous()).toBeNull(); // at the start
  });

  it('playNext inserts after the current item', () => {
    const q = createQueueStore();
    q.playAlbum(release(9, [{ disc: 1, tracks: [track(1, 1, 'T1'), track(3, 3, 'T3')] }]), 'A', 'A');
    q.playNext({
      track: track(2, 2, 'T2'),
      releaseId: 9,
      albumId: 1,
      albumTitle: 'A',
      artist: 'A',
      id: 't2',
      trackId: 2,
      title: 'T2',
      source: { kind: 'http-range', url: '/api/v1/tracks/2/audio', byteSize: 100 },
      codec: 'musepack-sv8',
    });
    expect(q.get().items.map((i) => i.track.title)).toEqual(['T1', 'T2', 'T3']);
    expect(q.get().index).toBe(0);
  });

  it('removeAt adjusts the cursor', () => {
    const q = createQueueStore();
    q.playAlbum(release(9, [{ disc: 1, tracks: [track(1, 1, 'T1'), track(2, 2, 'T2'), track(3, 3, 'T3')] }]), 'A', 'A');
    q.removeAt(0); // remove before current
    expect(q.get().index).toBe(0);
    q.removeAt(0); // remove the current
    expect(q.get().index).toBe(0);
    expect(q.get().items.map((i) => i.track.title)).toEqual(['T3']);
  });

  it('clear empties the queue', () => {
    const q = createQueueStore();
    q.playAlbum(release(9, [{ disc: 1, tracks: [track(1, 1, 'T1')] }]), 'A', 'A');
    q.clear();
    expect(q.get().items).toHaveLength(0);
    expect(q.get().index).toBe(-1);
  });

  it('tracksOfRelease orders by disc then track number', () => {
    const r = release(9, [
      { disc: 2, tracks: [track(3, 1, 'B'), track(4, 2, 'B2')] },
      { disc: 1, tracks: [track(1, 2, 'A2'), track(2, 1, 'A')] },
    ]);
    expect(tracksOfRelease(r).map((t) => t.title)).toEqual(['A', 'A2', 'B', 'B2']);
  });
});
