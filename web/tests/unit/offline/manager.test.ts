// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Manager lifecycle tests (plan §9, decision D2): stale detection flags
// only; removal is explicit; failed installs keep UI state coherent.

import { describe, expect, it } from 'vitest';
import { createOfflineManager } from '../../../app/src/lib/offline/manager';
import { memoryCatalog, memoryFileStore } from '../../../app/src/lib/offline/stores';
import type { ReleaseDetail } from '../../../app/src/lib/api/types';

const SHA_ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';

function releaseFixture(audioSha256: string): ReleaseDetail {
  return {
    id: 7,
    media: [
      {
        disc: 1,
        tracks: [
          {
            id: 101,
            audio: { url: '/api/v1/tracks/101/audio', size: 3, sha256: audioSha256 },
          },
        ],
      },
    ],
    artwork: [],
  } as unknown as ReleaseDetail;
}

/** The same release plus one track-linked lyric document (R3.6). */
function lyricsReleaseFixture(lyricSha256: string): ReleaseDetail {
  return {
    ...releaseFixture(SHA_ABC),
    trackLyrics: [
      { trackId: 101, id: 501, url: '/api/v1/assets/501', size: 3, sha256: lyricSha256, lang: 'en' },
    ],
  } as unknown as ReleaseDetail;
}

function chunkedResponse(bytes: Uint8Array): Response {
  let off = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (off >= bytes.length) {
        controller.close();
        return;
      }
      controller.enqueue(bytes.subarray(off, Math.min(off + 2, bytes.length)));
      off += 2;
    },
  });
  return new Response(stream, { status: 200 });
}

const fixtureFetch = ((url: string) => {
  const enc = new TextEncoder();
  const sources: Record<string, Uint8Array> = {
    '/api/v1/tracks/101/audio': enc.encode('abc'),
    '/api/v1/assets/501': enc.encode('abc'),
  };
  const src = sources[url];
  if (!src) return Promise.reject(new Error('no fixture ' + url));
  return Promise.resolve(chunkedResponse(src));
}) as unknown as typeof fetch;

function makeManager(stores?: { fileStore?: ReturnType<typeof memoryFileStore>; catalog?: ReturnType<typeof memoryCatalog> }) {
  const fileStore = stores?.fileStore ?? memoryFileStore();
  const catalog = stores?.catalog ?? memoryCatalog();
  const manager = createOfflineManager({
    fileStore,
    catalog,
    fetch: fixtureFetch,
  });
  return { manager, fileStore, catalog };
}

