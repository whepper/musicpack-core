// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The Musepack playback engine: owns the AudioContext, the PCM AudioWorklet
// (with its bounded ring), and TWO decoder workers so the next track is
// already opened when the current one ends (gapless). The playback controller
// drives it; the UI never touches it directly.
// The PCM AudioWorklet. In dev Vite serves the TS source transformed; in the
// production build it is bundled to /assets/worklet.js (see vite.config.ts).
const WORKLET_URL = import.meta.env.PROD
  ? '/assets/worklet.js'
  : '/src/lib/playback/audio-worklet.ts';
import { type WorkletReport } from './worklet-protocol';
import type {
  CrossfadeResult,
  Engine,
  EngineCapabilities,
  EngineEvents,
  EngineEventName,
} from '../../../../player-core/src/engine';
import type { PlaybackItem } from '../../../../player-core/src/types';
import { sameItemIdentity } from '../../../../player-core/src/types';

export type EngineStreamInfo = import('../../../../player-core/src/types').StreamInfo & {
  /** Kept as a type alias for web-internal call sites; structurally the
   *  player-core StreamInfo. */
};

/** Constructor handlers (M4): the core EngineEvents naming. */
export interface EngineHandlers {
  primed: () => void;
  buffering: () => void;
  eos: () => void;
  error: (message: string) => void;
  tick: (streamSamples: number) => void;
}

const MUSEPACK_CAPABILITIES: EngineCapabilities = {
  preloadNext: true,
  sampleAccurateGapless: true,
  decodeGate: true,
  // M8 Phase B: overlap-add mixing in the worklet's crossfade lane.
  crossfade: true,
};

interface WorkerHandle {
  worker: Worker;
  info: EngineStreamInfo | null;
  sourceInfo: EngineStreamInfo | null;
  eos: boolean;
  nextUrl: string | null;
  /** The queue item this handle was opened for. Standby/policy agreement:
   *  the core may only promote a standby whose item still matches its
   *  current policy target (see advance()). */
  item: PlaybackItem | null;
  cancelOpen: (() => void) | null;
  /** Crossfade: the lane decode finished before promotion; the engine must
   *  re-emit the core's eos once this handle becomes current. */
  syntheticEosPending?: boolean;
  /** Continuous-frame position of this handle's audible end after a
   *  lane-decoded promotion (`resetBase + outgoingFrames + remaining`).
   *  The raw length no longer applies: the overlap consumed part of it. */
  syntheticEosAt?: number;
}

const CALLBACK_MARGIN_FRAMES = 2048;

const DECODER_WORKER_URL = '/decoder.worker.js';

/** One in-flight crossfade transition, from lane priming through mixing and
 *  swap. All per-attempt state (lane feed queue, credit pacing, resolvers,
 *  timers) lives here so cancellation is a single null-out plus prompt
 *  resolve — no cross-attempt flag can survive a supersede. */
interface XfadeAttempt {
  token: number;
  /** 'priming': filling the lane; 'mixing': xfade-go sent, awaiting xfaded. */
  phase: 'priming' | 'mixing';
  outgoing: WorkerHandle;
  incoming: WorkerHandle;
  fadeFrames: number;
  /** Lane frames required before the mix may start. */
  needed: number;
  filled: number;
  /** Decoded chunks queued for the lane behind the outstanding credit. */
  q: Float32Array[];
  awaitingAccepted: boolean;
  decodeDone: boolean;
  endSent: boolean;
  readyResolve: ((ok: boolean) => void) | null;
  readyTimer: ReturnType<typeof setTimeout> | null;
  swapResolve: ((ok: boolean) => void) | null;
  swapTimer: ReturnType<typeof setTimeout> | null;
  /** Frame accounting reported by the worklet's 'xfaded' message. */
  swapFacts: {
    outgoingFrames: number;
    incomingFrames: number;
    swapBaseFrames: number;
    overlapFrames: number;
  } | null;
}

export class MusepackEngine implements Engine {
  readonly kind = 'musepack' as const;
  readonly capabilities = MUSEPACK_CAPABILITIES;
  private ctx: AudioContext | null = null;
  private gain: GainNode | null = null;
  private node: AudioWorkletNode | null = null;
  private current: WorkerHandle | null = null;
  private standby: WorkerHandle | null = null;
  private h: EngineHandlers;
  private streamSamples = 0;
  /** Stream position at the last ring reset (seek/open); rendered is added. */
  private resetBase = 0;
  private token: string | null = null;
  private servedBytes = 0;
  private ready: Promise<void> | null = null;
  private pumpingRequested = false;
  private backpressured = false;
  private pullInFlight = false;
  private generation = 0;
  private seekResolve: (() => void) | null = null;
  private transitioning = false;
  /** Decoder done + output dry: the current track's audible end (used by
   *  the core as a drain-complete signal after crossfade clock shifts). */
  // Crossfade state (M8 Phase B). One attempt object owns everything about
  // an in-flight transition; eos suppression is IDENTITY-scoped (the
  // outgoing handle inside the live attempt), so a completed, cancelled or
  // superseded fade can never leak suppression into later tracks.
  private xfadeToken = 0;
  private xfade: XfadeAttempt | null = null;
  private outputDrained = false;
  private standbyRequest = 0;
  private trackEndResolve: (() => void) | null = null;
  private contextShouldRun = false;

