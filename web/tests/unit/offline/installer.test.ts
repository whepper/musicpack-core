// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Installer behavior tests (plan §4): atomic commit, integrity, quota,
// abort, and the damaged-alternate policy. All storage is in-memory fakes;
// fetch is a stubbed ReadableStream source.

import { describe, expect, it, vi } from 'vitest';
import { Installer, type InstallOutcome } from '../../../app/src/lib/offline/installer';
import { memoryCatalog, memoryFileStore } from '../../../app/src/lib/offline/stores';
import { plannedAssetBytes } from '../../../app/src/lib/offline/types';

const SHA_ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
const SHA_12345 = '5994471abb01112afcc18159f6cc74b4f511b99806da59b3caf5a9c173cacfc5';
const SHA_WXYZ = '17f488f768db8fbe7a408a9469203c61e03b5fe43214b95a00e7c0c52d2fd933';

function releaseFixture(overrides: Record<string, unknown> = {}) {
  return {
    id: 7,
    media: [
      {
        disc: 1,
        tracks: [
          {
            id: 101,
            audio: { url: '/api/v1/tracks/101/audio', size: 3, sha256: SHA_ABC },
          },
          {
            id: 102,
            audio: { url: '/api/v1/tracks/102/audio', size: 5, sha256: SHA_12345 },
            representations: [
              {
                id: 201,
                url: '/api/v1/tracks/102/representations/201/audio',
                size: 4,
                sha256: SHA_WXYZ,
              },
            ],
          },
        ],
      },
    ],
    artwork: [],
    ...overrides,
  };
}

function chunkedResponse(bytes: Uint8Array, chunk = 2): Response {
  let off = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (off >= bytes.length) {
        controller.close();
        return;
      }
      controller.enqueue(bytes.subarray(off, Math.min(off + chunk, bytes.length)));
      off += chunk;
    },
  });
  return new Response(stream, { status: 200 });
}

/** Fetch stub keyed by URL; values are bytes or an Error factory. */
function fetchStub(sources: Record<string, Uint8Array | (() => Error)>) {
  return vi.fn().mockImplementation((url: string, init?: RequestInit) => {
    const src = sources[url];
    if (src === undefined) {
      return Promise.reject(new Error(`no fixture for ${url}`));
    }
    if (typeof src === 'function') return Promise.reject(src());
    void init;
    return Promise.resolve(chunkedResponse(src));
  });
}

function makeInstaller(fetchImpl: ReturnType<typeof fetchStub>, onProgress?: (p: { downloadedBytes: number; totalAssets: number; doneAssets: number }) => void) {
  const fileStore = memoryFileStore();
  const catalog = memoryCatalog();
  let tick = 0;
  const installer = new Installer({
    fileStore,
    catalog,
    now: () => ++tick,
    fetch: fetchImpl as unknown as typeof fetch,
    onProgress: onProgress as never,
  });
  return { installer, fileStore, catalog };
}

function outcomeOf(p: Promise<InstallOutcome>): Promise<InstallOutcome> {
  return p;
}

/** A release with one track-linked lyric document (R3.6). */
function lyricsRelease(sha256: string = SHA_ABC, size = 3) {
  return releaseFixture({
    trackLyrics: [
      { trackId: 101, id: 501, url: '/api/v1/assets/501', size, sha256, lang: 'en' },
    ],
  });
}

/** The audio bytes of [`releaseFixture`], keyed by URL. */
function audioBytes() {
  const enc = new TextEncoder();
  return {
    '/api/v1/tracks/101/audio': enc.encode('abc'),
    '/api/v1/tracks/102/audio': enc.encode('12345'),
    '/api/v1/tracks/102/representations/201/audio': enc.encode('wxyz'),
  } as Record<string, Uint8Array>;
}

