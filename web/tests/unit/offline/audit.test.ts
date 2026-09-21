// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Boot audit tests (plan §9): browser eviction / corruption detection and
// the availability-safe reconciliation.

import { describe, expect, it } from 'vitest';
import { applyAudit, auditPackage, type AuditResult } from '../../../app/src/lib/offline/audit';
import { memoryFileStore } from '../../../app/src/lib/offline/stores';
import type { InstalledPackage } from '../../../app/src/lib/offline/types';

function pkgFixture(): InstalledPackage {
  return {
    releaseId: 7,
    status: 'installed',
    createdAt: 1,
    updatedAt: 1,
    bytes: 13,
    assets: [
      { key: 't.101.primary', kind: 'audio-primary', state: 'ok', trackId: 101, size: 3 },
      { key: 't.102.primary', kind: 'audio-primary', state: 'ok', trackId: 102, size: 5 },
      { key: 'art.301', kind: 'artwork', state: 'ok', artworkId: 301, size: 0 },
    ],
    releaseDetail: {} as InstalledPackage['releaseDetail'],
  };
}

describe('offline boot audit', () => {
  it('reports ok when every committed file matches its size', async () => {
    const files = memoryFileStore();
    files.files.set('t.101.primary', new Uint8Array(3));
    files.files.set('t.102.primary', new Uint8Array(5));
    // artwork has declared size 0 (undeclared): audit must not flag it.
    files.files.set('art.301', new Uint8Array(0));
    const pkg = pkgFixture();
    const result = await auditPackage(pkg, files);
    expect(result.verdict).toBe('ok');
    expect(result.damagedKeys).toEqual([]);
  });

  it('flags vanished files as damaged (browser eviction)', async () => {
    const files = memoryFileStore();
    files.files.set('t.101.primary', new Uint8Array(3));
    files.files.set('art.301', new Uint8Array(0)); // artwork present, size undeclared
    // t.102 evicted
    const result = await auditPackage(pkgFixture(), files);
    expect(result.verdict).toBe('damaged');
    expect(result.damagedKeys).toEqual(['t.102.primary']);
  });

  it('flags size-mismatched files as damaged (truncation)', async () => {
    const files = memoryFileStore();
    files.files.set('t.101.primary', new Uint8Array(3));
    files.files.set('t.102.primary', new Uint8Array(4)); // ≠ 5
    files.files.set('art.301', new Uint8Array(0));
    const result = await auditPackage(pkgFixture(), files);
    expect(result.damagedKeys).toEqual(['t.102.primary']);
  });

  it('applyAudit removes damaged records so availability cannot serve ghosts', () => {
    const pkg = pkgFixture();
    const result: AuditResult = {
      releaseId: 7,
      damagedKeys: ['t.102.primary'],
      verdict: 'damaged',
    };
    const updated = applyAudit(pkg, result, 99);
    expect(updated.assets.map((a) => a.key)).toEqual(['t.101.primary', 'art.301']);
    expect(updated.stale).toBe(true);
    expect(updated.updatedAt).toBe(99);
  });

  it('applyAudit is a no-op for ok verdicts', () => {
    const pkg = pkgFixture();
    const result: AuditResult = { releaseId: 7, damagedKeys: [], verdict: 'ok' };
    expect(applyAudit(pkg, result, 99)).toBe(pkg);
  });
});