  constructor(h: EngineHandlers) {
    this.h = h;
  }

  /** Port-facing event registration (M2). Listeners fire IN ADDITION to the
   *  legacy constructor callbacks (which the web controller still uses until
   *  M4); either side may be absent. */
  private listeners = new Map<EngineEventName, Set<(sender: Engine) => void>>();
  on(name: EngineEventName, cb: (sender: Engine) => void): () => void {
    let set = this.listeners.get(name);
    if (!set) this.listeners.set(name, (set = new Set()));
    set.add(cb);
    return () => set!.delete(cb);
  }
  private emitNamed(name: EngineEventName): void {
    for (const cb of this.listeners.get(name) ?? []) cb(this);
  }

  /** Lazily creates the AudioContext + worklet. Must run after a user gesture. */
  private async ensureContext(): Promise<void> {
    if (this.ctx) return;
    this.ctx = new AudioContext();
    this.gain = this.ctx.createGain();
    this.gain.connect(this.ctx.destination);
    await this.ctx.audioWorklet.addModule(WORKLET_URL);
    this.node = new AudioWorkletNode(this.ctx, 'musicpack-pcm', {
      numberOfOutputs: 1,
      outputChannelCount: [2],
      channelCount: 2,
      channelCountMode: 'explicit',
    });
    this.node.connect(this.gain);
    // A crashed processor stops receiving process() calls entirely — audio
    // goes silent and every counter freezes. Surface it as a player error
    // instead of an unexplainable silent hang.
    this.node.onprocessorerror = () => {
      const message = 'Audio worklet processor crashed.';
      this.h.error(message);
      this.emitNamed('error');
    };
    this.node.port.onmessage = (ev: MessageEvent<WorkletReport>) => {
      this.onWorkletMessage(ev.data);
    };
  }

  async init(token: string | null): Promise<void> {
    this.token = token;
    await this.ensureContext();
  }

  private makeWorker(): WorkerHandle {
    const worker = new Worker(DECODER_WORKER_URL, { type: 'classic' });
    const h: WorkerHandle = {
      worker,
      info: null,
      sourceInfo: null,
      eos: false,
      nextUrl: null,
      item: null,
      cancelOpen: null,
    };
    worker.onmessage = (ev: MessageEvent) => {
      this.onWorkerMessage(h, ev.data);
    };
    worker.onerror = (ev: ErrorEvent) => {
      const detail = ev.message ? ` (${ev.message})` : '';
      this.h.error(`Decoder worker failed${detail}.`);
      this.emitNamed('error');
      void ev;
    };
    return h;
  }

  private onWorkerMessage(h: WorkerHandle, msg: Record<string, unknown>): void {
    const messageGeneration = msg.generation;
    const generationScoped =
      msg.type === 'pcm' ||
      msg.type === 'seeked' ||
      msg.type === 'stats' ||
      msg.type === 'eos' ||
      msg.type === 'error';
    if (generationScoped && messageGeneration !== this.generation) {
      return;
    }
    switch (msg.type) {
      case 'info':
        h.sourceInfo = {
          rate: msg.rate as number,
          channels: msg.channels as number,
          version: msg.version as number,
          lengthSamples: msg.lengthSamples as number,
        };
        h.info = this.normalizedInfo(h.sourceInfo);
        break;
      case 'pcm':
        if (h === this.current) {
          const samples = msg.samples as Float32Array;
          this.node?.port.postMessage(
            { type: 'samples', buffer: samples.buffer, generation: this.generation },
            [samples.buffer as ArrayBuffer],
          );
        } else if (h === this.standby && this.xfade?.incoming === h) {
          // Crossfade lane: queue the standby's decode and forward it under
          // one-outstanding-credit pacing so the lane ring never overflows.
          this.xfade.q.push(msg.samples as Float32Array);
          this.pumpXlaneQueue();
        }
        break;
      case 'seeked': {
        this.transitioning = false;
        const resolve = this.seekResolve;
        this.seekResolve = null;
        resolve?.();
        break;
      }
      case 'stats':
        this.servedBytes = msg.served as number;
        break;
      case 'eos':
        this.pullInFlight = false;
        if (!h.eos) {
          h.eos = true;
          const xfade = this.xfade;
          if (xfade && xfade.incoming === h) {
            // The lane decode finished: once the final credit returns, tell
            // the worklet to flush the resampler tail (xend).
            xfade.decodeDone = true;
            this.sendXlaneEndIfDrained();
          }
          if (h === this.current) {
            if (h.syntheticEosPending) {
              // The incoming track finished decoding inside the crossfade
              // lane BEFORE promotion; its one-shot eos is spent. The core
              // still needs an eos at the audible end so the normal
              // boundary chain keeps working — re-emit it now.
              h.syntheticEosPending = false;
              this.h.eos();
              this.emitNamed('eos');
            } else if (xfade && xfade.phase === 'mixing' && xfade.outgoing === h) {
              // The outgoing track is draining under the ACTIVE fade: the
              // core already advanced its cursor at trigger time. Swallow.
              // Identity-scoped to this attempt — a completed, cancelled or
              // superseded fade leaves nothing behind that could swallow a
              // later track's legitimate eos.
            } else {
              this.h.eos();
              this.emitNamed('eos');
            }
          }
        }
        break;
      case 'error':
        this.pullInFlight = false;
        this.h.error((msg.message as string) ?? 'Decoder error.');
        this.emitNamed('error');
        break;
    }
  }

