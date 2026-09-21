// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Offline manager (plan §6/§9/§12): the single composition point that owns
// the catalog + file store, exposes install/cancel/remove/update lifecycle,
// and publishes reactive UI state. The playback path consumes ONLY
// `availability` (local-first source rule); the UI consumes the stores.

import { writable } from '../store';
import type { Readable } from 'svelte/store';
import { Installer } from './installer';
import type { DownloadProgress, InstalledPackage } from './types';
import { createAvailability } from './availability';
import { memoryCatalog } from './stores';
import { idbCatalog, opfsFileStore } from './browser-stores';
import type { CatalogStore, FileStore } from './stores';
import type { ReleaseDetail } from '../api/types';
import type { PackageUiState } from './availability';
import { auditAll, applyAudit } from './audit';
import { lyricsAssetKey } from './plan';

/** Feature detection (plan §3 graceful degradation): without OPFS the
 *  offline subsystem stays disabled and the app behaves exactly as an
 *  online-only client. */
export function offlineSupported(): boolean {
  return (
    typeof navigator !== 'undefined' &&
    !!navigator.storage?.getDirectory &&
    typeof indexedDB !== 'undefined'
  );
}

export interface OfflineManagerOptions {
  fileStore?: FileStore;
  catalog?: CatalogStore;
  fetch?: typeof fetch;
}

function defaultStores(): { fileStore: FileStore; catalog: CatalogStore } | null {
  if (!offlineSupported()) return null;
  try {
    return { fileStore: opfsFileStore(), catalog: idbCatalog() };
  } catch {
    return null;
  }
}

