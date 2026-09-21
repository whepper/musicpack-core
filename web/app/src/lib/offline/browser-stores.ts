// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Production storage backends (plan §3/§4): Origin-Private FileSystem for
// audio bytes, IndexedDB for the catalog. DOM-dependent by design — every
// consumer goes through the injectable FileStore/CatalogStore seams, so
// unit tests keep running on Node with memory fakes.

import type { InstalledPackage } from './types';
import type { CatalogStore, FileStore } from './stores';

const ROOT = 'musicpack-offline-v1';
const STAGING = '.staging';
const RELEASES = 'releases';
const DB_NAME = 'musicpack-offline';
const DB_VERSION = 1;
const PACKAGES = 'packages';

function opfsAvailable(): boolean {
  return typeof navigator !== 'undefined' && !!navigator.storage?.getDirectory;
}

async function root(): Promise<FileSystemDirectoryHandle> {
  const base = await navigator.storage.getDirectory();
  return base.getDirectoryHandle(ROOT, { create: true });
}

async function subdir(dir: FileSystemDirectoryHandle, name: string): Promise<FileSystemDirectoryHandle> {
  return dir.getDirectoryHandle(name, { create: true });
}

/** OPFS-backed byte store. Staged files live under
 *  `<root>/.staging/<installId>/<key>`; committed files under
 *  `<root>/releases/<key>` (keys are namespaced by the planner, e.g.
 *  "t.101.primary"). Writes use createWritable; readers that need
 *  synchronous access (the wasm demand reader) use sync access handles in
 *  their own worker. */
export function opfsFileStore(): FileStore {
  if (!opfsAvailable()) throw new Error('OPFS unavailable');
  let cachedRoot: Promise<FileSystemDirectoryHandle> | null = null;
  const r = (): Promise<FileSystemDirectoryHandle> =>
    (cachedRoot ??= root());

  const stagingDir = async (installId: string, create: boolean) => {
    const stg = await r().then((d) => d.getDirectoryHandle(STAGING, { create: true }));
    try {
      return await stg.getDirectoryHandle(installId, { create });
    } catch {
      // create=false and missing
      return null as unknown as FileSystemDirectoryHandle;
    }
  };

  async function writeChunked(handle: FileSystemFileHandle, chunks: AsyncIterable<Uint8Array> | Iterable<Uint8Array>): Promise<number> {
    let n = 0;
    const writable = await handle.createWritable();
    try {
      for await (const c of chunks) {
        // Copy into a plain ArrayBuffer view: TS's FileSystemWriteChunkType
        // requires ArrayBuffer-backed views (chunks may be length-tracked
        // subarray views over larger buffers).
        await writable.write(new Uint8Array(c).buffer as ArrayBuffer);
        n += c.length;
      }
      await writable.close();
    } catch (e) {
      // Abort frees the temp file on failure (quota/integrity/abort paths).
      const w = writable as FileSystemWritableFileStream & { abort?(): Promise<void> };
      if (w.abort) { try { await w.abort(); } catch { /* already closed */ } }
      throw e;
    }
    return n;
  }

  async function stagedFileHandle(installId: string, key: string): Promise<FileSystemFileHandle> {
    const slash = key.indexOf('/');
    const parent = slash >= 0 ? await subdir(await stagingDir(installId, true), key.slice(0, slash)) : await stagingDir(installId, true);
    return parent.getFileHandle(slash >= 0 ? key.slice(slash + 1) : key, { create: true });
  }

  return {
    async stage(installId, key, chunks) {
      const handle = await stagedFileHandle(installId, key);
      return writeChunked(handle, chunks);
    },

    async commit(installId, keys) {
      const releases = await subdir(await r(), RELEASES);
      for (const key of keys) {
        const src = await stagedFileHandle(installId, key);
        const dst = await releases.getFileHandle(key, { create: true });
        const file = await (await src.getFile()).arrayBuffer();
        const writable = await dst.createWritable();
        await writable.write(file);
        await writable.close();
      }
    },

    async read(key) {
      try {
        const releases = await subdir(await r(), RELEASES);
        const fh = await releases.getFileHandle(key, { create: false });
        const file = await fh.getFile();
        return new Uint8Array(await file.arrayBuffer());
      } catch {
        return null;
      }
    },

    readStream(key): AsyncIterable<Uint8Array> | null {
      // Synchronous probe is impossible on OPFS; callers use has() via
      // sizeOf before streaming. We return a lazy stream that throws if
      // the file vanished mid-read (auditable damage).
      return {
        async *[Symbol.asyncIterator]() {
          const releases = await subdir(await r(), RELEASES);
          const fh = await releases.getFileHandle(key, { create: false });
          const file = await fh.getFile();
          yield new Uint8Array(await file.arrayBuffer());
        },
      };
    },

    async sizeOf(key) {
      try {
        const releases = await subdir(await r(), RELEASES);
        const fh = await releases.getFileHandle(key, { create: false });
        return (await fh.getFile()).size;
      } catch {
        return null;
      }
    },

    async remove(key) {
      try {
        const releases = await subdir(await r(), RELEASES);
        await releases.removeEntry(key);
      } catch {
        /* already gone */
      }
    },

    async removeStaging(scope) {
      try {
        const stg = await r().then((d) => d.getDirectoryHandle(STAGING, { create: false }));
        const slash = scope.indexOf('/');
        if (slash < 0) await stg.removeEntry(scope, { recursive: true });
        else await stg.removeEntry(scope, { recursive: true });
      } catch {
        /* nothing to sweep */
      }
    },

    async listStaging() {
      try {
        const stg = await r().then((d) => d.getDirectoryHandle(STAGING, { create: false }));
        const out: string[] = [];
        for await (const name of entriesOf(stg)) out.push(name);
        return out;
      } catch {
        return [];
      }
    },

    async listFiles() {
      try {
        const releases = await subdir(await r(), RELEASES);
        const out: string[] = [];
        for await (const name of entriesOf(releases)) out.push(name);
        return out;
      } catch {
        return [];
      }
    },
  };
}

async function* entriesOf(dir: FileSystemDirectoryHandle): AsyncGenerator<string> {
  // TypeScript's lib dom does not model AsyncIterator on the handle yet.
  const it = (dir as unknown as { entries(): AsyncIterableIterator<[string, unknown]> }).entries();
  for await (const [name] of it) yield name;
}

/** IndexedDB-backed package catalog. One record per release; putPackage()
 *  is a single-object transaction = the atomic publish point. */
export function idbCatalog(): CatalogStore {
  if (typeof indexedDB === 'undefined') throw new Error('IndexedDB unavailable');

  function open(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(DB_NAME, DB_VERSION);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(PACKAGES)) db.createObjectStore(PACKAGES, { keyPath: 'releaseId' });
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error ?? new Error('idb open failed'));
    });
  }

  async function tx<T>(mode: IDBTransactionMode, fn: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
    const db = await open();
    try {
      return await new Promise<T>((resolve, reject) => {
        const t = db.transaction(PACKAGES, mode);
        const req = fn(t.objectStore(PACKAGES));
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error ?? new Error('idb request failed'));
      });
    } finally {
      db.close();
    }
  }

  return {
    async getPackage(releaseId) {
      return (await tx('readonly', (s) => s.get(releaseId))) ?? null;
    },
    async allPackages() {
      return (await tx('readonly', (s) => s.getAll())) as InstalledPackage[];
    },
    async putPackage(pkg) {
      await tx('readwrite', (s) => s.put(pkg));
    },
    async deletePackage(releaseId) {
      await tx('readwrite', (s) => s.delete(releaseId));
    },
  };
}