  private onWorkletMessage(msg: WorkletReport): void {
    if (msg.generation !== this.generation) return;
    switch (msg.type) {
      case 'rendered':
        this.streamSamples = this.resetBase + msg.frames;
        // A crossfade-promoted track whose decode finished inside the lane
        // has a spent one-shot worker eos; re-emit the core's eos at its
        // COMPRESSED audible end (swap point + remaining length), which is
        // not the raw track length because the overlap consumed part of it.
        const cur = this.current;
        if (
          cur?.syntheticEosPending &&
          cur.syntheticEosAt !== undefined &&
          this.streamSamples >= cur.syntheticEosAt
        ) {
          cur.syntheticEosPending = false;
          this.h.eos();
          this.emitNamed('eos');
        }
        this.h.tick(this.streamSamples);
        this.emitNamed('tick');
        break;
      case 'primed':
        this.outputDrained = false;
        this.h.primed();
        this.emitNamed('primed');
        break;
      case 'need':
        this.outputDrained = false;
        this.backpressured = false;
        this.resumePumpingIfRequested();
        break;
      case 'full':
        this.backpressured = true;
        this.current?.worker.postMessage({ type: 'pause', generation: this.generation });
        break;
      case 'underrun':
        // Decoder exhausted + output dry = the track's audible end. The core
        // uses this (via isOutputDrained) as an alternative drain-complete
        // signal, because a crossfade compresses the album clock and the
        // positional threshold may never be reached.
        if (this.current?.eos) this.outputDrained = true;
        this.h.buffering();
        this.emitNamed('buffering');
        this.backpressured = false;
        // A starved output also proves any outstanding decode request is
        // lost (its response can no longer arrive meaningfully). Release
        // the credit so pumping can actually resume — otherwise the ring
        // stays empty forever and playback freezes silently.
        this.pullInFlight = false;
        this.resumePumpingIfRequested();
        break;
      case 'accepted':
        if (msg.lane === 2) {
          this.onXlaneAccepted(msg.available);
        } else {
          this.pullInFlight = false;
          this.resumePumpingIfRequested();
        }
        break;
      case 'trackEnded': {
        const resolve = this.trackEndResolve;
        this.trackEndResolve = null;
        resolve?.();
        break;
      }
      case 'xfadeReady': {
        // Whole incoming track queued and flushed: readiness is guaranteed.
        const xfade = this.xfade;
        if (!xfade || msg.token !== xfade.token) break;
        xfade.incoming.worker.postMessage({
          type: 'pause',
          generation: this.generation,
        });
        const resolve = xfade.readyResolve;
        if (resolve) {
          xfade.readyResolve = null;
          if (xfade.readyTimer) clearTimeout(xfade.readyTimer);
          resolve(true);
        }
        break;
      }
      case 'xfaded': {
        // The worklet swapped the rings mid-mix. Record the exact frame
        // accounting and let the awaiting attempt finish its bookkeeping.
        const xfade = this.xfade;
        if (!xfade || msg.token !== xfade.token || xfade.phase !== 'mixing') break;
        xfade.swapFacts = {
          outgoingFrames: msg.outgoingFrames ?? 0,
          incomingFrames: msg.incomingFrames ?? 0,
          // Fall back to the legacy (uncompressed) accounting when talking
          // to an older worklet build that hasn't sent these yet.
          swapBaseFrames: msg.swapBaseFrames ?? msg.outgoingFrames ?? 0,
          overlapFrames: msg.overlapFrames ?? msg.incomingFrames ?? 0,
        };
        const resolve = xfade.swapResolve;
        if (resolve) {
          xfade.swapResolve = null;
          if (xfade.swapTimer) clearTimeout(xfade.swapTimer);
          resolve(true);
        }
        break;
      }
      case 'error':
        this.pullInFlight = false;
        this.h.error(msg.message ?? 'Audio worklet protocol error.');
        this.emitNamed('error');
        break;
    }
  }

