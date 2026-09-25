// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// RustPlaybackEngine — the production-side browser adapter that joins the
// existing player-core `Engine` / `PreloadEngine` / `CrossfadeEngine` /
// `DecodeGate` contracts to the proven Rust browser playback path:
//
//   PlayerController
//     └─ createWebEngine('rust', handlers)
//          └─ RustPlaybackEngine
//               ├─ rust-playback.worker.js   (WASM + range source + bounded ring)
//               │     ├─ networker.js / reader_mailbox.js  (Phase 14D/14E)
//               │     ├─ musicpack-engine decoder          (Phase 14C)
//               │     └─ WasmEngine                         (Phase 14A)
//               └─ rust-playback-sink.js AudioWorklet       (Phase 14F, dumb PCM sink)
//
// PlayerController sees only Engine-level concepts: it never learns about
// WASM, workers, SABs, Atomics, range mailboxes, the PCM ring or the
// AudioWorklet. PCM never travels through the main thread — it flows from the
// producer worker through the bounded ring into the sink. Rust
// `rendered_samples()` remains the authoritative position; the AudioContext
// clock is never consulted for playback position.
//
// This adapter is constructed only by tests or an explicit test hook in
// 14H-1. It is NOT reachable through normal application configuration:
// `chooseBackend` still resolves 'musepack' | 'native' only. No feature flag
// exists yet.

import type {
  CrossfadeEngine,
  CrossfadeResult,
  DecodeGate,
  Engine,
  EngineCapabilities,
  EngineEventName,
  PreloadEngine,
} from '../../../../player-core/src/engine';
import type {
  PlaybackItem,
  PlaybackSource,
  StreamInfo,
} from '../../../../player-core/src/types';

/** One track a `.mpak` container's MANF defines, as reported by Rust.
 *
 *  Plain data only: `source` is the canonical `mpak:<container>#<member>` key
 *  built by core's formatter, never assembled in JavaScript. `codec`/`mimeType`
 *  are absent when the member is not a decodable stream — the track still
 *  exists, and playing it fails loudly at backend selection rather than the
 *  album looking complete. */
export interface ContainerTrackWire {
  number: number;
  title: string;
  /** The audio member's package-relative path inside the container. */
  member: string;
  /** Canonical playback source for this member. */
  source: string;
  /** The member's byte length. */
  size: number;
  codec?: string;
  mimeType?: string;
  durationSeconds?: number;
}

/** A container's album identity plus its MANF tracks. */
export interface ContainerAlbumWire {
  /** The container's transport URL/key. */
  container: string;
  /** The container's length in bytes. */
  size: number;
  title: string;
  artists: string[];
  releaseType?: string;
  tracks: ContainerTrackWire[];
}

/** Constructor callbacks (the legacy web-controller handler shape). */
export interface EngineHandlers {
  primed(): void;
  buffering(): void;
  eos(): void;
  error(message: string): void;
  tick(): void;
}

/** Injected WASM glue + module bytes (the 14E/14F test injection shape). */
export interface RustPlaybackAssets {
  glue: string;
  wasmB64: string;
}

/** The subset of the DOM Worker the adapter drives (injectable for tests). */
export interface PlaybackWorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: unknown) => void) | null;
}
export type PlaybackWorkerFactory = () => PlaybackWorkerLike;

/** The AudioContext/AudioWorklet boundary (injectable for tests). */
export interface RustAudioSink {
  readonly sampleRate: number;
  /** Points the sink at a generation's bounded PCM ring. */
  attach(control: SharedArrayBuffer, data: SharedArrayBuffer, frames: number, channels: number): void;
  resume(): Promise<void>;
  suspend(): Promise<void>;
  close(): Promise<void>;
}
export type RustAudioSinkFactory = () => Promise<RustAudioSink>;

export interface RustPlaybackEngineOptions {
  handlers: EngineHandlers;
  /** Test seams (defaults: a served worker + a private AudioContext). */
  workerFactory?: PlaybackWorkerFactory;
  sinkFactory?: RustAudioSinkFactory;
  /** WASM glue/module injected into the worker (required by the test path). */
  assets?: RustPlaybackAssets;
  ringFrames?: number;
  outputChannels?: number;
}

