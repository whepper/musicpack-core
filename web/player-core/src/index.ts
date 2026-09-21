// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// @musicpack/player-core public entry point (M4).

export { writable } from './store';
export type { Readable, Writable } from './store';
export type { PlaybackItem, PlaybackSource, StreamInfo } from './types';
export {
  isPreloadEngine,
  decodeGateOf,
  type Engine,
  type EngineCapabilities,
  type EngineEventName,
  type EngineEvents,
  type EngineKind,
  type PreloadEngine,
  type DecodeGate,
} from './engine';
export { createQueueModel } from './queue';
export type { QueueModel, QueueState } from './queue';
export {
  shuffleOrder,
  nextIndexUnderRepeat,
  itemKey,
  type RepeatMode,
  type Rng,
} from './order';
export {
  PLAYBACK_TARGET_LUFS,
  TRUE_PEAK_CAP_DB,
  normalizationGainDb,
  dbToLinear,
  combinedGain,
  type NormalizationMode,
  type TrackLoudnessLike,
  type AlbumLoudnessLike,
} from './gain';
export {
  SNAPSHOT_VERSION,
  SNAPSHOT_VERSION_V1,
  clampIndex,
  decodeSnapshot,
  encodeSnapshot,
  type SessionSnapshot,
  type SessionSnapshotV1,
} from './snapshot';
export {
  Player,
  END_TOLERANCE_SAMPLES,
  type PlayerPorts,
  type PlayerOptions,
  type PlayerState,
  type PlayerModel,
  type MediaControlsPort,
  type StoragePort,
} from './player';
export { PlayerEventSink, type PlayerEvent, type PlayerListener } from './events';