  /** Port signature (M4): open by resolved item. Local-file items open the
   *  OPFS-backed reader inside the worker; remote ones the HTTP demand
   *  reader. The decode path itself is byte-source agnostic. */
  async open(item: PlaybackItem): Promise<EngineStreamInfo> {
    if (item.source.kind === 'local-file') {
      return this.openLocalSource(item.source.url, item.source.byteSize ?? -1);
    }
    return this.openSource(item.source.url, item.source.byteSize ?? -1);
  }

  async openSource(url: string, size: number): Promise<EngineStreamInfo> {
    return this.openInSlot({ url, size });
  }

  /** Offline variant: `key` addresses a committed OPFS audio file. */
  async openLocalSource(key: string, size: number): Promise<EngineStreamInfo> {
    return this.openInSlot({ localKey: key, size });
  }

  /** Opens one track into the current slot from any byte source. */
  private async openInSlot(src: { url?: string; localKey?: string; size: number }): Promise<EngineStreamInfo> {
    const url = src.url ?? '';
    await this.ensureContext();
    const generation = ++this.generation;
    // Any in-flight crossfade is dead: the lane, its worker handles and its
    // resolvers must not outlive the open (stale suppression or a late
    // promotion would corrupt the freshly opened track).
    this.cancelXfadeAttempt();
    this.pumpingRequested = false;
    this.transitioning = true;
    this.pullInFlight = false;
    this.seekResolve?.();
    this.seekResolve = null;
    this.trackEndResolve?.();
    this.trackEndResolve = null;
    this.node?.port.postMessage({ type: 'reset', generation });
    ++this.standbyRequest;
    await this.closeCurrent();
    await this.closeStandby();
    if (generation !== this.generation) throw new Error('Playback open was superseded.');
    const h = this.makeWorker();
    h.nextUrl = url;
    this.current = h;
    this.backpressured = false;
    this.resetBase = 0;
    this.streamSamples = 0;
    this.outputDrained = false;
    try {
      const info = await this.openInWorker(h, url, src.size, generation, src.localKey ?? null);
      if (generation !== this.generation || this.current !== h) {
        if (this.current === h) this.current = null;
        await this.closeWorker(h);
        throw new Error('Playback open was superseded.');
      }
      this.configureWorklet(h.sourceInfo ?? info, generation);
      this.transitioning = false;
      return info;
    } catch (error) {
      if (this.current === h) {
        this.current = null;
        await this.closeWorker(h);
      }
      throw error;
    }
  }

  /** Port signature (M4): standby-open by resolved item. The item rides on
   *  the handle for standby/policy agreement at promotion time. */
  async prepareNext(item: PlaybackItem): Promise<EngineStreamInfo | null> {
    if (item.source.kind === 'local-file') {
      return this.prepareNextLocalSource(item.source.url, item.source.byteSize ?? -1, item);
    }
    return this.prepareNextSource(item.source.url, item.source.byteSize ?? -1, item);
  }

  /** Opens the next track in the standby slot, ahead of the current one.
   *  `item` is optional for web-internal legacy callers; only item-carrying
   *  standbys are promotable through advance(expected). */
  async prepareNextSource(url: string, size: number, item?: PlaybackItem): Promise<EngineStreamInfo | null> {
    return this.prepareNextInSlot({ url, size }, item);
  }

  /** Offline variant of prepareNextSource. */
  async prepareNextLocalSource(key: string, size: number, item?: PlaybackItem): Promise<EngineStreamInfo | null> {
    return this.prepareNextInSlot({ localKey: key, size }, item);
  }

  private async prepareNextInSlot(src: { url?: string; localKey?: string; size: number }, item?: PlaybackItem): Promise<EngineStreamInfo | null> {
    const url = src.url ?? '';
    const request = ++this.standbyRequest;
    const previous = this.standby;
    if (previous) this.standby = null;
    if (previous) await this.closeWorker(previous);
    if (request !== this.standbyRequest) return null;
    const h = this.makeWorker();
    h.nextUrl = url;
    h.item = item ?? null;
    this.standby = h;
    try {
      const info = await this.openInWorker(h, url, src.size, this.generation, src.localKey ?? null);
      if (request !== this.standbyRequest || this.standby !== h) {
        if (this.standby === h) {
          this.standby = null;
          await this.closeWorker(h);
        }
        return null;
      }
      return info;
    } catch {
      if (this.standby === h) {
        this.standby = null;
        await this.closeWorker(h);
      }
      return null;
    }
  }