/** Control-word layout shared with `rust-playback-sink.js` / the worker. */
const CTRL = {
  WRITE: 0,
  READ: 1,
  UNDERRUNS: 2,
  EOS: 3,
  PAUSED: 4,
  IDLE: 5,
  CAP: 6,
  MIN_OCC: 7,
  MAX_OCC: 8,
  GEN: 9,
} as const;

const DEFAULT_RING_FRAMES = 16_384;
const CLOSE_GRACE_MS = 150;
const SEEK_TIMEOUT_MS = 30_000;

export interface RustPlaybackDiagnostics {
  read: number;
  write: number;
  underruns: number;
  eos: number;
  minOcc: number;
  maxOcc: number;
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
    // The transport size, read by the engine only for a `mpak:`
    // container-member source (where it is the container's own length).
    byteSize: item.source.byteSize,
    title: item.title,
    artist: item.artist,
    albumTitle: item.albumTitle,
    codec: item.codec,
  });
}

function defaultWorkerFactory(): PlaybackWorkerLike {
  return new Worker('/rust-playback.worker.js', { type: 'classic' }) as unknown as PlaybackWorkerLike;
}

/**
 * A private AudioContext + PCM sink node. The adapter owns (and closes) this
 * context; it never touches another subsystem's context. Each `attach()`
 * replaces the sink node so a generation's SABs are bound at construction.
 */
async function defaultSinkFactory(): Promise<RustAudioSink> {
  const ctx = new AudioContext();
  const gain = ctx.createGain();
  gain.connect(ctx.destination);
  await ctx.audioWorklet.addModule('/rust-playback-sink.js');
  let node: AudioWorkletNode | null = null;
  return {
    get sampleRate() {
      return ctx.sampleRate;
    },
    attach(control, data, frames, channels) {
      if (node) {
        node.disconnect();
        node = null;
      }
      node = new AudioWorkletNode(ctx, 'rust-playback-sink', {
        numberOfOutputs: 1,
        outputChannelCount: [channels],
        channelCount: channels,
        channelCountMode: 'explicit',
        processorOptions: { control, data, frames, channels },
      });
      node.connect(gain);
    },
    async resume() {
      if (ctx.state !== 'running') await ctx.resume();
    },
    async suspend() {
      if (ctx.state === 'running') await ctx.suspend();
    },
    async close() {
      if (node) {
        node.disconnect();
        node = null;
      }
      await ctx.close();
    },
  };
}

export class RustPlaybackEngine implements Engine, PreloadEngine, CrossfadeEngine, DecodeGate {
  readonly capabilities = RUST_CAPABILITIES;

  private readonly handlers: EngineHandlers;
  private readonly makeWorker: PlaybackWorkerFactory;
  private readonly makeSink: RustAudioSinkFactory;
  private readonly assets?: RustPlaybackAssets;
  private readonly ringFrames: number;
  private readonly channels: number;

  private sink: RustAudioSink | null = null;
  private worker: PlaybackWorkerLike | null = null;
  private controlState: Int32Array | null = null;
  private generation = 0;
  private token: string | null = null;
  /** Frames rendered by sessions retired by a gapless `advance`. Rust resets
   *  the promoted session's playhead, but the player-core contract requires a
   *  session-relative count that stays monotonic across a handoff (matching
   *  the legacy worklet, whose frame count survives a track switch). */
  private renderedBase = 0;
  private rendered = 0;
  private served = 0;
  private drained = false;
  private primed = false;
  private pumping = false;
  private closed = false;
  /** True while the AudioContext clock is running (`play()` .. `pause()`).
   *  Drives the tick pump independently of PCM production so end-of-album and
   *  drained state are still observed after the producer stops. Position is
   *  never derived from this clock — it only schedules `tick` delivery. */
  private outputRunning = false;
  private tickTimer: ReturnType<typeof setInterval> | null = null;

  private listeners = new Map<EngineEventName, Set<(sender: Engine) => void>>();

