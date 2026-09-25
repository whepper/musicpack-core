// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// `.mpak` MANF contents → client queue.
//
// A container that is already available to the client is read by Rust
// (`RustPlaybackEngine.openContainer` → the worker's `containerTracks` →
// core's `MpakBackend`), which returns its MANF's tracks with canonical
// `mpak:<container>#<member>` sources. This suite covers the browser-side
// half: the discovered tracks become ordinary `QueueItem`s and enter the
// existing queue.
//
// The fixture is the committed `fixtures/reference/reference-small.mpak`, a
// real container produced by the reference packer and read back here by
// core's real reader through the generated wasm module. No container
// formatting or parsing happens in this file.
//
// PCM equivalence for the member bytes is proven natively, where core's
// container *writer* lives: see `a_manf_discovered_member_decodes_identically`
// in `crates/musicpack-wasm`, which drives the same complete path
// (MANF → canonical source → RangeSourceBackend → member bytes → decoder) and
// compares against both the frozen oracle digest and a whole-file decode.

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import {
  containerQueueItems,
  itemsForContainer,
  type ContainerReader,
} from '../../app/src/lib/mpak/container-tracks';
import { createQueueStore } from '../../app/src/lib/state/queue';
import type {
  ContainerAlbumWire,
} from '../../app/src/lib/playback/rust-playback-engine';

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, '../../..');
const GLUE = path.join(ROOT, 'web/app/public/rust/musicpack_wasm.js');
const WASM = path.join(ROOT, 'web/app/public/rust/musicpack_wasm_bg.wasm');
const REFERENCE_CONTAINER = path.join(ROOT, 'fixtures/reference/reference-small.mpak');

const available = existsSync(GLUE) && existsSync(WASM) && existsSync(REFERENCE_CONTAINER);

const CONTAINER = 'https://library.test/reference-small.mpak';

type RangeRead = (url: string, offset: number, len: number) => Uint8Array;

interface WasmModule {
  initSync(input: { module: WebAssembly.Module }): void;
  containerTracks(read: RangeRead, container: string, size: number): string;
}

function loadWasm(): WasmModule {
  const glue = readFileSync(GLUE, 'utf8');
  // eslint-disable-next-line no-new-func
  const factory = new Function(`${glue}\n;return wasm_bindgen;`);
  const wasm_bindgen = factory() as WasmModule;
  wasm_bindgen.initSync({ module: new WebAssembly.Module(readFileSync(WASM)) });
  return wasm_bindgen;
}

const BLOCK = 64 * 1024;

/** A block-aligned host over a whole container — the browser's shape. */
function rangeSource(bytes: Uint8Array, expectUrl: string) {
  const asked: Array<{ url: string; offset: number; len: number }> = [];
  const read: RangeRead = (url, offset, len) => {
    asked.push({ url, offset, len });
    if (url !== expectUrl) throw new Error(`host asked for '${url}'`);
    const base = Math.floor(offset / BLOCK) * BLOCK;
    const end = Math.min(base + BLOCK, bytes.length);
    const from = Math.min(Math.max(offset, base), bytes.length);
    const to = Math.min(from + len, end);
    return bytes.subarray(from, Math.max(from, to));
  };
  return { read, asked };
}

/** A reader backed by the real wasm module, as the worker's is. */
function containerReader(wasm: WasmModule = loadWasm()): ContainerReader & {
  askedFor: string[];
} {
  const bytes = new Uint8Array(readFileSync(REFERENCE_CONTAINER));
  const urls: string[] = [];
  return {
    askedFor: urls,
    openContainer: async (container, size) => {
      const { read, asked } = rangeSource(bytes, container);
      urls.push(...asked.map((a) => a.url));
      return JSON.parse(wasm.containerTracks(read, container, size)) as ContainerAlbumWire;
    },
  };
}

