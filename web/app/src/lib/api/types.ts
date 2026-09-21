// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Typed views of the MusicPack HTTP API v1 resources (specs/musicpack-api-v1.md).

export interface ArtistRef {
  id: number;
  name: string;
  role?: string;
}

export interface ArtworkRef {
  id: number;
  url: string;
  mimeType?: string;
  kind?: string;
  role?: string;
  /** Manifest content hash (additive; strong ETag of the asset endpoint). */
  sha256?: string;
}

export interface AlbumSummary {
  id: number;
  title: string;
  releaseType?: string;
  originalReleaseDate?: string;
  genres?: string[];
  artists: ArtistRef[];
  releaseCount: number;
  artwork?: ArtworkRef;
}

export interface AlbumPage {
  albums: AlbumSummary[];
  limit: number;
  offset: number;
  total: number;
}

export interface ReleaseSummary {
  id: number;
  edition?: string;
  releaseDate?: string;
  country?: string;
  label?: string;
  catalogueNumber?: string;
  barcode?: string;
  mbid?: string;
  identitySource?: string;
  identityConfidence?: string;
  trackCount: number;
  media: string[];
  artwork?: ArtworkRef;
  packageStatus?: string;
  verifyStatus?: string;
}

export interface AlbumDetail {
  album: {
    id: number;
    title: string;
    releaseType?: string;
    originalReleaseDate?: string;
    mbid?: string;
    genres?: string[];
    artists: ArtistRef[];
  };
  releases: ReleaseSummary[];
}

export interface Loudness {
  lufs: number;
  truePeakDb: number;
}

export interface AlbumLoudness {
  algorithm?: string;
  albumLufs: number;
  albumTruePeakDb: number;
}

export interface CodecInfo {
  codec: string;
  mimeType: string;
  streamVersion?: number;
  sampleRate?: number;
  channels?: number;
}

export interface AudioRef {
  id: number;
  size: number;
  sha256?: string;
  url: string;
}

export interface WaveformRef {
  version: number;
  intervalMs: number;
  encoding: string;
  floorDb: number;
  points: number;
  /** Manifest content hash (additive; strong ETag of the waveform endpoint). */
  sha256?: string;
  url: string;
}

export interface Track {
  id: number;
  number: number;
  title: string;
  artists: ArtistRef[];
  isrc?: string;
  duration?: number;
  loudness?: Loudness;
  codec: CodecInfo;
  audio: AudioRef;
  /// Optional waveform envelope (see specs/musicpack-waveform-v1.md).
  /// `null` (or undefined) means the track has no waveform — the player
  /// falls back to the linear `<input type="range">` seek control.
  waveform?: WaveformRef | null;
  /// Optional alternate audio representations (Phase 3). The default
  /// remains `audio`; the client currently plays only the default, so
  /// this field is display/selection metadata. Omitted when empty.
  representations?: RepresentationRef[];
  /// Optional track-linked lyric documents (R3.3/R3.4,
  /// docs/musicpack-lyrics-v1.md §7.3). Present only on **track detail**
  /// (the server keeps track lists unchanged); omitted — never null, never
  /// empty — when the track has none. Array order is the server's
  /// deterministic selection order (first entry wins, spec §5).
  lyrics?: LyricsRef[];
}

/** One track-linked lyric document reference (R3.3 wire contract).
 *  Bytes flow through `url` (the existing asset endpoint); `sha256` is
 *  the manifest content hash (additive rule, strong ETag); `lang` is the
  * optional free-form language tag. */
export interface LyricsRef {
  id: number;
  url: string;
  size: number;
  mimeType: string;
  sha256?: string;
  lang?: string;
}

/** `GET /api/v1/tracks/{id}` returns the `Track` fields flattened at the
 *  top level with a `context` block appended (owning release/album, so the
 *  track page can deep-link back without a second round trip). The client
 *  normalizes that wire shape into this nested type at the API boundary:
 *  components read `detail.track` for the track and `detail.context` for
 *  where it lives. */
export interface TrackDetail {
  track: Track;
  context: {
    disc: number;
    albumId: number;
    albumTitle: string;
    releaseId: number;
    releaseEdition?: string;
  };
}

export interface RepresentationRef {
  id: number;
  size: number;
  /** Manifest content hash (additive; strong ETag of the variant endpoint). */
  sha256?: string;
  url: string;
  codec: CodecInfo;
  label?: string;
}

export interface MediaDisc {
  disc: number;
  format?: string;
  title?: string;
  tracks: Track[];
}

export interface AssetRef {
  id: number;
  kind: string;
  role?: string;
  mimeType: string;
  /** Manifest content hash (additive; strong ETag of the asset endpoint). */
  sha256?: string;
  url: string;
}

export interface ReleaseDetail {
  id: number;
  edition?: string;
  releaseDate?: string;
  country?: string;
  label?: string;
  catalogueNumber?: string;
  barcode?: string;
  mbid?: string;
  identitySource?: string;
  identityConfidence?: string;
  sourceType?: string;
  sourceStore?: string;
  sourceId?: string;
  provenanceTool?: string;
  provenanceToolVersion?: string;
  notes?: string;
  packageStatus?: string;
  verifyStatus?: string;
  loudness?: AlbumLoudness;
  album: {
    id: number;
    title: string;
    releaseType?: string;
    originalReleaseDate?: string;
    mbid?: string;
    artists: ArtistRef[];
  };
  media: MediaDisc[];
  artwork: ArtworkRef[];
  assets: AssetRef[];
  /** Release-level track-lyric index for offline planning (R3.6,
   *  docs/musicpack-lyrics-v1.md §7.4). The same underlying track-level
   *  assets as `TrackDetail.lyrics[]`, projected flat so the pure offline
   *  planner never fetches track details; omitted when the release has no
   *  track lyrics. See {@link TrackLyricsRef}. */
  trackLyrics?: TrackLyricsRef[];
}

/** One entry of the release-level `trackLyrics[]` index. `sha256` is
 *  mandatory here (offline staging verifies bytes against it), `lang`
 *  stays optional metadata, and array order is the server's
 *  deterministic `(trackId, id)` order (the offline planner keys assets
 *  by identity, never by position). */
export interface TrackLyricsRef {
  trackId: number;
  id: number;
  url: string;
  size: number;
  sha256: string;
  lang?: string;
}

export interface ArtistSummary {
  id: number;
  name: string;
  albumCount: number;
}

export interface ArtistPage {
  artists: ArtistSummary[];
  limit: number;
  offset: number;
  total: number;
}

export interface ArtistDetail {
  id: number;
  name: string;
  albums: Array<{
    id: number;
    title: string;
    releaseType?: string;
    originalReleaseDate?: string;
    artwork?: ArtworkRef;
  }>;
}

export interface SessionInfo {
  id?: number;
  createdAt?: string;
  expiresAt?: string;
}

export interface LibraryStatus {
  scan: {
    running: number;
    startedAt?: string;
    finishedAt?: string;
    packagesScanned?: number;
    added?: number;
    updated?: number;
    removed?: number;
    invalid?: number;
  };
  verify: {
    running: number;
    startedAt?: string;
    finishedAt?: string;
    packagesVerified?: number;
    passed?: number;
    warnings?: number;
    failed?: number;
  };
}
