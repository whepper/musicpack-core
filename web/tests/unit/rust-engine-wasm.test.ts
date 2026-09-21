// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// RustEngine against the REAL musicpack-wasm binding (Node build). Skips with
// a notice when the wasm package or fixture corpus is unavailable, matching the
// repository's reference-tooling pattern.

import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { RustEngine, type WasmEngineApi } from '../../app/src/lib/playback/rust-engine';
import type { PlaybackItem } from '../../player-core/src/types';

const here = path.dirname(fileURLToPath(import.meta.url));
// Repo-local paths: the node binding is produced by `tools/wasm_smoke.sh`
// (target/wasm-node) and the fixture corpus lives at the repository root.
const wasmPath =
  process.env.MUSICPACK_WASM_NODE ??
  path.resolve(here, '../../../target/wasm-node/musicpack_wasm.js');
const fixture = path.resolve(here, '../../../tests/fixtures/musepack/sine44-q5.mpc');
const runnable = existsSync(wasmPath) && existsSync(fixture);

if (!runnable) {
  // eslint-disable-next-line no-console
  console.warn(
    'note: RustEngine wasm integration skipped (build musicpack-wasm into ' +
      `${path.dirname(wasmPath)}; set MUSICPACK_WASM_NODE to override)`,
  );
}

interface WasmModule {
  WasmEngine: new (rate: number, channels: number) => WasmEngineApi;
}

function mpcItem(url: string, id: string): PlaybackItem {
  return {
    id,
    trackId: 1,
    source: { kind: 'http-range', url },
    title: 'T',
    artist: 'A',
    albumTitle: 'AL',
    codec: 'musepack-sv8',
  } as unknown as PlaybackItem;
}

describe.skipIf(!runnable)('RustEngine + real musicpack-wasm (Node)', () => {
  const mod: WasmModule | null = runnable
    ? (createRequire(import.meta.url)(wasmPath) as WasmModule)
    : null;

  function makeEngine(): RustEngine {
    const bytes = new Uint8Array(readFileSync(fixture));
    return new RustEngine({
      loadBytes: async () => bytes,
      createWasm: (rate, channels) => new mod!.WasmEngine(rate, channels),
    });
  }

  it('opens, decodes real PCM, seeks and closes', async () => {
    const engine = makeEngine();
    const events: string[] = [];
    engine.on('eos', () => events.push('eos'));
    engine.on('primed', () => events.push('primed'));

    const info = await engine.open(mpcItem('/f.mpc', 't1'));
    expect(info.rate).toBe(44100);
    expect(info.channels).toBe(2);
    // `version` is 0 from the generic engine seam (Musepack's stream version
    // is not plumbed through AudioInfo); the TS contract allows 0 = unknown.
    expect(typeof info.version).toBe('number');
    expect(info.lengthSamples).toBe(44100);

    engine.start();
    await engine.play();

    let audible = false;
    for (let i = 0; i < 400 && !events.includes('eos'); i++) {
      const pcm = engine.render(1152);
      if (pcm.some((s) => Math.abs(s) > 0.01)) audible = true;
    }
    expect(audible).toBe(true);
    expect(events).toContain('primed');
    expect(events).toContain('eos');
    expect(engine.renderedSamples()).toBe(44100);

    await engine.seekSample(1000);
    expect(engine.renderedSamples()).toBe(0);
    await engine.close();
    expect(engine.isClosed()).toBe(true);
  });

  it('opens again after close (lifecycle)', async () => {
    const engine = makeEngine();
    expect((await engine.open(mpcItem('/a.mpc', 'a'))).lengthSamples).toBe(44100);
    await engine.close();
    expect((await engine.open(mpcItem('/b.mpc', 'b'))).lengthSamples).toBe(44100);
    await engine.close();
  });
});
