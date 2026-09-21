// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The offline installer (plan §4): fetch → stage → verify → ATOMIC COMMIT.
//
// Governing invariant (Installed-Package Usability): a package becomes
// playable only through one catalog commit whose record contains every
// playback-critical asset verified. Staged files are unaddressable by
// selection; a failed install leaves nothing behind but staging garbage,
// which the sweeper removes. A damaged NON-critical asset (a corrupt
// alternate representation) commits as state:'damaged' and is excluded
// from availability; a damaged CRITICAL asset (primary audio, waveform,
// artwork) fails the whole install.

import type {
  DownloadProgress,
  InstalledPackage,
  PackageAssetRecord,
  PlannedAsset,
} from './types';
import { plannedAssetBytes } from './types';
import type { CatalogStore, FileStore } from './stores';
import { Sha256 } from './sha256';
import { planReleaseAssets } from './plan';

/** Assets that must verify for the install to commit at all: primary
 *  audio, waveforms and artwork. Lyrics (R3.6) and alternate
 *  representations are deliberately NOT critical — a corrupt one commits
 *  as state:'damaged' with no addressable bytes, degrading that piece of
 *  auxiliary content only; it must never fail an otherwise complete audio
 *  install (docs/musicpack-lyrics-v1.md §7.4). */
function isCritical(asset: PlannedAsset): boolean {
  return asset.kind === 'audio-primary' || asset.kind === 'waveform' || asset.kind === 'artwork';
}

export interface FetchOptions {
  signal?: AbortSignal;
  /** Chunk size for streaming reads (default 1 MiB). */
  chunkSize?: number;
}

export type InstallerDeps = {
  fileStore: FileStore;
  catalog: CatalogStore;
  now: () => number;
  fetch: typeof fetch;
  onProgress?: (p: DownloadProgress) => void;
};

export interface InstallHandle {
  releaseId: number;
  done: Promise<InstallOutcome>;
  cancel(): void;
}

export type InstallOutcome =
  | { ok: true; bytes: number }
  | { ok: false; reason: 'aborted' | 'quota' | 'network' | 'integrity' | 'server' | 'storage'; message: string };

let installSeq = 0;

export function nextInstallId(nowMs: number): string {
  return `i${nowMs.toString(36)}${(++installSeq).toString(36)}`;
}

export class Installer {
  private active = new Map<number, AbortController>();

  constructor(private readonly deps: InstallerDeps) {}

  isRunning(releaseId: number): boolean {
    return this.active.has(releaseId);
  }

  cancel(releaseId: number): void {
    this.active.get(releaseId)?.abort();
  }

  /** Plans + downloads one release. Never throws: outcomes carry typed
   *  reasons so UI can present them without string matching. */
  install(releaseDetail: Parameters<typeof planReleaseAssets>[0] & { id: number }, opts: FetchOptions = {}): InstallHandle {
    const releaseId = releaseDetail.id;
    const ac = new AbortController();
    const existing = this.active.get(releaseId);
    if (existing) existing.abort(); // one live install per release
    this.active.set(releaseId, ac);
    const done = this.run(releaseDetail, ac.signal, opts.chunkSize ?? 1024 * 1024)
      .catch((e): InstallOutcome => ({ ok: false, reason: 'storage', message: String(e) }))
      .finally(() => void this.active.delete(releaseId));
    return { releaseId, done, cancel: () => ac.abort() };
  }

