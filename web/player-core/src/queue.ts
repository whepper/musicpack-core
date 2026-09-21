// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// QueueModel (player-core M3, extended M5).
//
// Framework-free queue with canonical items + cursor. Repeat modes and
// shuffle are POLICY layered over the canonical list (never destroying it):
// - repeat 'one' is resolved by the Player at EOS (reload current), so
//   advance() treats it like 'off';
// - repeat 'all' wraps advance() to index 0;
// - shuffle produces a presentation order via order.ts (seeded RNG);
//   stepBack() consults a history stack so Previous works under shuffle;
//   toggling shuffle off restores canonical order.

import { writable, type Writable } from './store';
import type { PlaybackItem } from './types';
import { nextIndexUnderRepeat, shuffleOrder, type RepeatMode, type Rng } from './order';

export interface QueueState<TItem> {
  items: TItem[];
  /** Index of the current item (-1 when the queue is empty). */
  index: number;
}

function empty<TItem>(): QueueState<TItem> {
  return { items: [], index: -1 };
}

/** Framework-free queue model. Generic over the item shape: hosts may carry
 *  richer entries (the web QueueItem extends PlaybackItem). All semantics
 *  live here; hosts only add item CONSTRUCTION. */
export function createQueueModel<TItem extends PlaybackItem>(opts: { rng: Rng }) {
  const store: Writable<QueueState<TItem>> = writable<QueueState<TItem>>(empty<TItem>());
  /** Presentation order when shuffled: indices into canonical items.
   *  Empty = identity (canonical order). */
  let order: number[] = [];
  /** Visited cursors for history-aware step-back under shuffle/repeat-all. */
  let history: number[] = [];
  const rng: Rng = opts.rng;
  let repeat: RepeatMode = 'off';
  let shuffling = false;

  function effectiveNext(): TItem | null {
    const s = store.get();
    if (s.items.length === 0) return null;
    if (!shuffling) {
      const ni = nextIndexUnderRepeat(s.index, s.items.length, repeat);
      return ni === null ? null : s.items[ni] ?? null;
    }
    // Shuffled: follow the presentation order from the current position.
    const posInOrder = order.indexOf(s.index);
    if (posInOrder >= 0 && posInOrder + 1 < order.length) {
      const ni = order[posInOrder + 1]!;
      return s.items[ni] ?? null;
    }
    // End of the shuffled pass: wrap only under repeat-all (reshuffle).
    if (repeat === 'all') {
      reshuffle();
      const first = order[0];
      return first === undefined ? null : s.items[first] ?? null;
    }
    return null;
  }

  function effectivePrev(): TItem | null {
    const s = store.get();
    if (s.items.length === 0) return null;
    // History-aware back navigation (works under shuffle and after wraps).
    while (history.length > 0) {
      const prev = history.pop()!;
      if (prev >= 0 && prev < s.items.length && prev !== s.index) {
        return s.items[prev] ?? null;
      }
    }
    if (!shuffling) {
      const pi = s.index - 1;
      return pi >= 0 ? s.items[pi] ?? null : null;
    }
    const posInOrder = order.indexOf(s.index);
    if (posInOrder > 0) {
      const pi = order[posInOrder - 1]!;
      return s.items[pi] ?? null;
    }
    return null;
  }

  function rebuildOrder(): void {
    const s = store.get();
    if (!shuffling || s.items.length === 0) {
      order = [];
      return;
    }
    // Current stays first; the rest Fisher–Yates.
    const upcoming: number[] = [];
    for (let i = 0; i < s.items.length; i++) if (i !== s.index) upcoming.push(i);
    const rest = shuffleOrder(upcoming, rng);
    order = s.index >= 0 ? [s.index, ...rest] : [...rest];
  }

  function reshuffle(): void {
    rebuildOrder();
  }

  function pushHistory(from: number): void {
    if (from >= 0) history.push(from);
    // Bound the stack; deep sessions don't need unbounded memory.
    if (history.length > 500) history.splice(0, history.length - 500);
  }

  return {
    ...store,
    state: store.get,

    get repeat(): RepeatMode {
      return repeat;
    },
    setRepeat(mode: RepeatMode): void {
      repeat = mode;
    },

    get shuffle(): boolean {
      return shuffling;
    },
    setShuffle(on: boolean): void {
      if (shuffling === on) return;
      shuffling = on;
      history = [];
      rebuildOrder();
    },
    /** Test/inspection seam: the active presentation order (indices). */
    getPresentationOrder(): number[] {
      return shuffling ? [...order] : [];
    },
    /** Test-only seam: install a presentation order. Lets deterministic
     *  tests pin a shuffle successor without reaching into closures.
     *  Must only be called while shuffle is ON and with a permutation of
     *  canonical indices starting at the cursor. */
    setPresentationOrderForTest(next: number[]): void {
      if (!shuffling) return;
      order = [...next];
    },

    /** Play the given sequence starting at `startIndex` (default first). */
    playSequence(items: TItem[], startIndex = 0): TItem {
      if (items.length === 0) throw new Error('This release has no playable tracks.');
      const index = Math.max(0, Math.min(startIndex, items.length - 1));
      store.set({ items, index });
      history = [];
      rebuildOrder();
      const item = items[index] ?? items[0];
      if (!item) throw new Error('This release has no playable tracks.');
      return item;
    },
    /** Replace the queue with a single item. */
    playNow(item: TItem): TItem {
      store.set({ items: [item], index: 0 });
      history = [];
      rebuildOrder();
      return item;
    },
    /** Insert right after the current item. */
    playNext(item: TItem): void {
      store.update((s) => {
        if (s.items.length === 0) return { items: [item], index: 0 };
        const at = s.index + 1;
        const items = [...s.items];
        items.splice(at, 0, item);
        return { items, index: s.index };
      });
      rebuildOrder();
    },
    /** Append to the end of the queue. */
    enqueue(item: TItem): void {
      store.update((s) => ({ items: [...s.items, item], index: s.index }));
      rebuildOrder();
    },
    enqueueMany(items: TItem[]): void {
      store.update((s) => ({ items: [...s.items, ...items], index: s.index }));
      rebuildOrder();
    },
    current(): TItem | null {
      const s = store.get();
      return s.index >= 0 && s.index < s.items.length ? s.items[s.index] ?? null : null;
    },
    at(i: number): TItem | null {
      return store.get().items[i] ?? null;
    },
    /** Advance under the active policy (repeat/shuffle). Records history. */
    next(): TItem | null {
      let item: TItem | null = null;
      store.update((s) => {
        const target = this.peekNextIndex(s);
        if (target === null) return s;
        const next = { ...s, index: target };
        item = next.items[next.index] ?? null;
        return next;
      });
      return item;
    },
    /** History-aware back navigation. */
    previous(): TItem | null {
      let item: TItem | null = null;
      store.update((s) => {
        const target = this.peekPrevIndex(s);
        if (target === null) return s;
        const next = { ...s, index: target };
        item = next.items[next.index] ?? null;
        return next;
      });
      return item;
    },
    /** Move the cursor to an item (no queue rebuild; user-initiated jump). */
    moveTo(i: number): void {
      store.update((s) => {
        if (i < 0 || i >= s.items.length) return s;
        return { ...s, index: i };
      });
    },
    /** Reorders the canonical list (queue-panel drag/arrows). The cursor
     *  follows the current ITEM across the move; history indices are
     *  remapped so Previous still retraces real navigation. */
    move(from: number, to: number): void {
      store.update((s) => {
        const n = s.items.length;
        if (from < 0 || from >= n || to < 0 || to >= n || from === to) return s;
        const items = [...s.items];
        const [moved] = items.splice(from, 1);
        items.splice(to, 0, moved!);
        const remap = (i: number): number => {
          if (i === from) return to;
          if (from < to && i > from && i <= to) return i - 1;
          if (from > to && i >= to && i < from) return i + 1;
          return i;
        };
        history = history.map(remap);
        return { items, index: remap(s.index) };
      });
      rebuildOrder();
    },
    removeAt(i: number): void {
      store.update((s) => {
        if (i < 0 || i >= s.items.length) return s;
        const removedCursorShift = i < s.index ? 1 : 0;
        const wasCurrent = i === s.index;
        const items = s.items.filter((_, idx) => idx !== i);
        let index = s.index - removedCursorShift;
        if (wasCurrent) index = Math.min(index, items.length - 1);
        // keep history pointing at valid entries
        history = history.filter((h) => h !== i).map((h) => (h > i ? h - 1 : h));
        return { items, index };
      });
      rebuildOrder();
    },
    clear(): void {
      store.set(empty<TItem>());
      history = [];
      order = [];
    },

    // -- internal helpers used by next()/previous() above ------------------
    peekNextIndex(s: QueueState<TItem>): number | null {
      if (s.items.length === 0) return null;
      pushHistory(s.index);
      if (!shuffling) return nextIndexUnderRepeat(s.index, s.items.length, repeat);
      const posInOrder = order.indexOf(s.index);
      if (posInOrder >= 0 && posInOrder + 1 < order.length) return order[posInOrder + 1]!;
      if (repeat === 'all') {
        reshuffle();
        return order[0] ?? null;
      }
      return null;
    },
    peekPrevIndex(s: QueueState<TItem>): number | null {
      if (s.items.length === 0) return null;
      while (history.length > 0) {
        const prev = history.pop()!;
        if (prev >= 0 && prev < s.items.length && prev !== s.index) return prev;
      }
      if (!shuffling) {
        const pi = s.index - 1;
        return pi >= 0 ? pi : null;
      }
      const posInOrder = order.indexOf(s.index);
      return posInOrder > 0 ? order[posInOrder - 1]! : null;
    },
  };
}

export type QueueModel<TItem extends PlaybackItem = PlaybackItem> = ReturnType<
  typeof createQueueModel<TItem>
>;