  private async openInWorker(
    h: WorkerHandle,
    url: string,
    size: number,
    generation: number,
    localKey: string | null = null,
  ): Promise<EngineStreamInfo> {
    const infoPromise = new Promise<EngineStreamInfo>((resolve, reject) => {
      const orig = h.worker.onmessage;
      const finish = () => {
        h.cancelOpen = null;
        h.worker.onmessage = orig;
      };
      h.cancelOpen = () => {
        finish();
        reject(new Error('Playback open was superseded.'));
      };
      h.worker.onmessage = (ev: MessageEvent) => {
        const msg = ev.data;
        if (msg.type === 'info') {
          h.sourceInfo = {
            rate: msg.rate,
            channels: msg.channels,
            version: msg.version,
            lengthSamples: msg.lengthSamples,
          };
          if (h.sourceInfo.channels < 1 || h.sourceInfo.channels > 2) {
            finish();
            reject(new Error(`Unsupported Musepack channel count: ${h.sourceInfo.channels}`));
            return;
          }
          h.info = this.normalizedInfo(h.sourceInfo);
          finish();
          resolve(h.info);
        } else if (msg.type === 'error') {
          finish();
          reject(new Error(msg.message));
        }
      };
    });
    h.worker.postMessage({ type: 'open', url, size, token: this.token, generation, localKey });
    return infoPromise;
  }

  private configureWorklet(sourceInfo: EngineStreamInfo, generation: number): void {
    this.node?.port.postMessage({
      type: 'config',
      sourceRate: sourceInfo.rate,
      sourceChannels: sourceInfo.channels,
      outputRate: this.outputRate,
      outputChannels: 2,
      generation,
    });
    this.node?.port.postMessage({ type: 'reset', generation });
    this.streamSamples = 0;
  }

  /** Promotes the standby worker to current (gapless handoff at EOS).
   *  Standby/policy agreement: `expected` is the item the core's CURRENT
   *  queue policy selected; `null` means nothing may follow (flush-only is
   *  NOT needed for end detection — the ring drains into the END_TOLERANCE
   *  window on its own). Validation runs BEFORE any flush so a refused
   *  standby can neither bleed into output nor disturb resampler state;
   *  the caller recovers by loading `expected` fresh. */
  async advance(expected: PlaybackItem | null): Promise<EngineStreamInfo | null> {
    const generation = this.generation;
    const standby = this.standby;
    if (
      !expected ||
      !standby ||
      !standby.info ||
      !standby.item ||
      !sameItemIdentity(standby.item, expected)
    ) {
      return null;
    }
    await this.finishTrack(generation);
    if (generation !== this.generation) return null;
    if (!standby || !standby.info) return null;
    const promotedSourceInfo = standby.sourceInfo;
    if (!promotedSourceInfo) return null;
    ++this.standbyRequest;
    const promoted = standby;
    this.standby = null;
    const previous = this.current;
    if (previous) this.current = null;
    if (previous) await this.closeWorker(previous);
    if (generation !== this.generation || this.current !== null) {
      await this.closeWorker(promoted);
      return null;
    }
    this.current = promoted;
    // Changing the current handle invalidates any outstanding demand credit
    // (same invariant as the crossfade promotion above).
    this.pullInFlight = false;
    this.node?.port.postMessage({
      type: 'track',
      sourceRate: promotedSourceInfo.rate,
      sourceChannels: promotedSourceInfo.channels,
      generation,
    });
    return promoted.info;
  }

  private finishTrack(generation: number): Promise<void> {
    if (!this.node) return Promise.resolve();
    this.trackEndResolve?.();
    return new Promise((resolve) => {
      this.trackEndResolve = resolve;
      this.node?.port.postMessage({ type: 'end', generation });
    });
  }