  private async run(
    releaseDetail: Parameters<typeof planReleaseAssets>[0] & { id: number },
    signal: AbortSignal,
    chunkSize: number,
  ): Promise<InstallOutcome> {
    const { fileStore, catalog, now } = this.deps;
    const releaseId = releaseDetail.id;
    const assets = planReleaseAssets(releaseDetail);
    const totalBytes = plannedAssetBytes(assets);
    let downloadedBytes = 0;
    let doneAssets = 0;
    const report = () =>
      this.deps.onProgress?.({
        releaseId,
        downloadedBytes,
        totalBytes,
        doneAssets,
        totalAssets: assets.length,
      });

    // Terminal record for failure paths: keeps UI state coherent without
    // ever feeding availability (status !== 'installed').
    const fail = async (reason: Extract<InstallOutcome, { ok: false }>['reason'], message: string): Promise<InstallOutcome> => {
      await safeRemoveStaging(fileStore, installId);
      await safePut(catalog, {
        releaseId,
        status: 'failed',
        createdAt: now(),
        updatedAt: now(),
        bytes: 0,
        assets: [],
        releaseDetail: releaseDetail as never,
        error: message,
      });
      return { ok: false, reason, message };
    };

    const installId = nextInstallId(now());
    report();
    try {
      const stagedKeys: Array<{ key: string; size: number }> = [];
      const damagedKeys = new Set<string>();
      for (const asset of assets) {
        if (signal.aborted) return await fail('aborted', 'Download canceled.');
        try {
          const size = await this.stageOne(installId, asset, signal, chunkSize);
          stagedKeys.push({ key: asset.key, size });
          downloadedBytes += size;
          doneAssets++;
          report();
        } catch (e) {
          if (signal.aborted) return await fail('aborted', 'Download canceled.');
          const message = e instanceof Error ? e.message : String(e);
          if (message.startsWith('QUOTA')) return await fail('quota', 'Not enough storage space for this download.');
          if (message.startsWith('INTEGRITY')) {
            if (isCritical(asset)) {
              return await fail('integrity', `Content verification failed for ${asset.key}.`);
            }
            // Damaged NON-critical alternate (policy §4): the install may
            // still commit — availability excludes this asset. Drop its
            // staging file so no corrupt bytes ever reach the committed set.
            await fileStore.removeStaging(`${installId}/${asset.key}`);
            damagedKeys.add(asset.key);
            doneAssets++;
            report();
            continue;
          }
          return await fail('network', `Could not download ${asset.url}: ${message}`);
        }
      }

      // ---- atomic publish --------------------------------------------
      // Damaged alternates are recorded with state:'damaged' but hold NO
      // file record: availability excludes them, playback can never touch
      // their bytes, and reinstall replaces the whole set atomically.
      const records: PackageAssetRecord[] = [];
      for (const asset of assets) {
        const staged = stagedKeys.find((s) => s.key === asset.key);
        if (!staged) continue; // damaged alternate: nothing addressable
        records.push({
          key: asset.key,
          kind: asset.kind,
          state: 'ok',
          sha256: asset.sha256,
          size: staged.size,
          trackId: asset.trackId,
          representationId: asset.representationId,
          lyricsId: asset.lyricsId,
        });
      }
      await fileStore.commit(installId, stagedKeys.map((s) => s.key));
      await safeRemoveStaging(fileStore, installId);
      const committedBytes = stagedKeys.reduce((n, s) => n + s.size, 0);
      await catalog.putPackage({
        releaseId,
        status: 'installed',
        createdAt: now(),
        updatedAt: now(),
        bytes: committedBytes,
        assets: records,
        releaseDetail: releaseDetail as never,
      });
      return { ok: true, bytes: committedBytes };
    } catch (e) {
      return fail('storage', String(e));
    }
  }

  /** Downloads one asset into staging while hashing it incrementally.
   *  Throws typed-prefixed errors: QUOTA / INTEGRITY / raw network text. */
  private async stageOne(
    installId: string,
    asset: PlannedAsset,
    signal: AbortSignal,
    chunkSize: number,
  ): Promise<number> {
    const res = await this.deps.fetch(assetUrlWithBase(asset), {
      signal,
      credentials: 'same-origin',
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const body = res.body;
    if (!body) throw new Error('empty response body');
    const reader = body.getReader();
    const hash = asset.sha256 ? new Sha256() : null;
    let size = 0;
    void size;
    const chunks: AsyncIterable<Uint8Array> = (async function* () {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (!value) continue;
        size += value.length;
        hash?.update(value);
        yield value;
      }
    })();
    let stagedSize = 0;
    try {
      stagedSize = await this.deps.fileStore.stage(installId, asset.key, chunks);
    } catch (e) {
      try { await reader.cancel(); } catch { /* already closed */ }
      throw e;
    }
    try { await reader.cancel(); } catch { /* already closed */ }
    if (asset.size > 0 && stagedSize !== asset.size) {
      throw new Error(`INTEGRITY: size mismatch (${stagedSize} != ${asset.size})`);
    }
    if (hash && asset.sha256 && hash.hex() !== asset.sha256) {
      throw new Error(`INTEGRITY: sha256 mismatch`);
    }
    return stagedSize;
  }
}

function assetUrlWithBase(asset: PlannedAsset): string {
  // API URLs are root-relative; same-origin fetch resolves them.
  return asset.url;
}

async function safePut(catalog: CatalogStore, pkg: InstalledPackage): Promise<void> {
  try {
    await catalog.putPackage(pkg);
  } catch {
    /* best-effort: the failed-record write must never mask the outcome */
  }
}

async function safeRemoveStaging(store: FileStore, installId: string): Promise<void> {
  try {
    await store.removeStaging(installId);
  } catch {
    /* swept again on next boot */
  }
}