describe('offline installer — track lyrics (R3.6)', () => {
  it('stages a lyric under its identity key and commits verified bytes', async () => {
    const enc = new TextEncoder();
    const fetchImpl = fetchStub({ ...audioBytes(), '/api/v1/assets/501': enc.encode('abc') });
    const { installer, fileStore, catalog } = makeInstaller(fetchImpl);
    const outcome = await outcomeOf(installer.install(lyricsRelease()).done);
    expect(outcome.ok).toBe(true);
    // Bytes live in the committed FileStore namespace under the stable key.
    expect(fileStore.files.get('t.101.lyr.501')).toEqual(enc.encode('abc'));
    expect(fileStore.staged.size).toBe(0);
    const pkg = await catalog.getPackage(7);
    const rec = pkg!.assets.find((a) => a.key === 't.101.lyr.501')!;
    expect(rec).toMatchObject({
      kind: 'lyrics',
      state: 'ok',
      trackId: 101,
      lyricsId: 501,
      sha256: SHA_ABC,
      size: 3,
    });
  });

  it('downloads the lyric through the existing asset URL (no track-detail fetch)', async () => {
    const enc = new TextEncoder();
    const fetchImpl = fetchStub({ ...audioBytes(), '/api/v1/assets/501': enc.encode('abc') });
    const { installer } = makeInstaller(fetchImpl);
    await outcomeOf(installer.install(lyricsRelease()).done);
    const urls = fetchImpl.mock.calls.map((c) => c[0]);
    expect(urls).toContain('/api/v1/assets/501');
    // The planner's purity contract: no track-detail request is ever made.
    expect(urls.some((u) => /\/api\/v1\/tracks\/\d+$/.test(String(u)))).toBe(false);
  });

  it('a hash-mismatched lyric commits as damaged; the install still succeeds', async () => {
    const enc = new TextEncoder();
    // Declared SHA_ABC ('abc'); serve different bytes.
    const fetchImpl = fetchStub({ ...audioBytes(), '/api/v1/assets/501': enc.encode('wxyz') });
    const { installer, fileStore, catalog } = makeInstaller(fetchImpl);
    const outcome = await outcomeOf(installer.install(lyricsRelease()).done);
    expect(outcome.ok).toBe(true); // non-critical: audio install is unaffected
    // No addressable bytes: no committed file, no asset record, no staging.
    expect(fileStore.files.get('t.101.lyr.501')).toBeUndefined();
    expect(fileStore.staged.size).toBe(0);
    const pkg = await catalog.getPackage(7);
    expect(pkg!.assets.some((a) => a.key === 't.101.lyr.501')).toBe(false);
    expect(pkg!.assets.some((a) => a.kind === 'audio-primary')).toBe(true);
  });

  it('a size-mismatched lyric is integrity-damaged, not fatal', async () => {
    const enc = new TextEncoder();
    const fetchImpl = fetchStub({ ...audioBytes(), '/api/v1/assets/501': enc.encode('abc') });
    const { installer, catalog } = makeInstaller(fetchImpl);
    // Declared size lies (99) while the hash would match: size is checked.
    const outcome = await outcomeOf(installer.install(lyricsRelease(SHA_ABC, 99)).done);
    expect(outcome.ok).toBe(true);
    const pkg = await catalog.getPackage(7);
    expect(pkg!.assets.some((a) => a.key === 't.101.lyr.501')).toBe(false);
  });

  it('a lyric is never a critical asset: hash failure cannot fail the install', async () => {
    const enc = new TextEncoder();
    const fetchImpl = fetchStub({ ...audioBytes(), '/api/v1/assets/501': enc.encode('nope') });
    const { installer } = makeInstaller(fetchImpl);
    const outcome = await outcomeOf(installer.install(lyricsRelease()).done);
    expect(outcome.ok).toBe(true);
  });
});

