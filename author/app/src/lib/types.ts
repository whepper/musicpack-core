// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Types for the MusicPack Author draft model and the Tauri command surface.
//
// The draft is application state (not a MusicPack format): it mirrors the
// .mpack v1 logical hierarchy but references files under `sourceRoot` and
// keeps release, source and identity separate, exactly as the spec requires.
// Everything here maps 1:1 onto the `musicpack` CLI JSON modes.

export interface Artist {
  name: string;
  role?: string;
}

export interface AlbumGroup {
  title: string;
  artists: Artist[];
  releaseType?: string;
  originalReleaseDate?: string;
  genres?: string[];
}

export interface ReleaseEdition {
  releaseDate?: string;
  edition?: string;
  country?: string;
  label?: string;
  catalogueNumber?: string;
  notes?: string;
}

export interface Identifiers {
  musicbrainzReleaseGroupId?: string;
  musicbrainzReleaseId?: string;
  barcode?: string;
}

export type IdentitySource = 'musicbrainz' | 'store' | 'local';
export type Confidence = 'exact' | 'confirmed' | 'probable' | 'none';

export interface Identity {
  source?: IdentitySource;
  confidence?: Confidence;
}

export interface SourceInfo {
  type?: string;
  store?: string;
  sourceId?: string;
}

export interface TrackSource {
  store?: string;
  trackId?: string;
}

export interface SourceAudio {
  codec?: string;
  md5?: string;
}

export interface TrackIdentifiers {
  isrc?: string;
  musicbrainzTrackId?: string;
  musicbrainzRecordingId?: string;
}

export interface Track {
  track: number;
  title: string;
  artists?: Artist[];
  identifiers?: TrackIdentifiers;
  source?: TrackSource;
  sourceAudio?: SourceAudio;
  duration?: number;
  codec?: string;
  streamVersion?: number;
  sampleRate?: number;
  channels?: number;
  bitDepth?: number;
  /** source file path relative to the draft's sourceRoot */
  audioPath: string;
  /** Per-track lyric documents (R3.5, docs/musicpack-lyrics-v1.md §6.1).
   *  Each entry references a lyric source file relative to the draft's
   *  sourceRoot; the backend attaches them as the manifest's
   *  `track.lyrics[]` at build. Omitted/empty = lyric-less (the server
   *  omits the field the same way). Never confused with the
   *  package-level `Draft.lyrics` (root assets). */
  lyrics?: TrackLyricRef[];
}

/** One track-linked lyric document reference in the authoring draft. The
 *  content hash is computed at build (never authored); `lang` is the
 *  optional language tag (BCP-47 recommended, free-form per spec §5). */
export interface TrackLyricRef {
  path: string;
  lang?: string;
}

/** Result of the `lyrics_probe` command: immediate validation feedback
 *  for the track editor. Probe failures are data (`ok: false`), never
 *  command errors; the authoritative gates are `validate-draft` and the
 *  build. */
export interface LyricsProbeResult {
  ok: boolean;
  /** True when the document carries line timestamps (spec §3). */
  synced?: boolean;
  /** Renderable line count (timed lines, or plain content lines). */
  lines?: number;
  error?: { code?: string; message?: string };
}

export interface Medium {
  disc: number;
  format?: string;
  title?: string;
  tracks: Track[];
}

export interface ArtworkEntry {
  role: string;
  path?: string;
  embedded?: boolean;
  sourceAudio?: string;
  mime?: string;
}

export interface AssetEntry {
  path: string;
}

/** Sonic analysis state carried by the authoring draft (application state,
 * not part of the .mpack manifest). The completed document lives outside the
 * package until build; `path` points at it for create_package to attach. */
export interface SonicAnalysis {
  status: 'not_analysed' | 'pending' | 'ready' | 'ready-with-warnings' | 'error';
  profile?: string;
  path?: string;
  tracksAnalysed?: number;
  tracksTotal?: number;
  warnings?: string[];
  error?: string;
}

/** Waveform envelope state carried by the authoring draft (application state,
 * not part of the .mpack manifest). Per-track envelopes live in a staging
 * directory until build; `tracks` carries the per-track `points`, `path`,
 * and `sha256` that build-draft will attach as the manifest `waveform`
 * references. */
export interface WaveformTrackResult {
  disc: number;
  track: number;
  points: number;
  sha256: string;
  path: string;
}

export interface WaveformAnalysis {
  status: 'not_generated' | 'pending' | 'ready' | 'disabled' | 'error';
  intervalMs: number;
  encoding: 'peak-rms-u8';
  floorDb: number;
  tracks: WaveformTrackResult[];
  tracksGenerated?: number;
  tracksTotal?: number;
  error?: string;
}

export interface Draft {
  schema: 'musicpack-draft';
  version: 1;
  sourceRoot: string;
  album: AlbumGroup;
  release?: ReleaseEdition;
  identifiers?: Identifiers;
  identity?: Identity;
  source?: SourceInfo;
  media: Medium[];
  artwork: ArtworkEntry[];
  booklet: AssetEntry[];
  lyrics: AssetEntry[];
  extras: AssetEntry[];
  sonicAnalysis?: SonicAnalysis;
  waveformAnalysis?: WaveformAnalysis;
  /** Set when the draft was opened from an existing .mpack package:
   * the package path. Audio is already encoded; saving writes back to
   * this package in place instead of creating a new one. */
  openedFrom?: string;
}

export const RELEASE_TYPES = [
  'album',
  'ep',
  'single',
  'maxi-single',
  'compilation',
  'soundtrack',
  'live-album',
  'remix-album',
  'box-set',
  'other',
] as const;