describe('offline manager (D2 lifecycle)', () => {
  it('install → installed state + availability populated', async () => {
    const { manager } = makeManager();
    await manager.install(releaseFixture(SHA_ABC));
    // install is fire-and-forget; wait for the committed record
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    expect(await manager.packageFor(7)).not.toBeNull();
    expect(manager.availability.localKeyFor(101, { kind: 'primary' })).toBe('t.101.primary');
    // states is a Svelte-style readable store: read via subscribe.
    let uiState: string | undefined;
    manager.states.subscribe((m) => (uiState = m.get(7)?.state))();
    expect(uiState).toBe('installed');
  });

  it('checkForUpdate flags stale on hash change but does NOT replace content', async () => {
    const { manager, fileStore } = makeManager();
    await manager.install(releaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    // Server now serves different bytes under the same URL.
    const changed = releaseFixture('f'.repeat(64));
    const stale = await manager.checkForUpdate(changed);
    expect(stale).toBe(true);
    const pkg = (await manager.packageFor(7))!;
    expect(pkg.stale).toBe(true);
    // D2: local-first still serves the ORIGINAL verified bytes.
    expect(fileStore.files.has('t.101.primary')).toBe(true);
    const bytes = await fileStore.read('t.101.primary');
    expect(new TextDecoder().decode(bytes!)).toBe('abc');
  });

  it('checkForUpdate covers track lyrics through the release index (R3.6)', async () => {
    const { manager, fileStore } = makeManager();
    await manager.install(lyricsReleaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    // The staged lyric committed with the authoritative hash.
    expect(fileStore.files.has('t.101.lyr.501')).toBe(true);
    expect((await manager.packageFor(7))!.assets.some((a) => a.key === 't.101.lyr.501')).toBe(true);
    // Unchanged release: not stale.
    expect(await manager.checkForUpdate(lyricsReleaseFixture(SHA_ABC))).toBe(false);
    // A lyric replaced upstream (same key, new hash) flags the package —
    // without replacing the local copy (D2).
    expect(await manager.checkForUpdate(lyricsReleaseFixture('9'.repeat(64)))).toBe(true);
    expect(new TextDecoder().decode((await fileStore.read('t.101.lyr.501'))!)).toBe('abc');
  });

  it('the committed snapshot preserves the release-level lyric index with language (R3.6)', async () => {
    const { manager } = makeManager();
    await manager.install(lyricsReleaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    const pkg = (await manager.packageFor(7))!;
    expect(pkg.releaseDetail.trackLyrics).toEqual([
      { trackId: 101, id: 501, url: '/api/v1/assets/501', size: 3, sha256: SHA_ABC, lang: 'en' },
    ]);
  });

  it('remove deletes files and the record; availability forgets', async () => {    const { manager, fileStore } = makeManager();
    await manager.install(releaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    await manager.remove(7);
    expect(await manager.packageFor(7)).toBeNull();
    expect(await fileStore.sizeOf('t.101.primary')).toBeNull();
    expect(manager.availability.localKeyFor(101, { kind: 'primary' })).toBeNull();
    expect(manager.availability.hasInstalled()).toBe(false);
  });

  it('boot audit damage surfaces as the damaged UI state and reinstall heals it', async () => {
    const { manager, fileStore, catalog } = makeManager();
    await manager.install(releaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    // Evict a committed primary behind the catalog's back (browser eviction).
    await fileStore.remove('t.101.primary');

    // A fresh manager over the SAME catalog+fileStore boots through init():
    // the audit runs, the record loses its asset and the UI state must read
    // 'damaged', not 'stale'. It carries the same fetch fixture so the
    // reinstall below can succeed (this mirrors one browser session's
    // reload, where only the page — not the storage — is new).
    const { manager: booted } = makeManager({ fileStore, catalog });
    await booted.init();
    let uiState: string | undefined;
    booted.states.subscribe((m) => (uiState = m.get(7)?.state))();
    expect(uiState).toBe('damaged');
    expect(await booted.packageFor(7)).not.toBeNull();

    // Reinstall (same bytes served again) clears the damaged presentation.
    await booted.install(releaseFixture(SHA_ABC));
    for (let i = 0; i < 50; i++) {
      let s: string | undefined;
      booted.states.subscribe((m) => (s = m.get(7)?.state))();
      if (s === 'installed') break;
      await new Promise((r) => setTimeout(r, 10));
    }
    booted.states.subscribe((m) => (uiState = m.get(7)?.state))();
    expect(uiState).toBe('installed');
  });

  it('listPackages exposes committed records for the storage panel', async () => {
    const { manager } = makeManager();
    expect(await manager.listPackages()).toEqual([]);
    await manager.install(releaseFixture(SHA_ABC));
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    const pkgs = await manager.listPackages();
    expect(pkgs.map((p) => p.releaseId)).toContain(7);
    expect(pkgs.find((p) => p.releaseId === 7)?.bytes).toBeGreaterThan(0);
  });

  it('installedAlbumIds maps installed releases to album ids', async () => {
    const { manager } = makeManager();
    expect(manager.installedAlbumIds().size).toBe(0);
    const rel = releaseFixture(SHA_ABC);
    (rel as unknown as { album: { id: number } }).album = { id: 42 };
    await manager.install(rel);
    for (let i = 0; i < 50 && !(await manager.packageFor(7)); i++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    expect(manager.installedAlbumIds()).toEqual(new Set([42]));
  });

  it('disabled manager (no OPFS/IDB) is inert and safe', async () => {
    const manager = createOfflineManager({
      catalog: memoryCatalog(),
      // no fileStore => defaultStores skipped only when both given;
      // emulate unsupported by omitting fetch-capable stores:
    });
    void manager;
    // When neither store is provided and feature detection fails in Node,
    // enabled must be false — construct via the real path to assert.
    const nodeManager = createOfflineManager({});
    expect(nodeManager.enabled).toBe(false);
    await nodeManager.init(); // must not throw
    await nodeManager.remove(7); // must not throw
  });
});
