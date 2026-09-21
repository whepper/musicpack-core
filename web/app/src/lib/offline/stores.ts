// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Byte-store seam for the offline downloader (plan §4).
//
// Two orthogonal capabilities:
//  - FileStore: named binary blobs (OPFS in production; Map fake in tests)
//    organized under a per-install staging root so an aborted download is
//    one subtree deletion away from clean.
//  - CatalogStore: the package records (IndexedDB in production). The
//    COMMIT of one record is the atomic publish point required by the
//    Installed-Package Usability Invariant.

import type {
  InstalledPackage,
} from './types';

export interface StagedFile {
  key: string;
  size: number;
}

export interface FileStore {
  /** Streams `chunks` into a fresh staging file, returning its size. */
  stage(installId: string, key: string, chunks: AsyncIterable<Uint8Array> | Iterable<Uint8Array>): Promise<number>;
  /** Moves every staged file of installId into the committed namespace.
   *  Files are copied per-key; missing keys throw. */
  commit(installId: string, keys: string[]): Promise<void>;
  read(key: string): Promise<Uint8Array | null>;
  /** A readable stream over a committed file, or null when absent.
   *  Used by playback paths that want incremental reads. */
  readStream(key: string): AsyncIterable<Uint8Array> | null;
  sizeOf(key: string): Promise<number | null>;
  remove(key: string): Promise<void>;
  /** Removes one staged file ("installId" or "installId/key"). */
  removeStaging(scope: string): Promise<void>;
  /** Staging subtrees currently on disk (for orphan sweeps). */
  listStaging(): Promise<string[]>;
  listFiles(): Promise<string[]>;
}

export interface CatalogStore {
  getPackage(releaseId: number): Promise<InstalledPackage | null>;
  allPackages(): Promise<InstalledPackage[]>;
  /** The atomic publish/replace point: writes the full record in one
   *  transaction. Callers must only invoke this with a fully verified
   *  asset set (or an explicit terminal 'failed' record). */
  putPackage(pkg: InstalledPackage): Promise<void>;
  deletePackage(releaseId: number): Promise<void>;
}

/** In-memory fakes (unit tests, SSR guards). */
export function memoryFileStore(): FileStore & {
  staged: Map<string, Uint8Array>;
  files: Map<string, Uint8Array>;
} {
  const staged = new Map<string, Uint8Array>();
  const files = new Map<string, Uint8Array>();
  return {
    staged,
    files,
    async stage(installId, key, chunks) {
      const parts: Uint8Array[] = [];
      let n = 0;
      for await (const c of chunks) {
        parts.push(c);
        n += c.length;
      }
      const buf = new Uint8Array(n);
      let off = 0;
      for (const p of parts) {
        buf.set(p, off);
        off += p.length;
      }
      staged.set(`${installId}/${key}`, buf);
      return n;
    },
    async commit(installId, keys) {
      for (const key of keys) {
        const buf = staged.get(`${installId}/${key}`);
        if (!buf) throw new Error(`staging missing file: ${key}`);
        files.set(key, buf);
        staged.delete(`${installId}/${key}`);
      }
    },
    async read(key) {
      return files.get(key) ?? null;
    },
    readStream(key) {
      const buf = files.get(key);
      if (!buf) return null;
      return (async function* () {
        yield buf;
      })();
    },
    async sizeOf(key) {
      return files.get(key)?.length ?? null;
    },
    async remove(key) {
      files.delete(key);
    },
    async removeStaging(scope: string) {
      // Whole subtree ("installId") or one staged file ("installId/key").
      for (const k of [...staged.keys()]) {
        if (k === scope || k.startsWith(`${scope}/`)) staged.delete(k);
      }
    },
    async listStaging() {
      return [...new Set([...staged.keys()].map((k) => k.split('/')[0]!))];
    },
    async listFiles() {
      return [...files.keys()];
    },
  };
}

export function memoryCatalog(): CatalogStore & { records: Map<number, InstalledPackage> } {
  const records = new Map<number, InstalledPackage>();
  return {
    records,
    async getPackage(releaseId) {
      return records.get(releaseId) ?? null;
    },
    async allPackages() {
      return [...records.values()];
    },
    async putPackage(pkg) {
      records.set(pkg.releaseId, structuredClone(pkg));
    },
    async deletePackage(releaseId) {
      records.delete(releaseId);
    },
  };
}