export const MEDIUM_FORMATS = [
  'CD',
  'SACD',
  'Vinyl',
  'Cassette',
  'Digital',
  'Blu-ray Audio',
  'DVD-Audio',
  'Other',
] as const;

export const ARTWORK_ROLES = [
  'front',
  'back',
  'medium',
  'booklet-page',
  'other',
] as const;

export const SOURCE_TYPES = [
  'cd-rip',
  'digital-download',
  'vinyl-rip',
  'tape-rip',
  'other',
] as const;

// ---- command results ------------------------------------------------------

export interface ValidationResult {
  ok: boolean;
  errors: string[];
  warnings: string[];
}

export interface IdentifyCandidate {
  releaseId?: string;
  releaseGroupId?: string;
  title?: string;
  artist?: string;
  date?: string;
  country?: string;
  barcode?: string;
  confidence: Confidence;
}

export type IdentifyResult =
  | { kind: 'candidates'; candidates: IdentifyCandidate[] }
  | { kind: 'applied'; draft: Draft; confidence: Confidence; applied: boolean };

export interface CreateResult {
  ok: boolean;
  outputPath?: string;
  /** True when an existing .mpack was rebuilt in place (--replace). */
  replaced?: boolean;
  verify?: { errors: number; warnings: number };
  error?: { code?: string; message?: string };
}

/** Result of packing a `.mpack` directory into a single-file `.mpak`
 * container (either a fresh build or a conversion of an existing package). */
export interface PackResult {
  ok: boolean;
  outputPath?: string;
  error?: { code?: string; message?: string };
}

/** Output packaging form chosen in the Create dialog. `.mpack` is the
 * directory package (the default authoring output); `.mpak` is the
 * deterministic single-file container built from a verified `.mpack`. */
export type PackageFormat = 'mpack' | 'mpak';

export interface ReadImageResult {
  mime: string;
  dataBase64: string;
}

/** Result of a sonic analysis run. `cancelled` is distinct from failure. */
export interface SonicResult {
  ok: boolean;
  cancelled?: boolean;
  profile?: string;
  outputPath?: string;
  sha256?: string;
  tracks?: number;
  contributing?: number;
}

/** Persistent Sonic model state (idle view). */
export type ModelState =
  | 'missing'
  | 'checking'
  | 'downloading'
  | 'verifying'
  | 'ready'
  | 'error';

export interface ModelStatus {
  profile: string;
  state: ModelState;
  path?: string;
  sizeBytes: number;
}

/** A progress event emitted during sonic analysis / model acquisition. */
export interface SonicProgress {
  event: 'model' | 'track' | 'album' | 'done' | 'error' | 'cancelled';
  state?: 'checking' | 'downloading' | 'verifying' | 'ready';
  path?: string;
  downloaded?: number;
  done?: number;
  total?: number;
  disc?: number;
  track?: number;
  status?: 'ok' | 'no-embedding' | 'error';
  code?: string;
  message?: string;
  contributing?: number;
  sha256?: string;
}

/** Result of a waveform envelope generation run. `cancelled` is distinct
 * from failure; `tracks` is the count of envelopes successfully written
 * into the staging directory; the transformed `draft` carries the
 * `waveformAnalysis` block that build-draft will attach as manifest
 * `waveform` references. */
export interface WaveformResult {
  ok: boolean;
  cancelled?: boolean;
  stagingDir?: string;
  tracks?: number;
  draft?: Draft;
}

/** Progress event emitted during waveform envelope generation. The
 * protocol mirrors `encode-progress` for Author UI consistency. */
export interface WaveformProgress {
  event: 'stage' | 'track' | 'done' | 'error' | 'cancelled';
  stage?: 'decoding';
  done?: number;
  total?: number;
  disc?: number;
  track?: number;
  title?: string;
  status?: 'ok';
  points?: number;
  sha256?: string;
  path?: string;
  code?: string;
  message?: string;
}

/** Typed error rejection from the sonic commands: `{ code, message }`. */
export interface SonicError {
  code?:
    | 'model_missing'
    | 'download_failed'
    | 'checksum_mismatch'
    | 'offline'
    | 'download_cancelled'
    | 'analyzer_unavailable'
    | 'runtime_dependency_missing'
    | 'analysis_failed'
    | 'sonic_retired';
  message?: string;
  /** Captured backend stderr tail, shown in a details expander. */
  details?: string;
}

export interface RecentAlbum {
  path: string;
  title?: string | null;
  lastOpenedMs: number;
}

export interface BackendInfo {
  musicpackVersion: string;
  authorApi: number;
  location: 'bundled' | 'development' | 'rust';
}

export interface IdentifyOptions {
  mbid?: string;
  barcode?: string;
  mbJson?: string;
}

// ---- FLAC -> Musepack encode stage ----------------------------------------

/** A progress event emitted during the encode stage (mirrors the CLI
 * `encode-draft` progress protocol). */
export interface EncodeProgress {
  event: 'stage' | 'track' | 'done' | 'error' | 'cancelled';
  stage?: 'decoding' | 'encoding' | 'tagging';
  done?: number;
  total?: number;
  disc?: number;
  track?: number;
  title?: string;
  status?: 'ok' | 'error';
  code?: string;
  message?: string;
  sha256?: string;
  duration?: number;
}

/** Result of an encode run. On success `draft` is the transformed draft
 * whose audioPath values point at the encoded .mpc files in `outputDir`
 * (kept until the package build; removed by `cleanupStaging`). */
export interface EncodeResult {
  ok: boolean;
  cancelled?: boolean;
  outputDir?: string;
  tracks?: number;
  draft?: Draft;
}
