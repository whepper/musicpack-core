// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Queue ordering policy (player-core M3 scaffolding; consumed by M5).
//
// Pure functions over the queue's canonical items. Shuffle never destroys
// the canonical order: it produces a presentation ORDER (a permutation) that
// QueueModel applies on top. Determinism: every function takes an injectable
// RNG so tests (and future hosts) can seed randomness — purity law 2.

import type { PlaybackItem } from './types';

export type RepeatMode = 'off' | 'one' | 'all';

export type Rng = () => number;

/** Fisher–Yates over `upcoming` using `rng`. Returns a new array; inputs
 *  are untouched. */
export function shuffleOrder<T>(upcoming: readonly T[], rng: Rng): T[] {
  const out = [...upcoming];
  for (let i = out.length - 1 > 0 ? out.length - 1 : 0; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    const a = out[i] as T;
    const b = out[j] as T;
    out[i] = b;
    out[j] = a;
  }
  return out;
}

/** The index of the next item under the given repeat mode.
 *  - off/one: null at the end ('one' repeats via reload in Player, not here)
 *  - all: wraps to 0 */
export function nextIndexUnderRepeat(
  cursor: number,
  length: number,
  repeat: RepeatMode,
): number | null {
  if (length === 0) return null;
  if (cursor + 1 < length) return cursor + 1;
  return repeat === 'all' ? 0 : null;
}

/** Identity helper: stable per-queue-entry id when a host has none. */
export function itemKey(item: PlaybackItem): string {
  return `${item.id}@${item.trackId}`;
}
