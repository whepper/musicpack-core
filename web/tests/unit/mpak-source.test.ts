// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Stage 1, browser layer: what the `.mpak` container source must get right
// *here*, as opposed to in Rust.
//
// The decode equivalence itself is proven where the format logic lives — the
// engine (`crates/musicpack-engine/tests/mpak_range_source.rs`) and the wasm
// boundary (`a_container_member_decodes_identically_over_the_range_source`) —
// both against the frozen PCM oracle. Duplicating a container writer in
// TypeScript to re-prove it here would put format logic in the wrong layer
// (web/AGENTS.md), so these tests deliberately cover only what is specific to
// running in a worker:
//
//   1. **Transport routing.** The engine asks the host for the *container's* URL,
//      never the `mpak:...#member` key. A host therefore implements one flat
//      range reader, and the online (HTTP networker) and offline (OPFS handle)
//      paths stay interchangeable.
//   2. **The container is really scanned** through that transport: reads reach
//      the container's tail framing, so no hidden whole-file access exists.
//   3. **A missing transport size is reported**, not guessed.
//   4. **Plain range sources are unaffected** by the container path.
//
// The committed `fixtures/reference/reference-small.mpak` is the container used
// here: a real artifact, produced by the reference packer, whose member bytes
// are independently known (its pack source `fixtures/reference/mpak-source/` is
// committed too). Its audio member is a stub, so it is never decoded — these
// tests assert routing and scanning, not PCM.
//
// Skips with a notice when the generated wasm or the container is unavailable,
// matching the repository's existing wasm-test pattern.

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, '../../..');
const GLUE = path.join(ROOT, 'web/app/public/rust/musicpack_wasm.js');
const WASM = path.join(ROOT, 'web/app/public/rust/musicpack_wasm_bg.wasm');
const CONTAINER = path.join(ROOT, 'fixtures/reference/reference-small.mpak');
const CONTAINER_SOURCE = path.join(ROOT, 'fixtures/reference/mpak-source');

const available = existsSync(GLUE) && existsSync(WASM) && existsSync(CONTAINER);

interface EngineApi {
  open(itemJson: string): string;
  add_source(url: string, bytes: Uint8Array): void;
  start(): void;
  play(): void;
  render(frames: number): Float32Array;
  take_error(): string | undefined;
  close(): void;
}

interface WasmModule {
  initSync(input: { module: WebAssembly.Module }): void;
  WasmEngine: {
    new (rate: number, channels: number): EngineApi;
    newRangeSource(rate: number, channels: number, read: RangeRead): EngineApi;
  };
}

type RangeRead = (url: string, offset: number, len: number) => Uint8Array;

const BLOCK = 64 * 1024;

function loadWasm(): WasmModule {
  const glue = readFileSync(GLUE, 'utf8');
  // eslint-disable-next-line no-new-func
  const factory = new Function(`${glue}\n;return wasm_bindgen;`);
  const wasm_bindgen = factory() as WasmModule;
  wasm_bindgen.initSync({ module: new WebAssembly.Module(readFileSync(WASM)) });
  return wasm_bindgen;
}

/**
 * A block-aligned transport, exactly like `networker.js` behind the mailbox and
 * `localreader.js` for OPFS: a reply never crosses the containing 64 KiB block.
 *
 * `expectUrl` is the transport the engine must ask for. Any other URL is a
 * contract violation, so the fake fails the read rather than quietly serving it.
 * Every request is recorded so a test can assert what was actually read.
 */
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

function memberItem(
  containerUrl: string,
  member: string,
  byteSize: number | null,
  kind = 'mpak'
): string {
  return JSON.stringify({
    id: 't1',
    trackId: 1,
    url: `mpak:${containerUrl}#${member}`,
    kind,
    byteSize,
    durationHintSeconds: null,
    title: 'One',
    artist: 'Tester',
    albumTitle: 'Reference Container',
    codec: 'musepack-sv8',
  });
}

function plainItem(url: string): string {
  return JSON.stringify({
    id: 't1',
    trackId: 1,
    url,
    kind: 'http-range',
    durationHintSeconds: null,
    title: 'One',
    artist: 'Tester',
    albumTitle: 'Reference Container',
    codec: 'musepack-sv8',
  });
}

