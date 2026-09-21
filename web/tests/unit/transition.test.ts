// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Golden-table tests for the Sweet-Fade transition policy. The policy is a
// pure function: every rule maps to a deterministic plan for a given
// boundary contour.

import { describe, expect, it } from 'vitest';
import {
  isCleanLoudEnding,
  isFastAttack,
  planTransition,
  trailingSilenceSeconds,
  type BoundaryProfile,
  type TransitionQuery,
} from '../../player-core/src/transition';
import type { PlaybackItem } from '../../player-core/src/types';

const ITEM: PlaybackItem = {
  id: 't',
  trackId: 1,
  source: { kind: 'http-range', url: '/x', byteSize: 1 },
  title: 'T',
  artist: 'A',
  albumTitle: 'AL',
};

function query(over: Partial<TransitionQuery> = {}): TransitionQuery {
  return {
    outgoing: ITEM,
    incoming: { ...ITEM, trackId: 2 },
    maxFadeSeconds: 8,
    repeatOne: false,
    ...over,
  };
}

/** Builds a normalized tail/head window from per-100ms values. */
function profile(
  tail: number[],
  head: number[] = [],
  lengthSeconds = 200,
): BoundaryProfile {
  return { lengthSeconds, tail, head };
}

const BPS = 0.1; // seconds per bucket in these fixtures

describe('Sweet-Fade transition policy', () => {
  it('never fades when the feature is off or repeat-one', () => {
    const out = profile([0.8, 0.8, 0.8]);
    expect(planTransition(query({ maxFadeSeconds: 0 }), out, out)).toEqual({ type: 'gapless' });
    expect(planTransition(query({ repeatOne: true }), out, out)).toEqual({ type: 'gapless' });
  });

  it('falls back to the legacy fixed fade when profiles are missing', () => {
    expect(planTransition(query(), null, null)).toEqual({
      type: 'sweet-fade',
      overlapSeconds: 8,
    });
    const partial = profile([]);
    expect(planTransition(query({ maxFadeSeconds: 4 }), partial, null)).toEqual({
      type: 'sweet-fade',
      overlapSeconds: 4,
    });
  });

  it('keeps gapless playback when the outgoing track ends in silence', () => {
    // 2 s of decaying tail into digital silence.
    const tail = [...Array(20).fill(0.7), ...Array(20).fill(0), ...Array(30).fill(0)];
    expect(trailingSilenceSeconds(tail, BPS)).toBeGreaterThanOrEqual(3);
    const plan = planTransition(query({ sameRelease: true }), profile(tail), profile([]));
    expect(plan).toEqual({ type: 'gapless' });
  });

  it('keeps gapless playback for same-release clean loud joins', () => {
    // Steady full-energy right up to the end: an intended continuous join.
    const tail = Array(100).fill(0.8);
    const plan = planTransition(
      query({ sameRelease: true }),
      profile(tail),
      profile(Array(50).fill(0.7)),
    );
    expect(plan).toEqual({ type: 'gapless' });
  });

  it('chooses hard-cut when a loud ending meets a fast attack cross-context', () => {
    const tail = Array(100).fill(0.9);
    const plan = planTransition(
      query({ sameRelease: false }),
      profile(tail),
      { lengthSeconds: 200, head: [0.85, 0.9, 0.9] },
    );
    expect(plan).toEqual({ type: 'hard-cut' });
  });

  it('hugs the outro decay when choosing the overlap', () => {
    // 10 s window: loud for 5 s then a linear decay over the last 5 s
    // (last bucket near-silent → not a "clean loud ending").
    const tail = [
      ...Array(50).fill(0.9),
      ...Array.from({ length: 50 }, (_, i) => 0.5 * (1 - i / 50)),
    ];
    const head = [0.1, 0.2, 0.6]; // moderate attack: fade is warranted
    const plan = planTransition(query({ maxFadeSeconds: 12 }), profile(tail), {
      lengthSeconds: 200,
      head,
    });
    expect(plan.type).toBe('sweet-fade');
    if (plan.type !== 'sweet-fade') return;
    // Decay spans ~2 s below the loud floor + grace; well under both the
    // 6 s base and the 12 s cap.
    expect(plan.overlapSeconds).toBeGreaterThan(1);
    expect(plan.overlapSeconds).toBeLessThanOrEqual(6);
  });

  it('caps the planned overlap at the user setting', () => {
    const tail = [...Array(80).fill(0.9), ...Array(20).fill(0.05)];
    const plan = planTransition(query({ maxFadeSeconds: 3 }), profile(tail), profile([0.1]));
    expect(plan.type).toBe('sweet-fade');
    if (plan.type !== 'sweet-fade') return;
    expect(plan.overlapSeconds).toBeLessThanOrEqual(3);
  });

  it('is deterministic for identical queries', () => {
    const out = profile(Array(60).fill(0.8));
    const inc: BoundaryProfile = { lengthSeconds: 200, head: [0.2, 0.5] };
    const a = planTransition(query(), out, inc);
    const b = planTransition(query(), out, inc);
    expect(a).toEqual(b);
  });

  it('classifies endings and attacks via helpers', () => {
    expect(isCleanLoudEnding([...Array(40).fill(0.9)], BPS)).toBe(true);
    expect(isCleanLoudEnding([...Array(38).fill(0.9), 0.1, 0.02], BPS)).toBe(false);
    expect(isFastAttack([0.01, 0.01, 0.9], BPS)).toBe(true);
    expect(isFastAttack([0.01, 0.01, 0.01], BPS)).toBe(false);
  });
});
