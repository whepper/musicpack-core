// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, it, expect } from 'vitest';
import { createQueueModel } from '../../player-core/src/queue';
import { nextIndexUnderRepeat, shuffleOrder } from '../../player-core/src/order';
import type { PlaybackItem } from '../../player-core/src/types';
function rng(): () => number {
  let s = 42;
  return () => {
    s = (s * 1103515245 + 12345) % 2147483648;
    return s / 2147483648;
  };
}


function item(idNum: number, title?: string): PlaybackItem {
  return {
    id: `t${idNum}`,
    trackId: idNum,
    source: { kind: 'http-range', url: `/api/v1/tracks/${idNum}/audio`, byteSize: 100 },
    title: title ?? `T${idNum}`,
    artist: 'A',
    albumTitle: 'AL',
  };
}

describe('QueueModel (core)', () => {
  it('playSequence picks startIndex and keeps the full sequence', () => {
    const q = createQueueModel({ rng: rng() });
    const first = q.playSequence([item(1), item(2), item(3)], 1);
    expect(first.title).toBe('T2');
    expect(q.get().items).toHaveLength(3);
    expect(q.get().index).toBe(1);
  });

  it('next/previous advance and stop at the boundaries', () => {
    const q = createQueueModel({ rng: rng() });
    q.playSequence([item(1), item(2)]);
    expect(q.next()?.title).toBe('T2');
    expect(q.next()).toBeNull();
    expect(q.previous()?.title).toBe('T1');
    expect(q.previous()).toBeNull();
  });

  it('playNext inserts after the cursor; enqueue appends; removeAt fixes the cursor', () => {
    const q = createQueueModel({ rng: rng() });
    q.playSequence([item(1), item(3)]);
    q.playNext(item(2));
    expect(q.get().items.map((i) => i.title)).toEqual(['T1', 'T2', 'T3']);
    expect(q.get().index).toBe(0);
    q.removeAt(0); // before cursor
    expect(q.get().index).toBe(0);
    q.removeAt(0); // the cursor itself
    expect(q.get().items.map((i) => i.title)).toEqual(['T3']);
    expect(q.get().index).toBe(0);
    q.clear();
    expect(q.get().items).toHaveLength(0);
    expect(q.get().index).toBe(-1);
  });
});

describe('order.ts policy scaffolding', () => {
  it('shuffleOrder is a permutation (deterministic with a seeded rng)', () => {
    const seq = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => item(n));
    let seed = 42;
    const rng = (): number => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed / 2147483648;
    };
    const shuffled = shuffleOrder(seq, rng);
    // same elements exactly once
    expect([...shuffled].map((i) => i.trackId).sort((a, b) => a - b)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    // deterministic: same seed → same order
    let seed2 = 42;
    const rng2 = (): number => {
      seed2 = (seed2 * 1103515245 + 12345) % 2147483648;
      return seed2 / 2147483648;
    };
    expect(shuffleOrder(seq, rng2).map((i) => i.trackId)).toEqual(shuffled.map((i) => i.trackId));
    // input untouched
    expect(seq.map((i) => i.trackId)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it('nextIndexUnderRepeat implements off/one/all boundary behavior', () => {
    expect(nextIndexUnderRepeat(0, 3, 'off')).toBe(1);
    expect(nextIndexUnderRepeat(2, 3, 'off')).toBeNull(); // end, no repeat
    expect(nextIndexUnderRepeat(2, 3, 'one')).toBeNull(); // repeat-one is a Player-level reload
    expect(nextIndexUnderRepeat(2, 3, 'all')).toBe(0); // wrap
    expect(nextIndexUnderRepeat(1, 3, 'all')).toBe(2); // mid-queue unaffected
    expect(nextIndexUnderRepeat(0, 0, 'all')).toBeNull(); // empty
  });
});
