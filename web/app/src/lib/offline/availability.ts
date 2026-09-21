// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Offline availability (plan §6/§7.4): the bridge between the committed
// download catalog and Phase-4 representation selection.
//
// The ONE policy stays resolveAudio(); this module only answers "is that
// candidate locally usable?" through SelectionContext.canPlay, exactly as
// docs/representation-selection-phase4.md anticipated ("Offline playback
// can reuse the resolver by injecting local-availability as canPlay").
//
// Decision D1 (local-first): an installed package's selected assets play
// from local storage online AND offline; remote sources serve only
// non-installed content.

import { writable } from '../store';
import type {
  InstalledPackage,
} from './types';
import type { CatalogStore } from './stores';

/** What selection needs to know about one candidate audio object. */
export interface CandidateRef {
  kind: 'primary' | 'representation';
  representationId?: number;
}

/** Presentation lifecycle (UI-facing; the catalog's PackageStatus is the
 *  storage-level truth). 'damaged' is set by the manager when the boot
 *  audit stripped assets from a committed record — it is deliberately
 *  distinct from 'stale' (server content changed): healing both is a fresh
 *  install, but the copy differs ("Needs repair" vs "Update available"). */
export type PackageUiState =
  | { state: 'not-installed' }
  | { state: 'downloading'; percent: number }
  | { state: 'installed' }
  | { state: 'stale' }
  | { state: 'damaged' }
  | { state: 'failed'; reason: string };

export function createAvailability(catalog: CatalogStore) {
  const installed = new Map<number, InstalledPackage>();
  const ready = writable(false);

  async function hydrate(): Promise<void> {
    for (const pkg of await catalog.allPackages()) {
      if (pkg.status === 'installed') installed.set(pkg.releaseId, pkg);
    }
    ready.set(true);
  }

  /** True when the track's candidate is locally available and undamaged
   *  (D1: local-first). Composed with browser capability by the host. */
  function allows(trackId: number, candidate: CandidateRef): boolean {
    return findAsset(trackId, candidate) !== null;
  }

  function findAsset(
    trackId: number,
    candidate: CandidateRef,
  ): InstalledPackage['assets'][number] | null {
    for (const pkg of installed.values()) {
      for (const asset of pkg.assets) {
        if (asset.trackId !== trackId || asset.state !== 'ok') continue;
        if (candidate.kind === 'primary' && asset.kind === 'audio-primary') return asset;
        if (
          candidate.kind === 'representation' &&
          asset.kind === 'audio-representation' &&
          asset.representationId === candidate.representationId
        ) {
          return asset;
        }
      }
    }
    return null;
  }

  /** Resolves the local file key for one candidate, or null when the
   *  candidate is not locally held. Used at item-construction time to
   *  emit `source.kind = 'local-file'` under the local-first rule. */
  function localKeyFor(trackId: number, candidate: CandidateRef): string | null {
    return findAsset(trackId, candidate)?.key ?? null;
  }

  function packageFor(releaseId: number): InstalledPackage | null {
    return installed.get(releaseId) ?? null;
  }

  /** True when at least one installed package exists (offline-session
   *  eligibility). */
  function hasInstalled(): boolean {
    return installed.size > 0;
  }

  /** The committed local key of one track's lyric document (R3.6), or
   *  null when it is not locally held or is damaged. Lyrics are auxiliary
   *  content, not an audio selection candidate, so they get this narrow
   *  lookup instead of joining `CandidateRef`: the offline LyricsPanel
   *  reads bytes through it, and a damaged lyric has no addressable bytes
   *  by construction. */
  function lyricKeyFor(trackId: number, lyricsId: number): string | null {
    for (const pkg of installed.values()) {
      for (const asset of pkg.assets) {
        if (asset.state !== 'ok' || asset.kind !== 'lyrics') continue;
        if (asset.trackId === trackId && asset.lyricsId === lyricsId) {
          const { key } = asset;
          return key;
        }
      }
    }
    return null;
  }

  function forget(releaseId: number): void {
    installed.delete(releaseId);
  }

  function remember(pkg: InstalledPackage): void {
    if (pkg.status === 'installed') installed.set(pkg.releaseId, pkg);
  }

  /** Album ids that hold at least one installed release (shelf badges and
   *  the "Available offline" filter). Derived from the committed records'
   *  release snapshots — no second index, no server parameter. */
  function installedAlbumIds(): Set<number> {
    const out = new Set<number>();
    for (const pkg of installed.values()) {
      const albumId = pkg.releaseDetail?.album?.id;
      if (typeof albumId === 'number') out.add(albumId);
    }
    return out;
  }

  return {
    hydrate,
    ready,
    allows,
    localKeyFor,
    lyricKeyFor,
    packageFor,
    hasInstalled,
    installedAlbumIds,
    forget,
    remember,
    /** Test/dev seam: drop all in-memory state (does not touch storage). */
    clearMemory(): void {
      installed.clear();
    },
  };
}

export type Availability = ReturnType<typeof createAvailability>;