describe.skipIf(!available)('mpak container members over a range source', () => {
  const HTTPS_URL = 'https://library.test/packages/reference-small.mpak';
  // An offline container is addressed by an OPFS store key rather than a URL;
  // the key is what the host resolves to a sync access handle.
  const STORE_KEY = 'musicpack-offline-v1/releases/release-42';

  it('asks the host only for the container URL, never the member key', () => {
    const wasm = loadWasm();
    const container = new Uint8Array(readFileSync(CONTAINER));
    const { read, asked } = rangeSource(container, HTTPS_URL);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);

    // The member is a stub, so opening it cannot succeed; what matters is that
    // the failure is a decode failure, never a transport or parse failure.
    let failure = '';
    try {
      engine.open(memberItem(HTTPS_URL, 'audio/01.bin', container.length));
    } catch (e) {
      failure = String(e);
    }
    engine.close();

    expect(asked.length).toBeGreaterThan(0);
    for (const call of asked) expect(call.url).toBe(HTTPS_URL);
    expect(failure).not.toMatch(/host asked for/);
    expect(failure).not.toMatch(/malformed 'mpak:'/);
    expect(failure).not.toMatch(/cannot scan container/);
  });

  it('scans the container through the transport rather than reading it whole', () => {
    const wasm = loadWasm();
    const container = new Uint8Array(readFileSync(CONTAINER));
    const { read, asked } = rangeSource(container, HTTPS_URL);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);
    try {
      engine.open(memberItem(HTTPS_URL, 'audio/01.bin', container.length));
    } catch {
      /* the member is a stub; the scan is what is under test */
    }
    engine.close();

    // The tail framing lives at the end of the file, so reaching it proves the
    // container index was parsed from ranged reads.
    const highest = asked.reduce((max, c) => Math.max(max, c.offset + c.len), 0);
    expect(highest).toBeGreaterThan(container.length - BLOCK);
    // Reading the member itself must not mean reading everything: a demand-driven
    // reader stays far below the whole file for a small container only by
    // accident, so assert the framing was reached *and* every read stayed inside
    // the file.
    for (const call of asked) {
      expect(call.offset).toBeGreaterThanOrEqual(0);
      expect(call.offset + call.len).toBeLessThanOrEqual(container.length);
    }
  });

  it('works the same through an offline (OPFS-shaped) transport', () => {
    const wasm = loadWasm();
    const container = new Uint8Array(readFileSync(CONTAINER));
    const { read, asked } = rangeSource(container, STORE_KEY);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);
    try {
      engine.open(memberItem(STORE_KEY, 'audio/01.bin', container.length, 'local-file'));
    } catch {
      /* the member is a stub; transport identity is what is under test */
    }
    engine.close();
    expect(asked.length).toBeGreaterThan(0);
    for (const call of asked) expect(call.url).toBe(STORE_KEY);
  });

  it('reports a missing transport size instead of guessing', () => {
    const wasm = loadWasm();
    const container = new Uint8Array(readFileSync(CONTAINER));
    const { read } = rangeSource(container, HTTPS_URL);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);
    expect(() => engine.open(memberItem(HTTPS_URL, 'audio/01.bin', null))).toThrow(
      /byte_size/
    );
    engine.close();
  });

  it('rejects a malformed container key', () => {
    const wasm = loadWasm();
    const container = new Uint8Array(readFileSync(CONTAINER));
    const { read } = rangeSource(container, HTTPS_URL);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);
    expect(() =>
      engine.open(
        JSON.stringify({
          id: 't1',
          trackId: 1,
          url: 'mpak:no-separator-here',
          kind: 'mpak',
          byteSize: container.length,
          codec: 'musepack-sv8',
        })
      )
    ).toThrow(/malformed 'mpak:'/);
    engine.close();
  });

  it('leaves a plain range source working', () => {
    const wasm = loadWasm();
    // The container is not a decodable stream, so use the *member* bytes read
    // straight from the pack source: a plain source must serve them verbatim.
    const member = new Uint8Array(readFileSync(path.join(CONTAINER_SOURCE, 'audio/01.bin')));
    const { read, asked } = rangeSource(member, HTTPS_URL);
    const engine = wasm.WasmEngine.newRangeSource(44100, 2, read);
    let failure = '';
    try {
      engine.open(plainItem(HTTPS_URL));
    } catch (e) {
      failure = String(e);
    }
    engine.close();
    // A plain key is passed through untouched: the host was asked for it, and
    // no container framing was consulted.
    expect(asked.length).toBeGreaterThan(0);
    for (const call of asked) expect(call.url).toBe(HTTPS_URL);
    expect(failure).not.toMatch(/malformed 'mpak:'/);
    expect(failure).not.toMatch(/byte_size/);
  });
});