export function createOfflineManager(opts: OfflineManagerOptions = {}) {
  const stores = opts.fileStore && opts.catalog
    ? { fileStore: opts.fileStore, catalog: opts.catalog }
    : defaultStores();
  const enabled = stores !== null;
  const availability = createAvailability(stores?.catalog ?? memoryCatalog());


  const installer = stores
    ? new Installer({
        fileStore: stores.fileStore,
        catalog: stores.catalog,
        now: () => Date.now(),
        fetch: opts.fetch ?? ((u, i) => fetch(u, i)),
        onProgress: (p) => progress.set([p, ...progress.get().filter((x) => x.releaseId !== p.releaseId)]),
      })
    : null;
  let persistRequested = false;

  /** Per-release UI state, kept out of the domain record shape on purpose:
   *  this is presentation state derived from catalog + live activity. */
  const uiStates = writable<Map<number, PackageUiState>>(new Map());
  const progress = writable<DownloadProgress[]>([]);
  /** Releases the boot audit found damaged (in-memory presentation
   *  marker; any later committed install clears it via setState). */
  const damagedReleases = new Set<number>();

  function setState(releaseId: number, s: PackageUiState): void {
    if (s.state === 'damaged') damagedReleases.add(releaseId);
    else damagedReleases.delete(releaseId);
    uiStates.update((m) => {
      const next = new Map(m);
      next.set(releaseId, s);
      return next;
    });
  }

  async function refreshState(releaseId: number): Promise<void> {
    const pkg = await stores!.catalog.getPackage(releaseId);
    if (!pkg) {
      setState(releaseId, { state: 'not-installed' });
      return;
    }
    if (pkg.status === 'installed') {
      // Audit-damaged records are flagged by the boot audit (assets were
      // stripped, stale set); present them distinctly from a server-side
      // content update so the UI can offer "Reinstall" with the right copy.
      if (damagedReleases.has(releaseId)) {
        setState(releaseId, { state: 'damaged' });
      } else {
        setState(releaseId, pkg.stale === true ? { state: 'stale' } : { state: 'installed' });
      }
    } else {
      setState(releaseId, { state: 'failed', reason: pkg.error ?? 'unknown error' });
    }
  }

  /** Boot-time initialization: hydrate availability, sweep orphaned
   *  staging subtrees from interrupted installs, and audit committed
   *  packages against on-disk reality (browser eviction detection). */
  async function init(): Promise<void> {
    if (!enabled || !installer || !stores) return;
    try {
      await availability.hydrate();
      // Orphan sweep: staging subtrees with no live install are garbage.
      for (const installId of await stores.fileStore.listStaging()) {
        await stores.fileStore.removeStaging(installId);
      }
      // Audit: reconcile catalog vs files; damaged assets leave the record
      // so availability can never point at vanished bytes.
      const damagedIds = new Set<number>();
      for (const result of await auditAll(stores.catalog, stores.fileStore)) {
        if (result.verdict === 'damaged') {
          damagedIds.add(result.releaseId);
          const pkg = await stores.catalog.getPackage(result.releaseId);
          if (pkg) {
            const updated = applyAudit(pkg, result, Date.now());
            await stores.catalog.putPackage(updated);
            availability.remember(updated);
          }
        }
      }
      for (const releaseId of [...(await stores.catalog.allPackages()).map((p) => p.releaseId)]) {
        if (damagedIds.has(releaseId)) {
          // Presentation marker for the audit verdict; the catalog record
          // itself stays storage-shaped (assets already stripped by the
          // audit apply). Cleared implicitly by any successful re-install.
          setState(releaseId, { state: 'damaged' });
          continue;
        }
        await refreshState(releaseId);
      }
    } catch {
      /* offline subsystem degraded: online behavior unaffected */
    }
  }

  async function install(releaseDetail: ReleaseDetail): Promise<void> {
    if (!installer || !stores) return;
    const releaseId = releaseDetail.id;
    setState(releaseId, { state: 'downloading', percent: 0 });
    // First-install persistence request (best effort, once per session).
    if (!persistRequested) {
      persistRequested = true;
      void import('./audit').then((m) => m.requestPersistence()).catch(() => {});
    }
    const handle = installer.install(releaseDetail);
    void handle.done.then((outcome) => {
      progress.set(progress.get().filter((p) => p.releaseId !== releaseId));
      if (outcome.ok) {
        // Availability is refreshed from the committed record.
        void stores.catalog.getPackage(releaseId).then((pkg) => {
          if (pkg) availability.remember(pkg);
          void refreshState(releaseId);
        });
      } else {
        void refreshState(releaseId);
      }
    });
    // Track progress percentages for the UI.
    const unsub = progress.subscribe((list) => {
      const p = list.find((x) => x.releaseId === releaseId);
      if (p && p.totalBytes > 0) {
        setState(releaseId, {
          state: 'downloading',
          percent: Math.min(99, Math.round((p.downloadedBytes / p.totalBytes) * 100)),
        });
      }
    });
    void handle.done.then(() => unsub());
  }

  async function cancel(releaseId: number): Promise<void> {
    installer?.cancel(releaseId);
    await refreshState(releaseId);
  }

  /** Explicit user-initiated removal (D2 companion: nothing is removed
   *  automatically except browser eviction). */
  async function remove(releaseId: number): Promise<void> {
    if (!stores) return;
    const pkg = await stores.catalog.getPackage(releaseId);
    if (pkg) {
      for (const asset of pkg.assets) await stores.fileStore.remove(asset.key);
    }
    await stores.catalog.deletePackage(releaseId);
    availability.forget(releaseId);
    await refreshState(releaseId);
  }

  /** Update check (decision D2): compare online hashes against the
   *  committed record and FLAG differences. Replacement happens only via
   *  explicit remove+install / reinstall by the user. */
  async function checkForUpdate(release: ReleaseDetail): Promise<boolean> {
    if (!stores) return false;
    const pkg = await stores.catalog.getPackage(release.id);
    if (!pkg || pkg.status !== 'installed') return false;
    const onlineHashes = collectHashes(release);
    const localHashes = new Map(pkg.assets.filter((a) => a.sha256).map((a) => [a.key, a.sha256!]));
    let stale = false;
    for (const [key, sha] of onlineHashes) {
      const local = localHashes.get(key);
      if (local && local !== sha) stale = true;
    }
    if (stale) {
      await stores.catalog.putPackage({ ...pkg, stale: true, updatedAt: Date.now() });
      await refreshState(release.id);
    }
    return stale;
  }

  return {
    enabled,
    availability,
    /** Reactive per-release UI states. */
    states: uiStates,
    /** Reactive download-progress snapshots. */
    downloads: progress,
    init,
    install,
    cancel,
    remove,
    checkForUpdate,
    packageFor: (releaseId: number): Promise<InstalledPackage | null> =>
      stores ? stores.catalog.getPackage(releaseId) : Promise.resolve(null),
    /** All committed package records (settings/storage panel rows). */
    listPackages: (): Promise<InstalledPackage[]> =>
      stores ? stores.catalog.allPackages() : Promise.resolve([]),
    /** Album ids with at least one installed release (shelf badges/filter). */
    installedAlbumIds: (): Set<number> => availability.installedAlbumIds(),
    /** Storage accounting for the future settings panel (§11). */
    storageUsage: () =>
      import('./audit').then((m) => m.storageUsage()),
  };
}

function collectHashes(release: ReleaseDetail): Map<string, string> {
  const out = new Map<string, string>();
  for (const disc of release.media) {
    for (const track of disc.tracks) {
      if (track.audio.sha256) out.set(`t.${track.id}.primary`, track.audio.sha256);
      for (const rep of track.representations ?? []) {
        if (rep.sha256) out.set(`t.${track.id}.r.${rep.id}`, rep.sha256);
      }
    }
  }
  // Track-linked lyrics (R3.6): the release-level index carries the
  // authoritative hashes, so a lyric replaced upstream flags the package
  // stale exactly like audio — the staged asset can never silently serve
  // old text. Same key builder as the planner (single source).
  for (const lyric of release.trackLyrics ?? []) {
    if (lyric.sha256) out.set(lyricsAssetKey(lyric.trackId, lyric.id), lyric.sha256);
  }
  return out;
}

export type OfflineManager = ReturnType<typeof createOfflineManager>;
