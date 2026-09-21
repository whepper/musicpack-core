// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// RustEngine — an ADDITIVE browser adapter implementing the existing
// player-core Engine/PreloadEngine/CrossfadeEngine/DecodeGate contracts over
// `musicpack-wasm`'s `WasmEngine`.
//
// This file is NOT wired into production. `chooseBackend`/`createWebEngine`
// still select MusepackEngine/NativeBackend; nothing here runs unless a host
// (or a test) explicitly constructs it. It exists to prove the control-plane
// seam before the separate AudioWorklet and source-bridge work.
//
// Source model (Phase 14A): the adapter takes an injected `loadBytes(item)`
// resolver. Production demand-range streaming is deliberately NOT assumed —
// the existing SharedArrayBuffer range reader remains the production path and
// this adapter is exercised with a fixture/in-memory source until a range
// bridge is designed. See the Phase 14A report.
//
// PCM: the adapter exposes `render(frames)` (the host pull). It does NOT touch
// MusicPackPcmProcessor; tests consume the returned Float32Array directly.

import type {
  CrossfadeEngine,
  CrossfadeResult,
  DecodeGate,
  Engine,
  EngineCapabilities,
  EngineEventName,
  PreloadEngine,
} from '../../../../player-core/src/engine';
import type { PlaybackItem, StreamInfo } from '../../../../player-core/src/types';

/** The subset of `musicpack-wasm`'s `WasmEngine` the adapter uses. */
export interface WasmEngineApi {
  add_source(url: string, bytes: Uint8Array): void;
  open(itemJson: string): string;
  start(): void;
  stop(): void;
  play(): void;
  pause(): void;
  seek(samples: number): void;
  set_gain(linear: number): void;
  rendered_samples(): number;
  prepare_next(itemJson: string): string | null | undefined;
  advance(expectedJson: string | null | undefined): string | null | undefined;
  begin_crossfade(nextJson: string, fadeSeconds: number): string;
  is_output_drained(): boolean;
  backpressured(): boolean;
  render(frames: number): Float32Array;
  take_crossfade_result(): string | null | undefined;
  take_error(): string | null | undefined;
  close(): void;
}

/** Constructor callbacks (the legacy web-controller handler shape). */
export interface EngineHandlers {
  primed(): void;
  buffering(): void;
  eos(): void;
  error(message: string): void;
  tick(): void;
}

export interface RustEngineOptions {
  /** Resolves an item's complete bytes. Test/fixture source for this phase. */
  loadBytes: (item: PlaybackItem) => Promise<Uint8Array>;
  /** Injected engine instance (tests). */
  wasm?: WasmEngineApi;
  /** Engine factory used when `wasm` is absent (host builds the module). */
  createWasm?: (outputRate: number, outputChannels: number) => WasmEngineApi;
  handlers?: Partial<EngineHandlers>;
  outputRate?: number;
  outputChannels?: number;
}

const RUST_CAPABILITIES: EngineCapabilities = {
  preloadNext: true,
  sampleAccurateGapless: true,
  decodeGate: true,
  crossfade: true,
};

/** Maps a PlaybackItem to the wasm engine's JSON shape. */
function toWasmItem(item: PlaybackItem): string {
  return JSON.stringify({
    id: item.id,
    trackId: item.trackId,
    url: item.source.url,
    kind: item.source.kind,
    durationHintSeconds: item.durationHintSeconds,
    title: item.title,
    artist: item.artist,
    albumTitle: item.albumTitle,
    codec: item.codec,
  });
}

function hasSignal(pcm: Float32Array): boolean {
  for (let i = 0; i < pcm.length; i++) if (pcm[i] !== 0) return true;
  return false;
}

export class RustEngine implements Engine, PreloadEngine, CrossfadeEngine, DecodeGate {
  readonly capabilities = RUST_CAPABILITIES;
  private readonly opts: RustEngineOptions;
  private readonly rate: number;
  private readonly channels: number;
  private wasm: WasmEngineApi | null = null;
  private currentUrl: string | null = null;
  private pumping = false;
  private primed = false;
  private eosEmitted = false;
  private bufferingEmitted = false;
  private closed = false;
  private listeners = new Map<EngineEventName, Set<(sender: Engine) => void>>();
  private xfade: {
    resolve: (r: CrossfadeResult | null) => void;
    reject: (e: Error) => void;
  } | null = null;
  private xfadeArmed = false;

  constructor(options: RustEngineOptions) {
    this.opts = options;
    this.rate = options.outputRate ?? 44_100;
    this.channels = options.outputChannels ?? 2;
  }

  private ensureWasm(): WasmEngineApi {
    if (this.wasm) return this.wasm;
    if (this.opts.wasm) {
      this.wasm = this.opts.wasm;
    } else if (this.opts.createWasm) {
      this.wasm = this.opts.createWasm(this.rate, this.channels);
    } else {
      throw new Error('RustEngine requires a wasm engine instance or factory.');
    }
    return this.wasm;
  }

  async init(_token: string | null): Promise<void> {
    this.ensureWasm();
  }

  on(name: EngineEventName, cb: (sender: Engine) => void): () => void {
    let set = this.listeners.get(name);
    if (!set) this.listeners.set(name, (set = new Set()));
    set.add(cb);
    return () => set!.delete(cb);
  }