  /**
   * Crossfade (M8 Phase B): overlap-add the standby track over the current
   * one inside the worklet. The standby worker is pumped into the worklet's
   * crossfade lane under its own credit loop; once the lane holds at least
   * the fade window of audio, mixing starts at the end of the outgoing
   * track. Resolves with the incoming track's info and the exact overlap
   * AFTER the worklet swap, with the standby promoted exactly like
   * advance() — or null (caller falls back to normal EOS) when there is no
   * standby/context, the decode fails, another attempt is already running,
   * or a competing transport action superseded the attempt. Every exit path
   * funnels through cancelXfadeAttempt(), so a superseded fade leaves no
   * state behind.
   */
  async beginCrossfade(
    next: PlaybackItem,
    fadeSeconds: number,
  ): Promise<CrossfadeResult | null> {
    if (this.xfade) return null; // one attempt at a time; refuse, don't queue
    const generation = this.generation;
    const outgoing = this.current;
    const incoming = this.standby;
    if (!this.node || !outgoing || !incoming || !incoming.info || !incoming.sourceInfo) {
      return null;
    }
    // Standby/policy agreement: the caller's `next` is its current policy
    // target. A standby prepared for a different item must never be faded
    // in — refuse and let the normal EOS path recover.
    if (!incoming.item || !sameItemIdentity(incoming.item, next)) return null;
    const clamped = Math.max(0.25, Math.min(15, fadeSeconds));
    const attempt: XfadeAttempt = {
      token: ++this.xfadeToken,
      phase: 'priming',
      outgoing,
      incoming,
      fadeFrames: Math.max(1, Math.round(clamped * this.outputRate)),
      needed: 0,
      filled: 0,
      q: [],
      awaitingAccepted: false,
      decodeDone: false,
      endSent: false,
      readyResolve: null,
      readyTimer: null,
      swapResolve: null,
      swapTimer: null,
      swapFacts: null,
    };
    this.xfade = attempt;
    // A standby slot is opened but never pumped before a fade, so a stale
    // eos flag should not exist — clear it defensively so the lane's own
    // decode lifecycle owns the flag from here.
    incoming.eos = false;

    // Arm the worklet lane for the incoming format.
    this.node.port.postMessage({
      type: 'xfade',
      sourceRate: incoming.sourceInfo.rate,
      sourceChannels: incoming.sourceInfo.channels,
      fadeFrames: attempt.fadeFrames,
      token: attempt.token,
      generation,
    });

    try {
      // Route the standby's decode into the lane. Its decode-eos must NOT
      // reach the core: the incoming track is only beginning.
      const ok = await this.pumpLaneUntilReady(attempt, generation);
      if (!ok || generation !== this.generation || this.standby !== incoming) {
        this.cancelXfadeAttempt();
        return null;
      }

      // Start mixing; the render callback owns the transition timing from
      // here. The outgoing worker may still be decoding — its late eos is
      // swallowed by the identity-scoped rule while THIS attempt lives.
      attempt.phase = 'mixing';
      this.node.port.postMessage({ type: 'xfade-go', token: attempt.token, generation });

      // Wait for the swap report (real-time: the fade window elapses in the
      // audio callback). Cancellation resolves false promptly.
      const swapped = await this.waitForSwap(attempt);
      if (
        !swapped ||
        generation !== this.generation ||
        this.standby !== incoming ||
        this.xfade !== attempt ||
        !attempt.swapFacts
      ) {
        this.cancelXfadeAttempt();
        return null;
      }

      // Success: capture the accounting, hand the lane leftovers to the
      // normal decode path, promote exactly like advance().
      const facts = attempt.swapFacts;
      this.xfade = null;
      ++this.standbyRequest;
      this.standby = null;
      this.current = incoming;
      // Promotion invalidates the outgoing lane's outstanding demand credit:
      // its worker is about to be closed, so a held credit would block every
      // future resumePumping for the new current track.
      this.pullInFlight = false;
      attempt.awaitingAccepted = false;
      for (const samples of attempt.q) {
        this.node.port.postMessage(
          { type: 'samples', buffer: samples.buffer, generation: this.generation },
          [samples.buffer as ArrayBuffer],
        );
      }
      // If the incoming decode already completed inside the lane, its
      // one-shot worker eos is spent: synthesize the core's eos at the
      // COMPRESSED audible end — swap point plus what is left of the track
      // after the overlap consumed its head.
      if (incoming.eos && incoming.info) {
        incoming.eos = false;
        incoming.syntheticEosPending = true;
        // Expressed from the swap's BOUNDARY (swapBaseFrames), not the
        // outgoing ring's raw uncompressed swap-time count — the promoted
        // ring's own playhead was rebased to that same boundary in
        // completeXfadeSwap(), so the two stay in the same frame space.
        incoming.syntheticEosAt =
          this.resetBase + facts.swapBaseFrames + Math.max(0, incoming.info.lengthSamples - facts.incomingFrames);
      }
      await this.closeWorker(outgoing);
      // overlapFrames is the TRUE overlap the outgoing ring actually
      // supplied (bounded by the fade window, shorter when it ran dry) —
      // not incomingFrames, which is ~always just the nominal fade window
      // regardless of how much of the outgoing track it really covered.
      return { info: incoming.info, overlapFrames: facts.overlapFrames };
    } catch {
      this.cancelXfadeAttempt();
      return null;
    }
  }

  /** Tears down any in-flight crossfade attempt (competing transport won,
   *  readiness failure, or post-swap supersede). Resolves both pending
   *  waits promptly so callers never hang on a dead transition. */
  private cancelXfadeAttempt(): void {
    const xfade = this.xfade;
    if (!xfade) return;
    this.xfade = null;
    if (xfade.readyTimer) clearTimeout(xfade.readyTimer);
    if (xfade.swapTimer) clearTimeout(xfade.swapTimer);
    const ready = xfade.readyResolve;
    xfade.readyResolve = null;
    ready?.(false);
    const swap = xfade.swapResolve;
    xfade.swapResolve = null;
    swap?.(false);
    this.node?.port.postMessage({ type: 'xfade-cancel', generation: this.generation });
  }

