import { describe, it, expect, beforeAll, type TestContext } from 'vitest';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import { RingBuffer } from '../../app/src/lib/playback/ring-buffer';
import { StreamingResampler } from '../../app/src/lib/playback/streaming-resampler';

const require = createRequire(import.meta.url);
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

let wasm: (() => Promise<Record<string, any>>) | null = null;
let fixtureA: string | null = null;
let fixtureB: string | null = null;

beforeAll(() => {
  const candidates = [
    process.env.MUSICPACK_WASM_JS,
    path.join(ROOT, 'build-wasm/wasm/musepack.js'),
  ].filter(Boolean) as string[];
  const mod = candidates.find((p) => {
    try {
      require(p);
      return true;
    } catch {
      return false;
    }
  });
  const fa = path.join(ROOT, 'tests/fixtures/sine44-q5.mpc');
  const fb = path.join(ROOT, 'tests/fixtures/sine44-q7.mpc');
  if (mod && readFileSync(fa).length && readFileSync(fb).length) {
    wasm = require(mod) as () => Promise<Record<string, any>>;
    fixtureA = fa;
    fixtureB = fb;
  }
});

async function decodeAll(Module: Record<string, any>, h: number, channels: number): Promise<Float32Array> {
  const pcmPtr = Module._malloc(1152 * channels * 4);
  const out: number[] = [];
  for (;;) {
    const frames = (await Module._mpc_wasm_read(h, pcmPtr, 1152)) as number;
    if (frames < 0) break; // EOF
    if (frames === 0) break;
    const view = new Float32Array(Module.HEAPF32.buffer, pcmPtr, frames * channels);
    for (let i = 0; i < view.length; i++) out.push(view[i] ?? 0);
  }
  Module._free(pcmPtr);
  return Float32Array.from(out);
}

async function decode(Module: Record<string, any>, file: string) {
  const bytes = readFileSync(file);
  const h = Module._mpc_wasm_create();
  const memPtr = Module._malloc(bytes.length);
  Module.HEAPU8.set(bytes, memPtr);
  const err = await Module._mpc_wasm_open(h, memPtr, bytes.length);
  if (err !== 0) throw new Error(`open: ${err}`);
  const channels = Module._mpc_wasm_channels(h);
  const length = Module._mpc_wasm_length_samples(h);
  const rate = Module._mpc_wasm_sample_rate(h);
  const pcm = await decodeAll(Module, h, channels);
  Module._free(memPtr);
  Module._mpc_wasm_destroy(h);
  return { pcm, channels, length, rate };
}

describe('wasm + ring (gapless feed)', () => {
  it('needs the wasm module built (skip otherwise)', (ctx: TestContext) => {
    if (!fixtureA || !fixtureB) ctx.skip();
    expect(fixtureA).toBeTruthy();
    expect(fixtureB).toBeTruthy();
  });

  it('feeds two decoded tracks through the fixed-rate playback ring at the boundary', async () => {
    if (!wasm || !fixtureA || !fixtureB) return;
    const Module = await wasm();
    const A = await decode(Module, fixtureA);
    const B = await decode(Module, fixtureB);
    expect(A.channels).toBe(B.channels);
    expect(A.rate).toBe(B.rate);

    const outputRate = 48000;
    const outputChannels = 2;
    const outputFramesA = Math.ceil((A.length * outputRate) / A.rate);
    const outputFramesB = Math.ceil((B.length * outputRate) / B.rate);
    const ring = new RingBuffer(outputFramesA + outputFramesB, outputChannels);

    for (const track of [A, B]) {
      const resampler = new StreamingResampler(
        track.rate,
        track.channels,
        outputRate,
        outputChannels,
      );
      expect(resampler.process(track.pcm, 0, ring)).toBe(track.length);
      expect(resampler.finish(ring)).toBe(true);
    }

    expect(ring.availableFrames).toBe(outputFramesA + outputFramesB);
    const drained = new Float32Array(ring.availableFrames * outputChannels);
    expect(ring.readInterleaved(drained, outputFramesA + outputFramesB)).toBe(
      outputFramesA + outputFramesB,
    );
    expect(drained[0]).toBe(A.pcm[0]);
    expect(drained[outputFramesA * outputChannels]).toBe(B.pcm[0]);
    expect(drained[(outputFramesA + outputFramesB - 1) * outputChannels]).toBe(
      B.pcm[(B.length - 1) * B.channels],
    );
  });
});
