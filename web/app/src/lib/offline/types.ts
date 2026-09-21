// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Offline catalog types (offline downloads, plan §4/§6).
//
// The catalog is the SINGLE source of truth for what is installed. The
// Installed-Package Usability Invariant: a package record is only visible
// to availability/selection/UI after one atomic commit that contains every
// playback-critical asset; staged/failed/orphaned files are invisible by
// construction. All storage handles are behind interfaces so the logic is
// Node-testable with in-memory fakes.

import type { ReleaseDetail } from '../api/types';

/** Lifecycle of an installed (or attempted) package record.
 *  - 'installed': committed + usable (the ONLY state selection consults)
 *  - 'downloading'/'failed': kept for UI progress/resume surfaces;
 *    never feed availability */
export type PackageStatus = 'downloading' | 'installed' | 'failed';

/** Per-asset state inside a committed package. 'damaged' assets were
 *  committed despite failing verification (policy allows damaged
 *  non-selected alternates); they are excluded from availability. */
export type AssetState = 'ok' | 'damaged';

export type OfflineAssetKind =
  | 'audio-primary'
  | 'audio-representation'
  | 'waveform'
  | 'artwork'
  | 'lyrics';

/** One downloadable byte asset of a release, resolved from the API detail
 *  objects (all carry url + size + optional sha256). */
export interface PlannedAsset {
  /** Stable per-record OPFS file name (never the server path). */
  key: string;
  kind: OfflineAssetKind;
  trackId?: number;
  representationId?: number;
  artworkId?: number;
  /** Lyric asset id (kind === 'lyrics'), R3.6. */
  lyricsId?: number;
  url: string;
  size: number;
  sha256?: string;
}

export interface PackageAssetRecord {
  key: string;
  kind: OfflineAssetKind;
  state: AssetState;
  sha256?: string;
  size: number;
  trackId?: number;
  representationId?: number;
  /** Artwork asset id (kind === 'artwork'). */
  artworkId?: number;
  /** Lyric asset id (kind === 'lyrics', R3.6): with `trackId`, the
   *  offline lookup identity for one track's lyric document. */
  lyricsId?: number;
}

export interface InstalledPackage {
  releaseId: number;
  status: PackageStatus;
  createdAt: number;
  updatedAt: number;
  /** Total bytes of committed audio assets (UI/storage accounting). */
  bytes: number;
  assets: PackageAssetRecord[];
  /** The release JSON snapshot at install time (offline library surface). */
  releaseDetail: ReleaseDetail;
  error?: string;
  /** Set by an update check when online hashes differ from the committed
   *  record (decision D2: flag only; the user chooses replacement). */
  stale?: boolean;
}

/** Progress reported during download (bytes of the current plan). */
export interface DownloadProgress {
  releaseId: number;
  downloadedBytes: number;
  totalBytes: number;
  doneAssets: number;
  totalAssets: number;
}

export function plannedAssetBytes(assets: PlannedAsset[]): number {
  return assets.reduce((n, a) => n + a.size, 0);
}
