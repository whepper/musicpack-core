// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Transition planning for the crossfade feature ("Sweet Fades", M8 repair).
//
// A PURE function from content profiles + policy context to a transition
// decision. No DOM, no fetching, no ambient state (player-core purity laws):
// hosts compute TrackTailHeadProfiles from whatever data they have (the web
// app derives them from the per-track waveform envelopes) and inject this
// module's output through PlayerPorts.planTransition.
//
// Design rules:
// - Missing data degrades gracefully to the legacy fixed-length fade, never
//   to a worse experience than before.
// - Recordings that separate themselves (trailing silence) or join cleanly
//   (consecutive album tracks with a clean loud ending) keep true gapless
//   playback even when crossfade is enabled.
// - Overlaps hug the outgoing track's decay instead of blindly applying a
//   fixed duration.

import type { PlaybackItem } from './types';

export type TransitionPlan =
  /** Sample-exact sequential handoff (the normal EOS path). */
  | { type: 'gapless' }
  /** Natural stop/start; overlapping would mush two full-energy signals. */
  | { type: 'hard-cut' }
  /** Equal-power overlap of exactly this many seconds. */
  | { type: 'sweet-fade'; overlapSeconds: number };

/** Linear RMS energy contour around one track boundary. Values are
 *  normalized to the segment's own peak (0..1), so the absolute envelope
 *  scale of the source analysis is irrelevant here. Undefined when the
 *  host has no data (yet). */
export interface BoundaryProfile {
  lengthSeconds: number;
  /** Last TAIL_SECONDS of the track, oldest first, one value per bucket. */
  tail?: number[];
  /** First HEAD_SECONDS of the following track, one value per bucket. */
  head?: number[];
}

export interface TransitionQuery {
  outgoing: PlaybackItem;
  incoming: PlaybackItem;
  /** User cap in seconds (0 disables the feature entirely). */
  maxFadeSeconds: number;
  /** Repeat-one reloads instead of fading (core also guards this). */
  repeatOne: boolean;
  /** Both items belong to one release (album flow keeps gapless intent). */
  sameRelease?: boolean;
}

// ---- tuning constants (exported for tests + future tuning) ----------------

/** Linear RMS below which a bucket counts as silence (~-34 dBFS relative). */
export const SILENCE_THRESHOLD = 0.02;
/** Trailing silence at least this long → the recording already separates. */
export const MIN_TRAILING_SILENCE_SECONDS = 1.2;
/** Fraction of a segment's median that still counts as "loud". */
export const LOUD_FRACTION = 0.25;
/** Shortest overlap worth scheduling when a fade is warranted. */
export const FADE_MIN_SECONDS = 1;
/** Longest overlap the planner will ever choose on its own. */
export const FADE_BASE_SECONDS = 6;
/** Slack added after the last loud bucket so the decay keeps its tail. */
export const OUTRO_GRACE_SECONDS = 0.5;
/** Extra lead before the overlap span in which arming is allowed (lets
 *  the engine prime its lane). Core-side constant, mirrored here for tests. */
export const PRIME_LEAD_SECONDS = 1;

function median(values: number[]): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1
    ? sorted[mid]!
    : (sorted[mid - 1]! + sorted[mid]!) / 2;
}

function energyFloor(contour: number[]): number {
  return Math.max(SILENCE_THRESHOLD, LOUD_FRACTION * median(contour));
}

/** Seconds of near-silence at the very end of the contour. */
export function trailingSilenceSeconds(tail: number[], secondsPerBucket: number): number {
  let n = 0;
  for (let i = tail.length - 1; i >= 0; i--) {
    if ((tail[i] ?? 0) >= SILENCE_THRESHOLD) break;
    n++;
  }
  return n * secondsPerBucket;
}

/** True when the track ends loud and stays loud: no sustained decay, no
 *  drop into silence — the recording "cuts" at full energy. */