  private emit(name: EngineEventName, message?: string): void {
    for (const cb of this.listeners.get(name) ?? []) cb(this);
    const h = this.opts.handlers;
    if (!h) return;
    if (name === 'primed') h.primed?.();
    else if (name === 'buffering') h.buffering?.();
    else if (name === 'eos') h.eos?.();
    else if (name === 'error') h.error?.(message ?? 'engine error');
    else if (name === 'tick') h.tick?.();
  }

  private async registerSource(item: PlaybackItem): Promise<WasmEngineApi> {
    const wasm = this.ensureWasm();
    const bytes = await this.opts.loadBytes(item);
    wasm.add_source(item.source.url, bytes);
    return wasm;
  }

  async open(item: PlaybackItem): Promise<StreamInfo> {
    const wasm = await this.registerSource(item);
    const info = JSON.parse(wasm.open(toWasmItem(item))) as StreamInfo;
    this.currentUrl = item.source.url;
    this.primed = false;
    this.eosEmitted = false;
    this.bufferingEmitted = false;
    this.closed = false;
    return info;
  }

  async play(): Promise<void> {
    const wasm = this.ensureWasm();
    wasm.start();
    wasm.play();
    this.pumping = true;
  }

  async pause(): Promise<void> {
    const wasm = this.ensureWasm();
    wasm.pause();
    wasm.stop();
    this.pumping = false;
  }

  async seekSample(samples: number): Promise<void> {
    const wasm = this.ensureWasm();
    wasm.seek(samples);
    this.primed = false;
    this.eosEmitted = false;
    this.bufferingEmitted = false;
  }

  setGain(linear: number): void {
    this.ensureWasm().set_gain(linear);
  }

  renderedSamples(): number {
    return this.wasm ? this.wasm.rendered_samples() : 0;
  }

  async close(): Promise<void> {
    if (this.xfade) {
      this.settleCrossfade(null, new Error('engine closed'));
    }
    if (this.wasm) this.wasm.close();
    this.closed = true;
    this.pumping = false;
    this.listeners.clear();
  }

  async prepareNext(item: PlaybackItem): Promise<StreamInfo | null> {
    const wasm = await this.registerSource(item);
    const json = wasm.prepare_next(toWasmItem(item));
    return json ? (JSON.parse(json) as StreamInfo) : null;
  }

  async advance(expected: PlaybackItem | null): Promise<StreamInfo | null> {
    const wasm = this.ensureWasm();
    const json = wasm.advance(expected ? toWasmItem(expected) : undefined);
    if (json) {
      this.primed = false;
      this.eosEmitted = false;
      this.bufferingEmitted = false;
      return JSON.parse(json) as StreamInfo;
    }
    return null;
  }

  async beginCrossfade(next: PlaybackItem, fadeSeconds: number): Promise<CrossfadeResult | null> {
    // Register the pending transition synchronously so close()/render() can
    // observe it before the (async) byte load completes.
    const promise = new Promise<CrossfadeResult | null>((resolve, reject) => {
      this.xfade = { resolve, reject };
      this.xfadeArmed = false;
    });
    try {
      const wasm = await this.registerSource(next);
      if (this.closed) return promise; // close() already rejected it
      const tag = wasm.begin_crossfade(toWasmItem(next), fadeSeconds);
      if (tag === 'declined') {
        this.settleCrossfade(null);
      } else if (tag === 'completed') {
        const json = wasm.take_crossfade_result();
        this.settleCrossfade(json ? (JSON.parse(json) as CrossfadeResult) : null);
      } else {
        this.xfadeArmed = true;
      }
    } catch (e) {
      this.settleCrossfade(null, e as Error);
    }
    return promise;
  }

  private settleCrossfade(result: CrossfadeResult | null, error?: Error): void {
    const pending = this.xfade;
    this.xfade = null;
    this.xfadeArmed = false;
    if (pending) {
      if (error) pending.reject(error);
      else pending.resolve(result);
    }
  }

  // ---- DecodeGate ----------------------------------------------------------

  start(): void {
    this.ensureWasm().start();
    this.pumping = true;
  }

  stop(): void {
    this.ensureWasm().stop();
    this.pumping = false;
  }

  // ---- host pull -----------------------------------------------------------

  /**
   * Renders `frames` output frames, delivering engine facts back to the
   * player exactly as a real host would: crossfade completion, errors, priming,
   * EOS and tick. Returns the interleaved PCM.
   */
  render(frames: number): Float32Array {
    const wasm = this.ensureWasm();
    const pcm = wasm.render(frames);

    const result = wasm.take_crossfade_result();
    if (result && this.xfade && this.xfadeArmed) {
      this.settleCrossfade(JSON.parse(result) as CrossfadeResult);
    }

    const error = wasm.take_error();
    if (error) this.emit('error', error);

    if (!this.primed && !this.eosEmitted && (wasm.backpressured() || hasSignal(pcm))) {
      this.primed = true;
      this.emit('primed');
    }

    if (this.pumping && !this.eosEmitted && !wasm.is_output_drained() && !hasSignal(pcm)) {
      if (!this.bufferingEmitted) {
        this.bufferingEmitted = true;
        this.emit('buffering');
      }
    } else {
      this.bufferingEmitted = false;
    }

    if (wasm.is_output_drained() && !this.eosEmitted) {
      this.eosEmitted = true;
      this.emit('eos');
    }

    this.emit('tick');
    return pcm;
  }

  /** Whether the adapter has been closed. Test/diagnostic accessor. */
  isClosed(): boolean {
    return this.closed;
  }
}