describe('offline installer', () => {
  it('downloads, verifies, commits atomically and cleans staging', async () => {
    const { installer, fileStore, catalog } = makeInstaller(
      fetchStub({
        '/api/v1/tracks/101/audio': new TextEncoder().encode('abc'),
        '/api/v1/tracks/102/audio': new TextEncoder().encode('12345'),
        '/api/v1/tracks/102/representations/201/audio': new TextEncoder().encode('wxyz'),
      }),
    );
    const handle = installer.install(releaseFixture());
    const result = await handle.done;
    expect(result).toEqual({ ok: true, bytes: 12 });
    const pkg = await catalog.getPackage(7);
    expect(pkg?.status).toBe('installed');
    expect(pkg?.assets).toHaveLength(3);
    // committed bytes are readable under stable keys
    expect(new TextDecoder().decode((await fileStore.read('t.101.primary'))!)).toBe('abc');
    expect(await fileStore.read('t.102.r.201')).not.toBeNull();
    // no staged leftovers
    expect(await fileStore.listStaging()).toEqual([]);
  });

  it('fails the whole install when a CRITICAL asset fails integrity', async () => {
    const enc = new TextEncoder();
    const { installer, catalog } = makeInstaller(
      fetchStub({
        '/api/v1/tracks/101/audio': enc.encode('XXX'), // wrong bytes for SHA_ABC
        '/api/v1/tracks/102/audio': enc.encode('12345'),
      }),
    );
    const result = await installer.install(releaseFixture()).done;
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toBe('integrity');
    const pkg = await catalog.getPackage(7);
    expect(pkg?.status).toBe('failed');
    // nothing playable leaked into availability shape
    expect(pkg!.assets).toHaveLength(0);
  });

  it('COMMITS with a damaged alternate when only a representation is corrupt', async () => {
    const enc = new TextEncoder();
    const { installer, catalog, fileStore } = makeInstaller(
      fetchStub({
        '/api/v1/tracks/101/audio': enc.encode('abc'),
        '/api/v1/tracks/102/audio': enc.encode('12345'),
        '/api/v1/tracks/102/representations/201/audio': enc.encode('QQQQ'), // corrupt alt (≠ SHA_WXYZ)
      }),
    );
    const result = await installer.install(releaseFixture()).done;
    expect(result.ok).toBe(true);
    const pkg = (await catalog.getPackage(7))!;
    expect(pkg.status).toBe('installed');
    // The corrupt alternate is recorded as damaged and holds no file:
    // availability excludes it; playback can never reach its bytes.
    expect(pkg.assets.find((a) => a.key === 't.102.r.201')).toBeUndefined();
    expect(pkg.bytes).toBe(3 + 5); // committed audio bytes only
    // primaries are healthy and readable
    expect(pkg.assets.find((a) => a.key === 't.101.primary')?.state).toBe('ok');
    expect(await fileStore.sizeOf('t.101.primary')).toBe(3);
  });

  it('maps quota failures to reason=quota and leaves no staging', async () => {
    const enc = new TextEncoder();
    let calls = 0;
    const store = memoryFileStore();
    const originalStage = store.stage.bind(store);
    store.stage = async (...args) => {
      if (++calls > 1) throw new Error('QUOTA: exceeded');
      return originalStage(...args);
    };
    const catalog = memoryCatalog();
    let tick = 0;
    const installer = new Installer({
      fileStore: store,
      catalog,
      now: () => ++tick,
      fetch: fetchStub({
        '/api/v1/tracks/101/audio': enc.encode('abc'),
        '/api/v1/tracks/102/audio': enc.encode('12345'),
        '/api/v1/tracks/102/representations/201/audio': enc.encode('wxyz'),
      }) as unknown as typeof fetch,
    });
    const result = await installer.install(releaseFixture()).done;
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toBe('quota');
    expect((await catalog.getPackage(7))!.status).toBe('failed');
  });

  it('aborts cleanly mid-download and removes staging', async () => {
    const enc = new TextEncoder();
    const { installer, fileStore } = makeInstaller(
      fetchStub({
        '/api/v1/tracks/101/audio': enc.encode('abc'),
        '/api/v1/tracks/102/audio': () => new Error('should not be reached'),
      }),
    );
    const handle = installer.install(releaseFixture());
    handle.cancel();
    const result = await handle.done;
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toBe('aborted');
    expect(await fileStore.listStaging()).toEqual([]);
  });

  it('reports progress across assets', async () => {
    const enc = new TextEncoder();
    const seen: Array<[number, number]> = [];
    const { installer } = makeInstaller(
      fetchStub({
        '/api/v1/tracks/101/audio': enc.encode('abc'),
        '/api/v1/tracks/102/audio': enc.encode('12345'),
        '/api/v1/tracks/102/representations/201/audio': enc.encode('wxyz'),
      }),
      (p) => seen.push([p.doneAssets, p.totalAssets]),
    );
    await installer.install(releaseFixture()).done;
    expect(seen.at(-1)).toEqual([3, 3]);
  });

  it('serializes installs per release (second call supersedes the first)', async () => {
    const enc = new TextEncoder();
    const sources = {
      '/api/v1/tracks/101/audio': enc.encode('abc'),
      '/api/v1/tracks/102/audio': enc.encode('12345'),
      '/api/v1/tracks/102/representations/201/audio': enc.encode('wxyz'),
    };
    const { installer, catalog } = makeInstaller(fetchStub(sources));
    const h1 = installer.install(releaseFixture());
    const h2 = installer.install(releaseFixture()); // aborts h1
    const [r1, r2] = await Promise.all([outcomeOf(h1.done), outcomeOf(h2.done)]);
    expect(r1.ok).toBe(false);
    expect(r2.ok).toBe(true);
    expect((await catalog.getPackage(7))!.status).toBe('installed');
  });
});