describe.skipIf(!available)('mpak MANF tracks in the client queue', () => {
  it('discovers a committed container MANF through the real core reader', () => {
    const wasm = loadWasm();
    const bytes = new Uint8Array(readFileSync(REFERENCE_CONTAINER));
    const { read, asked } = rangeSource(bytes, CONTAINER);
    const album = JSON.parse(
      wasm.containerTracks(read, CONTAINER, bytes.length),
    ) as ContainerAlbumWire;

    // MANF identity, read through core's container implementation.
    expect(album.container).toBe(CONTAINER);
    expect(album.size).toBe(bytes.length);
    expect(album.title).toBe('Reference Container');
    expect(album.artists).toEqual(['Tester']);
    expect(album.releaseType).toBe('ep');

    // Every MANF track, with its member matching the manifest.
    expect(album.tracks).toHaveLength(1);
    const track = album.tracks[0]!;
    expect(track.member).toBe('audio/01.bin');
    expect(track.size).toBeGreaterThan(0);

    // The host was only ever asked for the container — never a member key.
    expect(asked.length).toBeGreaterThan(0);
    for (const call of asked) expect(call.url).toBe(CONTAINER);
  });

  it('gives every discovered track the canonical source, and it round-trips', async () => {
    const album = await containerReader().openContainer(CONTAINER, new Uint8Array(
      readFileSync(REFERENCE_CONTAINER),
    ).length);

    expect(album.tracks.length).toBeGreaterThan(0);
    for (const track of album.tracks) {
      expect(track.source).toBe(`mpak:${CONTAINER}#${track.member}`);
      // The key carries both identities and nothing else, so a track can
      // never resolve to the container or to a different member.
      const hash = track.source.indexOf('#');
      expect(hash).toBeGreaterThan('mpak:'.length);
      expect(track.source.slice(0, hash)).toBe(`mpak:${CONTAINER}`);
      expect(track.source.slice(hash + 1)).toBe(track.member);
    }
  });

  it('feeds a discovered album into the existing queue', async () => {
    const items = await containerQueueItems(containerReader(), CONTAINER, 1365);
    expect(items).toHaveLength(1);

    // These are ordinary QueueItems in the ordinary queue — no container queue.
    const store = createQueueStore();
    const current = store.playItems(items);
    expect(store.get().items).toHaveLength(1);
    expect(store.get().index).toBe(0);
    expect(current.source.url).toBe(items[0]!.source.url);

    // Appending uses the same enqueue path as a server-backed release.
    const appended = createQueueStore();
    appended.addItems(items);
    expect(appended.get().items).toHaveLength(items.length);
    expect(appended.get().items[0]!.source.url).toBe(items[0]!.source.url);
  });

  it('preserves MANF metadata and keeps track and member identity aligned', async () => {
    const reader = containerReader();
    const size = new Uint8Array(readFileSync(REFERENCE_CONTAINER)).length;
    const album = await reader.openContainer(CONTAINER, size);
    const item = itemsForContainer(album)[0]!;
    const track = album.tracks[0]!;

    // Display metadata came from the MANF.
    expect(item.title).toBe(track.title);
    expect(item.albumTitle).toBe('Reference Container');
    expect(item.artist).toBe('Tester');
    expect(item.track.title).toBe(track.title);
    expect(item.track.number).toBe(track.number);

    // The member identity is the same everywhere it appears.
    expect(item.source.url).toBe(track.source);
    expect(item.track.audio.url).toBe(track.source);

    // A container source carries the CONTAINER's size (the scan reads its tail
    // framing); the member's own size rides on the track.
    expect(item.source.byteSize).toBe(album.size);
    expect(item.track.audio.size).toBe(track.size);
    expect(item.source.byteSize).not.toBe(track.size);
  });

  it('gives two members of one container independent identities', () => {
    // Driven from the wire shape Rust produces, so this file still contains no
    // container knowledge: it only checks that two members of one container
    // cannot collapse onto one identity.
    const album: ContainerAlbumWire = {
      container: CONTAINER,
      size: 4096,
      title: 'Two',
      artists: ['A'],
      tracks: [
        {
          number: 1,
          title: 'One',
          member: 'audio/01.mpc',
          source: `mpak:${CONTAINER}#audio/01.mpc`,
          size: 100,
          codec: 'musepack-sv8',
        },
        {
          number: 2,
          title: 'Two',
          member: 'audio/02.mpc',
          source: `mpak:${CONTAINER}#audio/02.mpc`,
          size: 200,
          codec: 'musepack-sv8',
        },
      ],
    };
    const items = itemsForContainer(album);
    expect(items).toHaveLength(2);
    const [one, two] = items as [typeof items[0], typeof items[0]];

    // Distinct sources, ids and trackIds: two members never alias, which is
    // what keeps the player's per-track length cache correct.
    expect(one.source.url).not.toBe(two.source.url);
    expect(one.id).not.toBe(two.id);
    expect(one.trackId).not.toBe(two.trackId);
    expect(one.track.audio.size).toBe(100);
    expect(two.track.audio.size).toBe(200);

    // And they are stable across rebuilds (the same container always yields the
    // same identities, so a restored session still matches).
    const again = itemsForContainer(album);
    expect(again.map((i) => [i.id, i.trackId])).toEqual(
      items.map((i) => [i.id, i.trackId]),
    );

    // Both enter the queue independently, in manifest order.
    const store = createQueueStore();
    store.addItems(items);
    expect(store.get().items.map((i) => i.source.url)).toEqual([
      `mpak:${CONTAINER}#audio/01.mpc`,
      `mpak:${CONTAINER}#audio/02.mpc`,
    ]);
  });

  it('a container that cannot be read fails closed, with no partial album', async () => {
    const wasm = loadWasm();
    const url = 'https://library.test/broken.mpak';
    const junk = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
    // Not a container at all: an error, never an empty album.
    expect(() => wasm.containerTracks(rangeSource(junk, url).read, url, junk.length)).toThrow();

    // A missing length is refused rather than scanned blindly.
    const bytes = new Uint8Array(readFileSync(REFERENCE_CONTAINER));
    await expect(containerQueueItems(containerReader(), CONTAINER, 0)).rejects.toThrow(
      /length in bytes/,
    );
    // ...and a broken container that reports no tracks never reaches the queue.
    const empty: ContainerReader = {
      openContainer: async () => ({
        container: url,
        size: 8,
        title: 'Empty',
        artists: [],
        tracks: [],
      }),
    };
    await expect(containerQueueItems(empty, url, 8)).rejects.toThrow(/no playable tracks/);
  });
});
