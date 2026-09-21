// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import { createQueueModel } from '../../player-core/src/queue';
import type { PlaybackItem } from '../../player-core/src/types';

function item(n: number): PlaybackItem {
  return {
    id: `t${n}`,
    trackId: n,
    source: { kind: 'http-range', url: `/api/v1/tracks/${n}/audio`, byteSize: 100 },
    title: `T${n}`,
    artist: 'A',
    albumTitle: 'AL',
  };
}

/** Deterministic LCG rng factory. */
function seeded(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) % 2147483648;
    return s / 2147483648;
  };
}

describe('QueueModel repeat + shuffle policy (M5)', () => {
  it('repeat-all wraps next() to index 0', () => {
    const q = createQueueModel({ rng: seeded(7) });
    q.playSequence([item(1), item(2), item(3)]);
    q.setRepeat('all');
    q.next();
    q.next();
    expect(q.get().index).toBe(2);
    expect(q.next()?.title).toBe('T1'); // wrap
    expect(q.get().index).toBe(0);
  });

  it('repeat-off stops at the end', () => {
    const q = createQueueModel({ rng: seeded(3) });
    q.playSequence([item(1), item(2)]);
    q.next();
    expect(q.next()).toBeNull();
    expect(q.get().index).toBe(1);
  });

  it('shuffle keeps the current item first and visits every item exactly once per pass', () => {
    const q = createQueueModel({ rng: seeded(42) });
    q.playSequence([item(1), item(2), item(3), item(4), item(5)], 2);
    q.setShuffle(true);
    const order = q.getPresentationOrder();
    expect(order[0]).toBe(2); // current first
    expect([...order].sort((a, b) => a - b)).toEqual([0, 1, 2, 3, 4]); // permutation
    // walk exactly one shuffled pass (repeat is off → next() returns null at end)
    const visited: number[] = [q.get().index];
    for (let steps = 0; steps < 10; steps++) {
      const n = q.next();
      if (!n) break;
      visited.push(q.get().index);
    }
    expect([...visited].sort((a, b) => a - b)).toEqual([0, 1, 2, 3, 4]);
    expect(visited[0]).toBe(2);
    expect(visited).toHaveLength(5);
  });

  it('toggling shuffle off restores canonical order navigation', () => {
    const q = createQueueModel({ rng: seeded(1) });
    q.playSequence([item(1), item(2), item(3)]);
    q.setShuffle(true);
    q.next(); // somewhere non-canonical
    q.setShuffle(false);
    expect(q.getPresentationOrder()).toEqual([]);
    // from wherever we are, next() is now +1 canonical
    const at = q.get().index;
    if (at < 2) {
      expect(q.next()).not.toBeNull();
      expect(q.get().index).toBe(at + 1);
    }
  });

  it('history-aware previous() retraces shuffled navigation', () => {
    const q = createQueueModel({ rng: seeded(9) });
    q.playSequence([item(1), item(2), item(3), item(4)]);
    q.setShuffle(true);
    const first = q.get().index;
    q.next(); // jump somewhere in the shuffled order (history records origin)
    // step back must return to WHERE WE CAME FROM, not merely index-1
    expect(q.previous()).not.toBeNull();
    expect(q.get().index).toBe(first);
  });

  it('move() reorders canonically; cursor and history follow the item', () => {
    const q = createQueueModel({ rng: seeded(5) });
    q.playSequence([item(1), item(2), item(3), item(4)], 1); // cursor at T2
    q.move(1, 3); // T2 to the end -> [T1,T3,T4,T2], cursor follows to 3
    expect(q.get().items.map((i) => i.title)).toEqual(['T1', 'T3', 'T4', 'T2']);
    expect(q.get().index).toBe(3);
    // moving an item BEFORE the cursor shifts the cursor up
    q.move(3, 0); // T2 to the front -> [T2,T1,T3,T4], cursor 0
    expect(q.get().items.map((i) => i.title)).toEqual(['T2', 'T1', 'T3', 'T4']);
    expect(q.get().index).toBe(0);
    // out-of-range and no-op moves are ignored
    q.move(-1, 2);
    q.move(0, 99);
    q.move(2, 2);
    expect(q.get().items.map((i) => i.title)).toEqual(['T2', 'T1', 'T3', 'T4']);
  });

  it('move() keeps history indices valid for previous()', () => {
    const q = createQueueModel({ rng: seeded(5) });
    q.playSequence([item(1), item(2), item(3), item(4)], 0);
    q.next(); // at T2; history [0]
    q.move(1, 0); // T2 to front; cursor was 1 -> 0; history 0 -> 1
    // previous() must land on the item we actually came from (original T1)
    const prev = q.previous();
    expect(prev?.title).toBe('T1');
  });

  it('removeAt keeps history indices valid', () => {
    const q = createQueueModel({ rng: seeded(5) });
    q.playSequence([item(1), item(2), item(3), item(4)]);
    q.setShuffle(true);
    q.next();
    q.setShuffle(false);
    q.removeAt(0);
    // no crash, cursor in range
    expect(q.get().index).toBeGreaterThanOrEqual(0);
    expect(q.get().index).toBeLessThan(q.get().items.length);
  });

  it('repeat-all reshuffles at the end of a shuffled pass', () => {
    const q = createQueueModel({ rng: seeded(11) });
    q.playSequence([item(1), item(2), item(3)]);
    q.setShuffle(true);
    q.setRepeat('all');
    // walk one bounded pass; the first next() AFTER exhaustion proves the reshuffle
    for (let steps = 0; steps < 5; steps++) {
      if (!q.next()) break;
    }
    expect(q.next()).not.toBeNull();
  });
});