  /** Demand-paces the standby worker into the crossfade lane until the lane
   *  holds enough audio to start the mix: the whole fade window plus
   *  callback margin (the lane ring is sized by laneCapacityFrames to hold
   *  exactly that, so even 12 s fades prime fully instead of being fed in
   *  real time), or the whole track when it is shorter. Resolves false on
   *  cancellation/supersede/error. */
  private pumpLaneUntilReady(xfade: XfadeAttempt, generation: number): Promise<boolean> {
    xfade.needed = Math.min(
      xfade.incoming.info?.lengthSamples ?? Number.POSITIVE_INFINITY,
      xfade.fadeFrames + CALLBACK_MARGIN_FRAMES,
    );
    xfade.filled = 0;
    xfade.decodeDone = false;
    xfade.endSent = false;
    xfade.awaitingAccepted = false;
    xfade.q = [];
    return new Promise<boolean>((resolve) => {
      xfade.readyResolve = resolve;
      // Never hang the fallback forever (e.g. suspended context).
      xfade.readyTimer = setTimeout(() => {
        if (this.xfade === xfade && xfade.readyResolve === resolve) {
          xfade.readyResolve = null;
          resolve(false);
        }
      }, 30000);
      xfade.incoming.worker.postMessage({ type: 'play', generation });
    });
  }

