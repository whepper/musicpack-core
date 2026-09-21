// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The playback engine port (player-core M2).
//
// A host-implementable abstraction over "the thing that turns one or more
// queued sources into audible, seekable output". The web hosts implement it
// with AudioContext/AudioWorklet/WASM (musepack) and HTMLMediaElement
// (native codecs); future Android/iOS hosts implement it natively. The core
// orchestrator knows nothing about audio plumbing beyond this contract.
//
// Honesty rules:
// - Capabilities declare what an engine actually does. Nothing here claims
//   sample-accurate gapless for element-based playback.
// - DecodeGate is a capability, not part of Engine: priming requires
//   decoding while the output clock is still stopped (a suspended
//   AudioContext pulls no render callbacks), so folding pump control into
//   play()/pause() would deadlock priming on pull-style pipelines.
// - All positions are OUTPUT-rate samples within the currently open track.
//   Source-rate conversion stays engine-internal.

import type { PlaybackItem, StreamInfo } from './types';

/** Backend families a host may select per item. `musepack` and `rust` are
 *  pull-decode demand-range engines whose reported position is session-relative
 *  (reset on open/seek, monotonic across gapless handoffs); `native` is
 *  element-backed and reports absolute within-track time. This is a host label
 *  only — the core learns nothing about a backend's internals. */
export type EngineKind = 'musepack' | 'native' | 'rust';

export interface EngineCapabilities {
  /** prepareNext()/advance() standby support. */
  readonly preloadNext: boolean;
  /** True only when track handoff is sample-exact (Musepack pipeline). */
  readonly sampleAccurateGapless: boolean;
  /** DecodeGate control is meaningful (pull-style decode pipelines). */
  readonly decodeGate: boolean;
  /** beginCrossfade() overlapped transitions are supported (M8). */
  readonly crossfade: boolean;
}

/** Events an engine reports back. Delivered asynchronously, in order,
 *  tagged with the engine instance (see plan §A.5 delivery rules). */
export type EngineEventName = 'primed' | 'buffering' | 'eos' | 'error' | 'tick';

export interface EngineEvents {
  /** Enough output is buffered to start/keep the clock running. */
  primed(): void;
  /** Output starved; playback cannot continue at full rate. */
  buffering(): void;
  /** The current source finished decoding/reached its end. */
  eos(): void;
  error(message: string): void;
  /** Coarse position change; may be coalesced by either side. */
  tick(): void;
}

export interface Engine {
  readonly capabilities: EngineCapabilities;

  init(authToken: string | null): Promise<void>;
  /** Opens a source; resolves once stream facts are known. */
  open(item: PlaybackItem): Promise<StreamInfo>;
  play(): Promise<void>;
  pause(): Promise<void>;
  /** Seeks within the OPEN track (output-rate samples). */
  seekSample(samples: number): Promise<void>;
  /** Combined linear gain (user volume × normalization). */
  setGain(linear: number): void;
  /** Output-rate frames rendered since the last open/seek reset. */
  renderedSamples(): number;
  close(): Promise<void>;

  on(name: EngineEventName, cb: (sender: Engine) => void): void;
}

/** Standby/preload capability: open the next source ahead of time and
 *  promote it at the boundary without re-opening the output graph.
 *
 *  Standby/policy agreement: `advance(expected)` receives the item the
 *  core's CURRENT queue policy selected (peeked at promotion time, not when
 *  the standby was prepared). The standby may be promoted only when it was
 *  prepared for exactly that item (`sameItemIdentity`); otherwise — or when
 *  `expected` is `null`, meaning nothing may follow — the engine returns
 *  `null` WITHOUT flushing or promoting, so a stale lane can never bleed
 *  into output or metadata. The caller recovers by loading `expected`
 *  fresh. */
export interface PreloadEngine extends Engine {
  prepareNext(item: PlaybackItem): Promise<StreamInfo | null>;
  advance(expected: PlaybackItem | null): Promise<StreamInfo | null>;
}

/** Result of a taken crossfade transition. */
export interface CrossfadeResult {
  /** The incoming track's stream facts (as from open()/advance()). */
  info: StreamInfo;
  /** Output-rate frames of overlap actually mixed. The caller shrinks the
   *  outgoing track's effective length by this so offsets, positions and
   *  end-of-queue detection stay truthful after the compressed boundary. */
  overlapFrames: number;
}

/** Crossfade capability (M8, opt-in). An engine implementing this can
 *  transition to `next` with an equal-power overlap instead of the normal
 *  sequential handoff. Returning null (or not implementing) makes the
 *  caller fall back to the normal EOS path — crossfade is best-effort by
 *  design. The standby/policy-agreement rule applies here too: `next` is
 *  the core's current policy target, and a standby prepared for a different
 *  item must be refused (null), never faded in. After a taken fade the
 *  engine suppresses the OLD lane's 'eos'; the new lane's lifecycle
 *  (eos/position) is the engine's responsibility, exactly as after a normal
 *  advance(). */
export interface CrossfadeEngine {
  beginCrossfade(next: PlaybackItem, fadeSeconds: number): Promise<CrossfadeResult | null>;
}

/** Pull-decode gate capability (e.g. the WASM musepack pipeline). */
export interface DecodeGate {
  start(): void;
  stop(): void;
}

export function isPreloadEngine(e: Engine): e is PreloadEngine {
  return e.capabilities.preloadNext && 'prepareNext' in e && 'advance' in e;
}

export function decodeGateOf(e: Engine): DecodeGate | null {
  if (!e.capabilities.decodeGate) return null;
  const g = e as unknown as Partial<DecodeGate>;
  return typeof g.start === 'function' && typeof g.stop === 'function' ? (e as unknown as DecodeGate) : null;
}
