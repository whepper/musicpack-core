// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Message contract between the playback controller (main thread) and the
// MusicPack PCM AudioWorklet processor.

export const RING_SECONDS = 8;       // main ring capacity
export const HIGH_WATER = 0.8;       // >= this fraction filled -> pause decode
export const LOW_WATER = 0.2;        // < this fraction -> resume decode
export const PRIME_FRACTION = 0.2;   // enough buffered before resuming the clock
/** Upper bound for the crossfade lane ring (covers the engine's 0.25–15 s
 *  fade clamp plus priming headroom). */
export const XLANE_MAX_SECONDS = 16;

/**
 * Physical capacity of the crossfade lane ring for a given fade window.
 * Always at least the main ring (small fades change nothing structurally),
 * and large enough to hold the WHOLE fade window plus producer margin so
 * long fades prime fully instead of being fed in real time. The engine's
 * readiness threshold must never exceed this capacity.
 */
export function laneCapacityFrames(
  outputRate: number,
  fadeFrames: number,
  marginFrames = 2048,
): number {
  const base = Math.round(outputRate * RING_SECONDS);
  const wanted = Math.ceil(fadeFrames * 1.25) + marginFrames;
  return Math.min(Math.max(base, wanted), Math.round(outputRate * XLANE_MAX_SECONDS));
}

export interface WorkletConfig {
  type: 'config';
  sourceRate: number;
  sourceChannels: number;
  outputRate: number;
  outputChannels: number;
  generation: number;
}

export interface WorkletTrack {
  type: 'track';
  sourceRate: number;
  sourceChannels: number;
  generation: number;
}

export interface WorkletSamples {
  type: 'samples';
  buffer: ArrayBuffer; // interleaved Float32Array, transferred
  generation: number;
}

export interface WorkletReset {
  type: 'reset';
  generation: number;
}

export interface WorkletEnd {
  type: 'end';
  generation: number;
}

// --- Crossfade lane (M8 Phase B). The lane carries the INCOMING track of a
// crossfade; the regular messages keep feeding the OUTGOING one. Mixing and
// the final ring swap happen inside the worklet. `token` disambiguates
// overlapping attempts after an aborted fade.

export interface WorkletXfade {
  type: 'xfade';
  sourceRate: number;
  sourceChannels: number;
  /** Length of the equal-power overlap in output-rate frames. */
  fadeFrames: number;
  token: number;
  generation: number;
}

export interface WorkletXsamples {
  type: 'xsamples';
  buffer: ArrayBuffer; // interleaved Float32Array, transferred
  /** Must match the armed/last-swapped lane; anything else is dropped. */
  token: number;
  generation: number;
}

export interface WorkletXend {
  type: 'xend';
  token: number;
  generation: number;
}

export interface WorkletXgo {
  type: 'xfade-go';
  token: number;
  generation: number;
}

export interface WorkletXcancel {
  type: 'xfade-cancel';
  generation: number;
}

// worklet -> main
export interface WorkletReport {
  type:
    | 'rendered'
    | 'primed'
    | 'need'
    | 'full'
    | 'underrun'
    | 'accepted'
    | 'trackEnded'
    | 'xfadeReady'
    | 'xfaded'
    | 'error';
  frames: number; // cumulative AudioContext-rate frames since reset
  generation: number;
  available?: number;
  message?: string;
  /** Which decode lane an `accepted` credit belongs to (default 1). */
  lane?: number;
  /** Echoed from the initiating `xfade` message; stale tokens are ignored. */
  token?: number;
  /** `xfaded`: frames consumed from the outgoing/incoming lane at the swap. */
  outgoingFrames?: number;
  incomingFrames?: number;
  /** `xfaded`: outgoing ring's own position when mixing began — the
   *  boundary itself, before any overlap. The rebase target that keeps the
   *  reported position aligned with the declared (overlap-compressed)
   *  offsets model on the caller side. */
  swapBaseFrames?: number;
  /** `xfaded`: true frames of the outgoing track's tail actually blended
   *  with the incoming lane (≤ fadeFrames; shorter when the outgoing ring
   *  ran dry before the fade window elapsed). This is what the caller
   *  should shrink the outgoing track's declared length by — NOT
   *  `incomingFrames`, which is (almost always) just the nominal fade
   *  window regardless of how much of the outgoing track it actually
   *  overlapped. */
  overlapFrames?: number;
}
