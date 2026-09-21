// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Platform-independent playback types (player-core M1).
//
// Purity laws (see web/player-core/README.md): this module must stay free of
// DOM / Svelte / Node / worker imports and ambient globals, and every type in
// a port signature must be JSON-representable. `PlaybackItem` is the
// platform-independent queue entry; the web `QueueItem` is its web-shaped
// alias (api/types.ts Track verbatim) until the orchestrator extraction (M4)
// moves the UI projection fully onto core events.

/** Where an engine obtains the bytes of one item. `http-range` is the
 *  demand-driven Musepack source (URL + total size); `stream` covers
 *  element/native playback where size is unknown or irrelevant;
 *  `local-file` addresses an offline-downloaded asset held in
 *  application-managed browser storage (`url` = the stable file key).
 *  Engines resolve each kind at their own boundary; the core never does. */
export interface PlaybackSource {
  kind: 'http-range' | 'stream' | 'local-file';
  url: string;
  byteSize?: number;
}

export interface PlaybackItem {
  /** Stable per queue entry; the identity the player commands and events
   *  use (queue indices are presentation, this is identity). */
  id: string;
  /** Server track row identity. Key of the exact-decoded-lengths cache. */
  trackId: number;
  source: PlaybackSource;
  /** Manifest duration hint in seconds; exact decoded length overrides it. */
  durationHintSeconds?: number;
  /** Display + integration metadata (Media Session / notifications). */
  title: string;
  artist: string;
  albumTitle: string;
  edition?: string;
  artworkUrl?: string;
  /** BS.1770 loudness for normalization policy (see gain.ts). */
  loudness?: { lufs: number; truePeakDb: number };
  albumLoudness?: { albumLufs: number; albumTruePeakDb: number };
  /** Codec hint for backend resolution, e.g. 'musepack-sv8' | 'flac'. */
  codec?: string;
  /** MIME type hint for browser-native capability probing. */
  mimeType?: string;
}

/** Stream facts an engine reports when a source is opened/advanced to.
 *  `rate`/`lengthSamples` are in the engine's OUTPUT timeline (the web
 *  MusepackEngine normalizes source rate -> AudioContext rate). */
export interface StreamInfo {
  rate: number;
  channels: number;
  /** Codec stream version (Musepack SV7 = 7, SV8 = 8; 0 when unknown). */
  version: number;
  lengthSamples: number;
}

/** Queue-entry identity (standby/policy agreement): two entries describe
 *  the same playable item iff both the queue-entry id and the server track
 *  id match — the same rule `beginCrossfadeTransition` uses to relocate its
 *  target after a fade. Reorders keep it; replacements break it. */
export function sameItemIdentity(a: PlaybackItem, b: PlaybackItem): boolean {
  return a.id === b.id && a.trackId === b.trackId;
}