  private openPending: {
    generation: number;
    resolve: (info: StreamInfo) => void;
    reject: (error: Error) => void;
  } | null = null;
  private seekPending: { generation: number; resolve: () => void } | null = null;
  private preparePending: { generation: number; resolve: (info: StreamInfo | null) => void } | null = null;
  private advancePending: { generation: number; resolve: (info: StreamInfo | null) => void } | null = null;
  private xfade: {
    generation: number;
    resolve: (result: CrossfadeResult | null) => void;
    reject: (error: Error) => void;
  } | null = null;
  private containerPending: {
    generation: number;
    resolve: (album: ContainerAlbumWire) => void;
    reject: (error: Error) => void;
  } | null = null;

  constructor(options: RustPlaybackEngineOptions) {
    this.handlers = options.handlers;
    this.makeWorker = options.workerFactory ?? defaultWorkerFactory;
    this.makeSink = options.sinkFactory ?? defaultSinkFactory;
    this.assets = options.assets;
    this.ringFrames = options.ringFrames ?? DEFAULT_RING_FRAMES;
    this.channels = options.outputChannels ?? 2;
  }

  async init(token: string | null): Promise<void> {
    this.token = token;
    if (!this.sink) this.sink = await this.makeSink();
  }

  on(name: EngineEventName, cb: (sender: Engine) => void): () => void {
    let set = this.listeners.get(name);
    if (!set) this.listeners.set(name, (set = new Set()));
    set.add(cb);
    return () => set!.delete(cb);
  }

  private emit(name: EngineEventName, message?: string): void {
    for (const cb of this.listeners.get(name) ?? []) cb(this);
    const h = this.handlers;
    if (name === 'primed') h.primed();
    else if (name === 'buffering') h.buffering();
    else if (name === 'eos') h.eos();
    else if (name === 'error') h.error(message ?? 'Rust playback error');
    else if (name === 'tick') h.tick();
  }

  // ---- lifecycle -----------------------------------------------------------

  /**
   * Reads an available `.mpak` container's MANF and returns the tracks it
   * defines, as plain data.
   *
   * This is the only way a container enters the client: the container is
   * opened by the same Rust container implementation, over the same
   * synchronous range transport, that playback uses — and the returned tracks
   * carry canonical `mpak:<container>#<member>` sources, so they play through
   * the ordinary queue and engine path with no container-specific branch.
   *
   * It needs no audio context and does not disturb playback: a worker is
   * spawned only if none is live, and the container's transport is registered
   * alongside any existing sources.
   *
   * `size` is the container's length in bytes and is required — the container
   * tail framing is at the end of the file.
   */
  async openContainer(
    container: string,
    size: number,
    kind: PlaybackSource['kind'] = 'http-range',
  ): Promise<ContainerAlbumWire> {
    if (!this.worker) {
      const worker = this.makeWorker();
      worker.onmessage = (event) => this.onWorkerMessage(event.data);
      worker.onerror = (event) => this.onWorkerError(event);
      this.worker = worker;
    }
    const generation = ++this.generation;
    const worker = this.worker;
    return await new Promise<ContainerAlbumWire>((resolve, reject) => {
      this.containerPending = { generation, resolve, reject };
      worker.postMessage({
        type: 'openContainer',
        generation,
        container,
        kind,
        size,
        token: this.token,
        assets: this.assets,
      });
    });
  }

  async open(item: PlaybackItem): Promise<StreamInfo> {
    await this.teardownWorker();
    this.outputRunning = false;
    this.stopTickClock();
    if (!this.sink) this.sink = await this.makeSink();
    const generation = ++this.generation;
    const control = new SharedArrayBuffer(64 * 4);
    const data = new SharedArrayBuffer(this.ringFrames * this.channels * 4);
    const state = new Int32Array(control);
    Atomics.store(state, CTRL.CAP, this.ringFrames);
    Atomics.store(state, CTRL.MIN_OCC, 0x7fffffff);
    Atomics.store(state, CTRL.MAX_OCC, 0);
    Atomics.store(state, CTRL.GEN, generation);
    this.controlState = state;
    this.renderedBase = 0;
    this.rendered = 0;
    this.served = 0;
    this.drained = false;
    this.primed = false;
    this.pumping = false;
    this.closed = false;
    this.sink.attach(control, data, this.ringFrames, this.channels);

    const worker = this.makeWorker();
    worker.onmessage = (event) => this.onWorkerMessage(event.data);
    worker.onerror = (event) => this.onWorkerError(event);
    this.worker = worker;

    return await new Promise<StreamInfo>((resolve, reject) => {
      this.openPending = { generation, resolve, reject };
      worker.postMessage({
        type: 'open',
        generation,
        url: item.source.url,
        kind: item.source.kind,
        size: item.source.byteSize ?? 0,
        token: this.token,
        rate: this.sink!.sampleRate,
        channels: this.channels,
        frames: this.ringFrames,
        control,
        data,
        itemJson: toWasmItem(item),
        assets: this.assets,
      });
    });
  }