  /** Waits for the worklet's swap report; resolves false via cancellation
   *  or the safety timeout. */
  private waitForSwap(xfade: XfadeAttempt): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      xfade.swapResolve = resolve;
      xfade.swapTimer = setTimeout(() => {
        if (this.xfade === xfade && xfade.swapResolve === resolve) {
          xfade.swapResolve = null;
          resolve(false);
        }
      }, 60000);
    });
  }

  /** Forwards one queued lane chunk when the previous credit returned. */
  private pumpXlaneQueue(): void {
    const xfade = this.xfade;
    if (!xfade || xfade.awaitingAccepted || xfade.q.length === 0 || !this.node) return;
    const samples = xfade.q.shift()!;
    xfade.awaitingAccepted = true;
    this.node.port.postMessage(
      {
        type: 'xsamples',
        buffer: samples.buffer,
        token: xfade.token,
        generation: this.generation,
      },
      [samples.buffer as ArrayBuffer],
    );
  }

  /** Lane bookkeeping after each accepted crossfade credit. */
  private onXlaneAccepted(available: number | undefined): void {
    const xfade = this.xfade;
    if (!xfade) return;
    xfade.awaitingAccepted = false;
    if (available !== undefined) xfade.filled = available;
    this.sendXlaneEndIfDrained();
    this.pumpXlaneQueue();
    // The decoder worker is pull-based (one chunk per 'play'): ask for the
    // next lane chunk until its decode is done.
    if (!xfade.decodeDone && !xfade.awaitingAccepted && xfade.q.length === 0) {
      xfade.incoming.worker.postMessage({ type: 'play', generation: this.generation });
    }
    this.settleXlaneReadiness();
  }

  /** Sends xend once the lane decode is complete and every queued chunk
   *  has been credited; the worklet answers with xfadeReady. */
  private sendXlaneEndIfDrained(): void {
    const xfade = this.xfade;
    if (!xfade) return;
    if (!xfade.decodeDone || xfade.awaitingAccepted || xfade.q.length > 0) return;
    if (xfade.endSent) return;
    xfade.endSent = true;
    this.node?.port.postMessage({
      type: 'xend',
      token: xfade.token,
      generation: this.generation,
    });
  }

  /** Readiness = the lane holds the needed prime audio, or the whole track
   *  is decoded, credited, and flushed (xfadeReady arrives separately). */
  private settleXlaneReadiness(): void {
    const xfade = this.xfade;
    if (!xfade || !xfade.readyResolve) return;
    if (
      xfade.filled >= xfade.needed ||
      (xfade.decodeDone && !xfade.awaitingAccepted && xfade.q.length === 0)
    ) {
      const resolve = xfade.readyResolve;
      xfade.readyResolve = null;
      if (xfade.readyTimer) clearTimeout(xfade.readyTimer);
      // Pause the standby decode while waiting for the go moment.
      xfade.incoming.worker.postMessage({ type: 'pause', generation: this.generation });
      resolve(true);
    }
  }

  startPumping(): void {
    this.pumpingRequested = true;
    this.resumePumpingIfRequested();
  }

  pausePumping(): void {
    this.pumpingRequested = false;
    this.current?.worker.postMessage({ type: 'pause', generation: this.generation });
  }

  private resumePumpingIfRequested(): void {
    if (
      this.pumpingRequested &&
      !this.backpressured &&
      !this.pullInFlight &&
      !this.transitioning &&
      this.current
    ) {
      this.pullInFlight = true;
      this.current.worker.postMessage({ type: 'play', generation: this.generation });
    }
  }

  /** Starts the audio clock (call after a user gesture or after priming). */
  async play(): Promise<void> {
    const ctx = this.ctx;
    if (!ctx) return;
    this.contextShouldRun = true;
    if (ctx.state !== 'running') await ctx.resume();
    if (!this.contextShouldRun && ctx.state === 'running') await ctx.suspend();
  }

  async pause(): Promise<void> {
    this.contextShouldRun = false;
    if (this.ctx && this.ctx.state !== 'closed') await this.ctx.suspend();
  }

  /** Port signature (M4). */
  async seekSample(samples: number): Promise<void> {
    return this.seek(samples);
  }

  async seek(sample: number): Promise<void> {
    if (!this.current) return;
    // A seek kills any in-flight fade: the lane is dropped (reset below),
    // and the attempt's resolvers fire so nothing promotes a stale standby.
    this.cancelXfadeAttempt();
    const generation = ++this.generation;
    this.transitioning = true;
    this.backpressured = false;
    this.pullInFlight = false;
    // A seek repositions the clock: a pending crossfade synthetic eos must
    // not fire from stale frame accounting.
    this.current.syntheticEosPending = false;
    this.seekResolve?.();
    this.seekResolve = null;
    this.trackEndResolve?.();
    this.trackEndResolve = null;
    this.current.eos = false;
    this.node?.port.postMessage({ type: 'reset', generation });
    this.resetBase = sample;
    this.streamSamples = sample;
    this.outputDrained = false;
    const sourceSample = this.outputToSourceSample(sample, this.current.sourceInfo);
    await this.postSeek(sourceSample, generation);
    if (generation !== this.generation) return;
    if (this.pumpingRequested) this.startPumping();
  }

  private postSeek(sample: number, generation: number): Promise<void> {
    return new Promise((resolve) => {
      const h = this.current;
      if (!h) return resolve();
      this.seekResolve = resolve;
      h.worker.postMessage({ type: 'seek', sample, generation });
    });
  }

  setGain(linear: number): void {
    if (this.gain) this.gain.gain.value = linear;
  }

  getPositionSamples(): number {
    return this.streamSamples;
  }

  /** AudioContext-rate frames rendered since the last reset (port name). */
  renderedSamples(): number {
    return this.streamSamples - this.resetBase;
  }

  /** Legacy alias (tests/web call sites until M4 cleanup). */
  getRenderedSamples(): number {
    return this.renderedSamples();
  }

  getServedBytes(): number {
    return this.servedBytes;
  }

  getInfo(): EngineStreamInfo | null {
    return this.current?.info ?? null;
  }

  get lengthSamples(): number {
    return this.current?.info?.lengthSamples ?? 0;
  }

  get rate(): number {
    return this.current?.info?.rate ?? 44100;
  }

  private get outputRate(): number {
    return this.ctx?.sampleRate ?? 44100;
  }

  private normalizedInfo(sourceInfo: EngineStreamInfo): EngineStreamInfo {
    return {
      rate: this.outputRate,
      channels: 2,
      version: sourceInfo.version,
      lengthSamples: Math.ceil((sourceInfo.lengthSamples * this.outputRate) / sourceInfo.rate),
    };
  }

  private outputToSourceSample(sample: number, sourceInfo: EngineStreamInfo | null): number {
    if (!sourceInfo) return sample;
    return Math.min(
      sourceInfo.lengthSamples,
      Math.max(0, Math.floor((sample * sourceInfo.rate) / this.outputRate)),
    );
  }

  /** True when the decoder is exhausted and the output ring has run dry —
   *  the current track has no more audible content. */
  isOutputDrained(): boolean {
    return this.outputDrained;
  }

  /** Terminates a decoder worker, but only after its nested demand-reader/
   *  network worker has been closed (the decoder acks `closed` after running
   *  its teardown). A bounded timeout guards against a stuck worker.
   *  Terminating the outer worker first would orphan the nested networker. */
  private closeWorker(h: WorkerHandle): Promise<void> {
    return new Promise((resolve) => {
      const worker = h.worker;
      h.cancelOpen?.();
      const orig = worker.onmessage;
      const done = () => {
        clearTimeout(timer);
        worker.onmessage = orig;
        worker.terminate();
        resolve();
      };
      const timer = setTimeout(() => {
        worker.onmessage = orig;
        worker.terminate();
        resolve();
      }, 500);
      worker.onmessage = (ev: MessageEvent) => {
        const msg = ev.data;
        if (msg.type === 'closed') done();
        else this.onWorkerMessage(h, msg);
      };
      worker.postMessage({ type: 'close' });
    });
  }

  private async closeCurrent(): Promise<void> {
    if (this.current) {
      const h = this.current;
      this.current = null;
      await this.closeWorker(h);
    }
  }

  private async closeStandby(): Promise<void> {
    if (this.standby) {
      const h = this.standby;
      this.standby = null;
      await this.closeWorker(h);
    }
  }

  async close(): Promise<void> {
    this.contextShouldRun = false;
    this.pumpingRequested = false;
    this.backpressured = false;
    this.pullInFlight = false;
    this.transitioning = false;
    this.cancelXfadeAttempt();
    this.seekResolve?.();
    this.seekResolve = null;
    this.trackEndResolve?.();
    this.trackEndResolve = null;
    ++this.standbyRequest;
    await this.closeCurrent();
    await this.closeStandby();
    if (this.ctx) {
      await this.ctx.close();
      this.ctx = null;
      this.gain = null;
      this.node = null;
    }
  }
}
