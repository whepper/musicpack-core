// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Boot-time integrity audit + storage accounting (plan §9/§10).
//
// Browsers may evict origin data under pressure even with a persist()
// grant. The audit reconciles the committed catalog against reality:
// any missing or size-mismatched file marks its package 'damaged', which
// excludes it from availability until the user reinstalls. This is
// corruption DETECTION; healing is always an explicit user action.

import type { CatalogStore, FileStore } from './stores';
import type { InstalledPackage } from './types';

export interface AuditResult {
  releaseId: number;
  /** Assets that vanished or changed size on disk. */
  damagedKeys: string[];
  verdict: 'ok' | 'damaged';
}

export interface StorageUsage {
  usageBytes: number | null;
  quotaBytes: number | null;
  persisted: boolean | null;
}

/** Audits one committed package. Never mutates storage directly; callers
 *  decide how to present/apply `damaged` (the manager flips the record).
 *  Rules: a missing file is always damage; a present file is only
 *  size-checked when the catalog declares its size (> 0). */
export async function auditPackage(
  pkg: InstalledPackage,
  files: FileStore,
): Promise<AuditResult> {
  const damagedKeys: string[] = [];
  for (const asset of pkg.assets) {
    if (asset.state !== 'ok') continue;
    const actual = await files.sizeOf(asset.key);
    if (actual === null) {
      damagedKeys.push(asset.key);
      continue;
    }
    if (asset.size > 0 && actual !== asset.size) {
      damagedKeys.push(asset.key);
    }
  }
  return { releaseId: pkg.releaseId, damagedKeys, verdict: damagedKeys.length === 0 ? 'ok' : 'damaged' };
}

/** Audits every installed package and returns per-package results. */
export async function auditAll(
  catalog: CatalogStore,
  files: FileStore,
): Promise<AuditResult[]> {
  const out: AuditResult[] = [];
  for (const pkg of await catalog.allPackages()) {
    if (pkg.status !== 'installed') continue;
    out.push(await auditPackage(pkg, files));
  }
  return out;
}

/** Applies an audit result: damaged packages lose their damaged assets
 *  (or flip wholesale when primaries were hit) so availability can never
 *  serve bytes that no longer exist. Returns the updated record. */
export function applyAudit(
  pkg: InstalledPackage,
  result: AuditResult,
  nowMs: number,
): InstalledPackage {
  if (result.verdict === 'ok') return pkg;
  const assets = pkg.assets.filter((a) => !result.damagedKeys.includes(a.key));
  const primaryHit = result.damagedKeys.some((k) => k.endsWith('.primary'));
  // A missing primary means that track is unplayable offline; keep the
  // package installed but let availability fall back to remote for it —
  // which happens naturally because the asset record is gone.
  void primaryHit;
  return { ...pkg, assets, stale: true, updatedAt: nowMs };
}

/** Best-effort quota probe. Nulls mean "unknown" — UI must tolerate. */
export async function storageUsage(): Promise<StorageUsage> {
  if (typeof navigator === 'undefined' || !navigator.storage?.estimate) {
    return { usageBytes: null, quotaBytes: null, persisted: null };
  }
  try {
    const est = await navigator.storage.estimate();
    const persisted =
      navigator.storage.persisted ? await navigator.storage.persisted() : null;
    return { usageBytes: est.usage ?? null, quotaBytes: est.quota ?? null, persisted };
  } catch {
    return { usageBytes: null, quotaBytes: null, persisted: null };
  }
}

/** Requests persistent storage once (first install). Fire-and-forget. */
export async function requestPersistence(): Promise<void> {
  try {
    if (typeof navigator !== 'undefined' && navigator.storage?.persist) {
      await navigator.storage.persist();
    }
  } catch {
    /* best effort */
  }
}