  async close(): Promise<void> {
    this.closed = true;
    this.outputRunning = false;
    this.stopTickClock();
    this.settleXfade(null, new Error('engine closed'));
    this.openPending = null;
    this.seekPending = null;
    this.preparePending = null;
    this.advancePending = null;
    await this.teardownWorker();
    if (this.sink) {
      const sink = this.sink;
      this.sink = null;
      await sink.close();
    }
    this.listeners.clear();
  }

  /** Terminates the current producer generation (if any) without closing the
   *  sink; used for replacement and before a fresh `open`. */
  private async teardownWorker(): Promise<void> {
    const worker = this.worker;
    this.worker = null;
    this.openPending?.reject(new Error('engine generation replaced'));
    this.openPending = null;
    this.containerPending?.reject(new Error('engine generation replaced'));
    this.containerPending = null;
    this.seekPending = null;
    this.preparePending = null;
    this.advancePending = null;
    if (!worker) return;
    await this.gracefulClose(worker);
  }

  /** Best-effort graceful worker shutdown: the worker terminates its nested
   *  networkers and acks `closed`; a bounded grace period guards a stuck
   *  worker, after which it is force-terminated. */
  private gracefulClose(worker: PlaybackWorkerLike): Promise<void> {
    return new Promise<void>((resolve) => {
      let settled = false;
      const previous = worker.onmessage;
      const finish = () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        worker.onmessage = previous;
        try {
          worker.terminate();
        } catch {
          /* already gone */
        }
        resolve();
      };
      const timer = setTimeout(finish, CLOSE_GRACE_MS);
      worker.onmessage = (event) => {
        const message = event.data as { type?: string } | null;
        if (message && message.type === 'closed') finish();
        else previous?.({ data: event.data });
      };
      try {
        worker.postMessage({ type: 'close', generation: this.generation });
      } catch {
        finish();
      }
    });
  }

  // ---- transport -----------------------------------------------------------

  async play(): Promise<void> {
    // Rust owns output state; the AudioContext only resumes the clock.
    this.worker?.postMessage({ type: 'play' });
    this.outputRunning = true;
    this.ensureTickClock();
    if (this.sink) await this.sink.resume();
  }

  async pause(): Promise<void> {
    this.worker?.postMessage({ type: 'pause' });
    this.outputRunning = false;
    this.stopTickClock();
    if (this.sink) await this.sink.suspend();
  }

  /** Periodic `tick` delivery while the output clock runs. The worker's
   *  `rendered` messages also tick, but they stop once the producer stops at
   *  the end of the queue; this pump keeps the player's end-of-track
   *  evaluation alive, mirroring the legacy worklet's continuous cadence. */
  private ensureTickClock(): void {
    if (this.tickTimer !== null) return;
    this.tickTimer = setInterval(() => {
      if (this.closed || this.worker === null || !this.outputRunning) return;
      this.emit('tick');
    }, 100);
    (this.tickTimer as unknown as { unref?: () => void }).unref?.();
  }

  private stopTickClock(): void {
    if (this.tickTimer === null) return;
    clearInterval(this.tickTimer as unknown as ReturnType<typeof setInterval>);
    this.tickTimer = null;
  }

  async seekSample(samples: number): Promise<void> {
    const worker = this.worker;
    if (!worker) return;
    const generation = ++this.generation;
    this.renderedBase = 0;
    this.rendered = 0;
    this.drained = false;
    this.primed = false;
    if (this.xfade) this.settleXfade(null, new Error('seek superseded crossfade'));
    await new Promise<void>((resolve) => {
      let settled = false;
      const done = () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve();
      };
      const timer = setTimeout(done, SEEK_TIMEOUT_MS);
      this.seekPending = { generation, resolve: done };
      worker.postMessage({ type: 'seek', generation, samples });
    });
  }

  setGain(linear: number): void {
    this.worker?.postMessage({ type: 'setGain', linear });
  }

  renderedSamples(): number {
    return this.renderedBase + this.rendered;
  }

  /** Output sample rate. player-core reads this via a duck-typed `rate`
   *  accessor (the legacy engines expose it too); without it the core falls
   *  back to 44100, inflating durations/positions by ~8.8% whenever the
   *  AudioContext runs at 48 kHz. */
  get rate(): number {
    return this.sink?.sampleRate ?? 44_100;
  }

  /** True when the decoder is exhausted and the ring has fully drained. */
  isOutputDrained(): boolean {
    return this.drained;
  }

  /** Compressed bytes fetched by the range readers (dev/perf instrumentation). */
  getServedBytes(): number {
    return this.served;
  }

  /** Test/debug-only ring accounting (never used for playback decisions). */
  getDiagnostics(): RustPlaybackDiagnostics {
    const state = this.controlState;
    if (!state) {
      return { read: 0, write: 0, underruns: 0, eos: 0, minOcc: 0, maxOcc: 0 };
    }
    return {
      read: Atomics.load(state, CTRL.READ),
      write: Atomics.load(state, CTRL.WRITE),
      underruns: Atomics.load(state, CTRL.UNDERRUNS),
      eos: Atomics.load(state, CTRL.EOS),
      minOcc: Atomics.load(state, CTRL.MIN_OCC),
      maxOcc: Atomics.load(state, CTRL.MAX_OCC),
    };
  }

  // ---- preload / crossfade -------------------------------------------------

  async prepareNext(item: PlaybackItem): Promise<StreamInfo | null> {
    const worker = this.worker;
    if (!worker) return null;
    const generation = this.generation;
    return await new Promise<StreamInfo | null>((resolve) => {
      this.preparePending = { generation, resolve };
      worker.postMessage({
        type: 'prepareNext',
        generation,
        url: item.source.url,
        kind: item.source.kind,
        size: item.source.byteSize ?? 0,
        token: this.token,
        itemJson: toWasmItem(item),
      });
    });
  }

  async advance(expected: PlaybackItem | null): Promise<StreamInfo | null> {
    const worker = this.worker;
    if (!worker) return null;
    const generation = this.generation;
    return await new Promise<StreamInfo | null>((resolve) => {
      this.advancePending = { generation, resolve };
      worker.postMessage({
        type: 'advance',
        generation,
        expectedJson: expected ? toWasmItem(expected) : null,
      });
    });
  }

  async beginCrossfade(next: PlaybackItem, fadeSeconds: number): Promise<CrossfadeResult | null> {
    const worker = this.worker;
    if (!worker) return null;
    const generation = this.generation;
    return await new Promise<CrossfadeResult | null>((resolve, reject) => {
      this.xfade = { generation, resolve, reject };
      worker.postMessage({
        type: 'beginCrossfade',
        generation,
        url: next.source.url,
        kind: next.source.kind,
        size: next.source.byteSize ?? 0,
        token: this.token,
        itemJson: toWasmItem(next),
        fadeSeconds,
      });
    });
  }

  private settleXfade(result: CrossfadeResult | null, error?: Error): void {
    const pending = this.xfade;
    this.xfade = null;
    if (!pending) return;
    if (error) pending.reject(error);
    else pending.resolve(result);
  }

  // ---- DecodeGate (production player duck-types startPumping/pausePumping) --

  startPumping(): void {
    this.pumping = true;
    this.worker?.postMessage({ type: 'startPumping' });
  }

  pausePumping(): void {
    this.pumping = false;
    this.worker?.postMessage({ type: 'pausePumping' });
  }

  start(): void {
    this.startPumping();
  }

  stop(): void {
    this.pausePumping();
  }

  // ---- worker events -------------------------------------------------------

  private onWorkerMessage(raw: unknown): void {
    const message = raw as Record<string, unknown> | null;
    if (!message || typeof message.type !== 'string') return;
    const generation = typeof message.generation === 'number' ? message.generation : -1;
    // Generation isolation: events from an obsolete generation never affect
    // the current one. `closed` is exempt (it belongs to the teardown).
    if (message.type !== 'closed' && generation !== this.generation) return;

    switch (message.type) {
      case 'opened': {
        const pending = this.openPending;
        this.openPending = null;
        if (pending && pending.generation === generation) {
          pending.resolve(message.info as StreamInfo);
        }
        return;
      }
      case 'containerTracks': {
        const pending = this.containerPending;
        this.containerPending = null;
        if (pending && pending.generation === generation) {
          pending.resolve(message.album as ContainerAlbumWire);
        }
        return;
      }
      case 'seeked': {
        const pending = this.seekPending;
        this.seekPending = null;
        this.renderedBase = 0;
        this.rendered = Number(message.samples) || 0;
        if (pending && pending.generation === generation) pending.resolve();
        return;
      }
      case 'prepared': {
        const pending = this.preparePending;
        this.preparePending = null;
        if (pending && pending.generation === generation) {
          pending.resolve((message.info as StreamInfo | null) ?? null);
        }
        return;
      }
      case 'advanced': {
        const pending = this.advancePending;
        this.advancePending = null;
        const info = (message.info as StreamInfo | null) ?? null;
        // Only a real promotion changes the lane. A null result (end of
        // queue) leaves the current — possibly drained — session in place, so
        // its drained latch and playhead must survive for the end-of-album
        // tick to observe.
        if (info) {
          // Gapless handoff: Rust's promoted session restarts its playhead, so
          // carry the retired session's frames to keep the count continuous.
          this.renderedBase += this.rendered;
          this.rendered = 0;
          this.drained = false;
          this.primed = false;
        }
        if (pending && pending.generation === generation) pending.resolve(info);
        return;
      }
      case 'rendered': {
        this.rendered = Number(message.samples) || 0;
        this.served = Number(message.served) || 0;
        this.emit('tick');
        return;
      }
      case 'primed': {
        this.primed = true;
        this.drained = false;
        this.emit('primed');
        return;
      }
      case 'buffering': {
        this.emit('buffering');
        return;
      }
      case 'eos': {
        this.drained = true;
        this.emit('eos');
        return;
      }
      case 'crossfade': {
        this.onCrossfadeMessage(message, generation);
        return;
      }
      case 'error': {
        const detail = String(message.message ?? 'Rust playback error');
        // A failed container read is the reader's error, not the player's: it
        // rejects that one request and leaves any live session playing.
        const reading = this.containerPending;
        if (reading && reading.generation === generation) {
          this.containerPending = null;
          reading.reject(new Error(detail));
          return;
        }
        // If this generation is still opening, reject the open promise with the
        // real cause (otherwise the caller would only see the teardown reason).
        const pending = this.openPending;
        if (pending && pending.generation === generation) {
          this.openPending = null;
          pending.reject(new Error(detail));
        }
        this.fail(detail);
        return;
      }
      default:
        return;
    }
  }

  private onCrossfadeMessage(message: Record<string, unknown>, generation: number): void {
    const pending = this.xfade;
    if (!pending || pending.generation !== generation) return;
    const status = message.status;
    if (status === 'pending') return;
    this.xfade = null;
    if (status === 'declined') {
      pending.resolve(null);
      return;
    }
    // A taken crossfade continues the playhead inside Rust
    // (`continue_playhead_from`), so the reported frame count stays monotonic;
    // only the latch flags reset.
    this.drained = false;
    this.primed = false;
    pending.resolve((message.result as CrossfadeResult | null) ?? null);
  }

  private onWorkerError(event: unknown): void {
    const message = (event as { message?: string } | null)?.message ?? 'Rust playback worker failed';
    const pending = this.openPending;
    this.openPending = null;
    if (pending) {
      pending.reject(new Error(message));
      return;
    }
    this.fail(message);
  }

  private fail(message: string): void {
    void this.teardownWorker();
    this.drained = false;
    this.emit('error', message);
  }

  // ---- diagnostics helpers -------------------------------------------------

  /** Whether a producer generation is currently open. Test/diagnostic accessor. */
  isOpen(): boolean {
    return this.worker !== null;
  }

  /** Whether `startPumping` has been requested. Test/diagnostic accessor. */
  isPumping(): boolean {
    return this.pumping;
  }

  /** Whether `close()` has been called. Test/diagnostic accessor. */
  isClosed(): boolean {
    return this.closed;
  }
}
