// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// RustEngine control-plane contract tests, driven by a fake WasmEngineApi.
// These verify the async TS lifecycle <-> synchronous Rust mapping without
// requiring the wasm build (see rust-engine-wasm.test.ts for the real binding).

import { describe, expect, it } from 'vitest';
import {
  RustEngine,
  type EngineHandlers,
  type WasmEngineApi,
} from '../../app/src/lib/playback/rust-engine';
import type { PlaybackItem } from '../../player-core/src/types';

function item(url = '/f.mpc', id = 't1'): PlaybackItem {
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

const INFO = { rate: 44100, channels: 2, version: 8, lengthSamples: 44100 };

class FakeWasm implements WasmEngineApi {
  sources = new Map<string, Uint8Array>();
  opened: string | null = null;
  pumping = false;
  playing = false;
  gain = 1;
  rendered = 0;
  drained = false;
  backpressure = true;
  signal = 0.5;
  errors: string[] = [];
  xfadeResult: string | null = null;
  xfadeTag = 'pending';
  closed = false;
  framesRendered = 0;

  add_source(url: string, bytes: Uint8Array): void {
    this.sources.set(url, bytes);
  }
  open(json: string): string {
    this.opened = json;
    return JSON.stringify(INFO);
  }
  start(): void {
    this.pumping = true;
  }
  stop(): void {
    this.pumping = false;
  }
  play(): void {
    this.playing = true;
  }
  pause(): void {
    this.playing = false;
  }
  seek(_samples: number): void {
    this.rendered = 0;
    this.drained = false;
  }
  set_gain(linear: number): void {
    this.gain = linear;
  }
  rendered_samples(): number {
    return this.rendered;
  }
  prepare_next(_json: string): string {
    return JSON.stringify({ ...INFO, lengthSamples: 22050 });
  }
  advance(_json: string | null | undefined): string {
    return JSON.stringify(INFO);
  }
  begin_crossfade(_json: string, _fade: number): string {
    return this.xfadeTag;
  }
  is_output_drained(): boolean {
    return this.drained;
  }
  backpressured(): boolean {
    return this.backpressure;
  }
  render(frames: number): Float32Array {
    const pcm = new Float32Array(frames * 2);
    pcm[0] = this.signal;
    this.framesRendered += frames;
    if (this.pumping || this.playing) this.rendered += frames;
    return pcm;
  }
  take_crossfade_result(): string | null {
    const r = this.xfadeResult;
    this.xfadeResult = null;
    return r;
  }
  take_error(): string | null {
    return this.errors.shift() ?? null;
  }
  close(): void {
    this.closed = true;
    this.opened = null;
  }
}

function makeEngine(wasm: FakeWasm, handlers: Partial<EngineHandlers> = {}) {
  return new RustEngine({ wasm, loadBytes: async () => new Uint8Array([1, 2, 3]), handlers });
}

/** Lets pending async adapter work (byte load) settle. */
function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('RustEngine (control-plane seam)', () => {
  it('exposes honest capabilities and initializes', async () => {
    const engine = makeEngine(new FakeWasm());
    expect(engine.capabilities).toEqual({
      preloadNext: true,
      sampleAccurateGapless: true,
      decodeGate: true,
      crossfade: true,
    });
    await engine.init(null);
  });

  it('open registers the source, awaits bytes and returns StreamInfo', async () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    const info = await engine.open(item('/a.mpc'));
    expect(info).toEqual(INFO);
    expect(wasm.sources.has('/a.mpc')).toBe(true);
    expect(JSON.parse(wasm.opened!).url).toBe('/a.mpc');
    expect(engine.renderedSamples()).toBe(0);
  });

  it('play/pause drive the decode gate and are repeatable', async () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    await engine.open(item());
    await engine.play();
    expect(wasm.pumping).toBe(true);
    await engine.pause();
    expect(wasm.pumping).toBe(false);
    await engine.play();
    await engine.pause();
    await engine.play();
    expect(wasm.pumping).toBe(true);
  });

  it('seek resets rendered samples; gain is applied; rendered samples advance', async () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    await engine.open(item());
    await engine.play();
    engine.render(1024);
    engine.render(1024);
    expect(engine.renderedSamples()).toBe(2048);
    await engine.seekSample(500);
    expect(engine.renderedSamples()).toBe(0);
    engine.setGain(0.25);
    expect(wasm.gain).toBe(0.25);
  });

  it('emits primed once, then tick', async () => {
    const wasm = new FakeWasm();
    const names: string[] = [];
    const engine = makeEngine(wasm, { primed: () => names.push('primed'), tick: () => names.push('tick') });
    engine.on('primed', () => names.push('on:primed'));
    await engine.open(item());
    await engine.play();
    engine.render(512);
    engine.render(512);
    expect(names.filter((n) => n === 'primed' || n === 'on:primed')).toHaveLength(2);
    expect(names.filter((n) => n === 'tick').length).toBeGreaterThanOrEqual(2);
  });

  it('emits buffering on underrun while pumping, not when drained', async () => {
    const wasm = new FakeWasm();
    wasm.signal = 0;
    const names: string[] = [];
    const engine = makeEngine(wasm, { buffering: () => names.push('buffering') });
    await engine.open(item());
    await engine.play();
    engine.render(512);
    expect(names).toEqual(['buffering']);
    // Recovery: real signal clears the buffering latch.
    wasm.signal = 0.5;
    engine.render(512);
    wasm.signal = 0;
    engine.render(512);
    expect(names).toEqual(['buffering', 'buffering']);
  });

  it('emits eos exactly once when the output is drained', async () => {
    const wasm = new FakeWasm();
    const names: string[] = [];
    const engine = makeEngine(wasm, { eos: () => names.push('eos') });
    await engine.open(item());
    await engine.play();
    wasm.drained = true;
    engine.render(512);
    engine.render(512);
    expect(names).toEqual(['eos']);
  });

  it('propagates engine errors', async () => {
    const wasm = new FakeWasm();
    const messages: string[] = [];
    const engine = makeEngine(wasm, { error: (m) => messages.push(m) });
    await engine.open(item());
    await engine.play();
    wasm.errors.push('decode failed');
    engine.render(512);
    expect(messages).toEqual(['decode failed']);
  });

  it('prepareNext and advance return stream info', async () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    await engine.open(item());
    expect((await engine.prepareNext(item('/b.mpc', 't2')))?.lengthSamples).toBe(22050);
    expect((await engine.advance(item('/b.mpc', 't2')))?.rate).toBe(44100);
    expect(await engine.advance(null)).toEqual(INFO);
  });

  it('crossfade: pending resolves with the engine result on render', async () => {
    const wasm = new FakeWasm();
    wasm.xfadeTag = 'pending';
    wasm.xfadeResult = JSON.stringify({ info: INFO, overlapFrames: 1000 });
    const engine = makeEngine(wasm);
    await engine.open(item());
    await engine.play();
    const promise = engine.beginCrossfade(item('/b.mpc', 't2'), 4);
    await flush();
    engine.render(512);
    const result = await promise;
    expect(result?.overlapFrames).toBe(1000);
    expect(result?.info.rate).toBe(44100);
  });

  it('crossfade: declined resolves null', async () => {
    const wasm = new FakeWasm();
    wasm.xfadeTag = 'declined';
    const engine = makeEngine(wasm);
    await engine.open(item());
    expect(await engine.beginCrossfade(item('/b.mpc', 't2'), 4)).toBeNull();
  });

  it('DecodeGate start/stop map to the pump', () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    engine.start();
    expect(wasm.pumping).toBe(true);
    engine.stop();
    expect(wasm.pumping).toBe(false);
  });

  it('close rejects an in-flight crossfade and is idempotent-safe', async () => {
    const wasm = new FakeWasm();
    wasm.xfadeTag = 'pending';
    const engine = makeEngine(wasm);
    await engine.open(item());
    await engine.play();
    const promise = engine.beginCrossfade(item('/b.mpc', 't2'), 4);
    await engine.close();
    await expect(promise).rejects.toThrow('engine closed');
    expect(wasm.closed).toBe(true);
    expect(engine.isClosed()).toBe(true);
  });

  it('reopens cleanly after close', async () => {
    const wasm = new FakeWasm();
    const engine = makeEngine(wasm);
    await engine.open(item('/a.mpc'));
    await engine.close();
    const info = await engine.open(item('/b.mpc', 't2'));
    expect(info.rate).toBe(44100);
    expect(wasm.sources.has('/b.mpc')).toBe(true);
  });

  it('open rejects when the byte source fails', async () => {
    const wasm = new FakeWasm();
    const engine = new RustEngine({
      wasm,
      loadBytes: async () => {
        throw new Error('source unavailable');
      },
    });
    await expect(engine.open(item())).rejects.toThrow('source unavailable');
  });

  it('requires a wasm engine instance or factory', () => {
    const engine = new RustEngine({ loadBytes: async () => new Uint8Array() });
    expect(() => engine.start()).toThrow(/wasm engine/);
  });
});