export function isCleanLoudEnding(tail: number[], secondsPerBucket: number): boolean {
  if (tail.length === 0) return false;
  const floor = energyFloor(tail);
  if ((tail[tail.length - 1] ?? 0) < floor) return false;
  const check = Math.min(tail.length, Math.max(1, Math.round(2 / secondsPerBucket)));
  for (let i = tail.length - check; i < tail.length; i++) {
    if ((tail[i] ?? 0) < LOUD_FRACTION * median(tail)) return false;
  }
  return true;
}

/** True when the next track reaches loud energy within ~0.3 s of starting. */
export function isFastAttack(head: number[], secondsPerBucket: number): boolean {
  if (head.length === 0) return false;
  const floor = energyFloor(head);
  const check = Math.min(head.length, Math.max(1, Math.round(0.3 / secondsPerBucket)));
  for (let i = 0; i < check; i++) {
    if ((head[i] ?? 0) >= floor) return true;
  }
  return false;
}

/** Seconds since the last loud bucket in the tail (the audible outro start). */
export function decaySeconds(tail: number[], secondsPerBucket: number): number {
  const floor = energyFloor(tail);
  for (let i = tail.length - 1; i >= 0; i--) {
    if ((tail[i] ?? 0) >= floor) return (tail.length - 1 - i) * secondsPerBucket;
  }
  return tail.length * secondsPerBucket;
}

/**
 * The Sweet-Fade policy. Deterministic and side-effect free; see the module
 * comment for the intent of each rule.
 */
export function planTransition(
  query: TransitionQuery,
  outgoingProfile: BoundaryProfile | null,
  incomingProfile: BoundaryProfile | null,
  secondsPerBucket = 0.1,
): TransitionPlan {
  const maxFade = Math.max(0, query.maxFadeSeconds);
  // Feature off / repeat-one: never fade. The core repeats these guards,
  // but the planner staying honest on its own keeps it testable.
  if (maxFade === 0 || query.repeatOne) return { type: 'gapless' };

  // No content data (yet): legacy behavior — fade for the full user cap.
  const outTail = outgoingProfile?.tail;
  const inHead = incomingProfile?.head;
  if (!outgoingProfile || !incomingProfile || !outTail || !inHead) {
    return { type: 'sweet-fade', overlapSeconds: maxFade };
  }

  // Rule 1: the recording already separates — do not fade over silence.
  if (trailingSilenceSeconds(outTail, secondsPerBucket) >= MIN_TRAILING_SILENCE_SECONDS) {
    return { type: 'gapless' };
  }

  // Rule 2: consecutive album tracks joining at full energy are meant to be
  // heard continuously — keep the sample-exact handoff even with crossfade
  // enabled. (Cross-context joins fall through to Rule 5.)
  if (query.sameRelease === true && isCleanLoudEnding(outTail, secondsPerBucket)) {
    return { type: 'gapless' };
  }

  // Rule 5: abrupt loud ending straight into a fast attack (e.g. shuffle
  // between two full-energy tracks) would sum two full-energy signals.
  if (
    isCleanLoudEnding(outTail, secondsPerBucket) &&
    isFastAttack(inHead, secondsPerBucket)
  ) {
    return { type: 'hard-cut' };
  }

  // Rule 4: sweet fade, hugged to the audible outro.
  let overlap = Math.min(maxFade, FADE_BASE_SECONDS);
  const decay = decaySeconds(outTail, secondsPerBucket);
  overlap = Math.min(overlap, decay + OUTRO_GRACE_SECONDS);
  // Never below the minimum useful fade, never above the user cap.
  overlap = Math.max(Math.min(FADE_MIN_SECONDS, maxFade), overlap);
  overlap = Math.min(overlap, maxFade);
  // Clamp against absurdly short sources (jingles): never more than half
  // of either side.
  const limit = Math.min(
    outgoingProfile.lengthSeconds > 0 ? outgoingProfile.lengthSeconds / 2 : overlap,
    incomingProfile.lengthSeconds > 0 ? incomingProfile.lengthSeconds / 2 : overlap,
  );
  overlap = Math.max(0, Math.min(overlap, limit));
  return { type: 'sweet-fade', overlapSeconds: Math.round(overlap * 100) / 100 };
}
