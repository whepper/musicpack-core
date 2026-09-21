// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Native-codec backend: plays browser-supported formats (FLAC, Vorbis, WAV,
// ...) through HTMLMediaElement routed into the shared gain pipeline.
//
// Gapless: the next track is preloaded into a second element ahead of time
// and swapped in on 'ended'. Browsers do not expose a sample-perfect gapless
// API for <audio>, so the boundary may carry a small gap — unlike the
// Musepack backend, which is exact. This is documented in web/README.md.
import type { EngineStreamInfo } from './musepack-engine';
import type {
  CrossfadeResult,
  Engine,
  EngineCapabilities,
  EngineEventName,
} from '../../../../player-core/src/engine';
import type { PlaybackItem } from '../../../../player-core/src/types';
import { sameItemIdentity } from '../../../../player-core/src/types';

const NATIVE_CAPABILITIES: EngineCapabilities = {
  preloadNext: true,
  // Browsers expose no sample-perfect gapless API for <audio>: the standby
  // element swap may carry a small boundary gap. Declared, not hidden.
  sampleAccurateGapless: false,
  decodeGate: false,
  // Element-based overlap is implemented (Phase A).
  crossfade: true,
};

interface ElementSlot {
  el: HTMLAudioElement;
  src: MediaElementAudioSourceNode;
  /** Dedicated pre-mix gain for this element: fades schedule AudioParam
   *  curves here instead of driving el.volume from a JS timer. */
  gain: GainNode;
  /** Set while this slot is the OUTGOING lane of a fade: its natural
   *  'ended' must not emit an eos (the core advanced at trigger time). */
  suppressEnded?: boolean;
  /** The queue item this slot was loaded for. Standby/policy agreement:
   *  only a standby matching the core's current policy target is promotable
   *  (see advance()). */
  item: PlaybackItem | null;
  url: string;
  /** Object URL created for a local-file source; revoked on dispose. */
  blobUrl?: string;
  /** True once the lane owns output (current or faded-in). Only OWNERSHIP
   *  failures are fatal to playback: a STANDBY that cannot decode its
   *  source (e.g. the next queue item needs another codec/engine) must not
   *  kill the sounding track — metadata() rejects, prepareNext returns
   *  null, and the core recovers by fresh-loading the target itself. */
  promoted: boolean;
}

export class NativeBackend {
  readonly kind = 'native' as const;
  readonly capabilities = NATIVE_CAPABILITIES;
  private ctx: AudioContext | null = null;
  private gain: GainNode | null = null;
  private current: ElementSlot | null = null;
  private standby: ElementSlot | null = null;
  /** Live element-based fade (Phase A). One at a time; pause completes it
   *  instantly, transport actions abort it back to the outgoing track. */
  private nativeFade: {
    outgoing: ElementSlot;
    incoming: ElementSlot;
    completed: boolean;
    timer: ReturnType<typeof setTimeout> | null;
    done: (() => void) | null;
  } | null = null;
  rate = 44100;
  private channels = 2;
  private token: string | null = null;
  private playRequest = 0;
  private shouldPlay = false;

  // events wired by the controller
  onEos: (() => void) | null = null;
  onError: ((msg: string) => void) | null = null;
  onPosition: (() => void) | null = null;
  onPrimed: (() => void) | null = null;
  onBuffering: (() => void) | null = null;

  /** Port-facing event registration (M2). Listeners fire IN ADDITION to the
   *  legacy assignable fields (which the web controller still uses until
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
  private emitLegacyAndPort(
    field: 'onEos' | 'onPosition' | 'onPrimed' | 'onBuffering',
    portName: EngineEventName,
  ): void {
    this[field]?.();
    this.emitNamed(portName);
  }

  async init(token: string | null): Promise<void> {
    this.token = token;
    this.ctx = new AudioContext();
    this.gain = this.ctx.createGain();
    this.gain.connect(this.ctx.destination);
  }

  private makeSlot(): ElementSlot {
    if (!this.ctx || !this.gain) throw new Error('native backend not initialized');
    const el = new Audio();
    el.crossOrigin = 'anonymous';
    el.preload = 'auto';
    const src = this.ctx.createMediaElementSource(el);
    const slotGain = this.ctx.createGain();
    src.connect(slotGain);
    slotGain.connect(this.gain);
    const slot: ElementSlot = { el, src, gain: slotGain, item: null, url: '', promoted: false };
    el.addEventListener('ended', () => {
      if (slot.suppressEnded) {
        // Outgoing lane of an active fade: the core advanced its cursor at
        // trigger time; this natural end must stay silent.
        el.pause();
        return;
      }
      this.emitLegacyAndPort('onEos', 'eos');
    });
    el.addEventListener('waiting', () => this.emitLegacyAndPort('onBuffering', 'buffering'));
    el.addEventListener('error', () => {
      if (!slot.promoted) {
        // Standby lane: a source the browser cannot decode (e.g. the next
        // queue item needs a different engine) is NOT a playback failure.
        // metadata() rejects and prepareNext reports null; the core's
        // boundary recovery fresh-loads the target with the right engine.
        return;
      }
      this.onError?.('This format cannot be played in your browser.');
      this.emitNamed('error');
    });
    el.addEventListener('timeupdate', () => this.emitLegacyAndPort('onPosition', 'tick'));
    return slot;
  }

  private loadInto(slot: ElementSlot, url: string): void {
    slot.url = url;
    // Same-origin audio (session cookie authenticates). crossOrigin anonymous
    // keeps the response CORS-clean so createMediaElementSource can route it.
    slot.el.src = url;
    slot.el.load();
  }

  /** Port signature (M4): open by resolved item. Local items resolve to a
   *  blob: object URL over the OPFS File (CSP already allows media-src
   *  blob:); remote ones use the server URL directly. */
  async open(item: PlaybackItem): Promise<EngineStreamInfo> {
    if (item.source.kind === 'local-file') {
      return this.openLocalSource(item.source.url);
    }
    return this.openSource(item.source.url);
  }

  async openSource(url: string, size = 0): Promise<EngineStreamInfo> {
    void size;
    this.playRequest++;
    this.shouldPlay = false;
    // A stale standby belongs to the PREVIOUS track's boundary; disposing it
    // here prevents a still-loaded (or still-fading) incoming element from
    // bleeding over the freshly selected track.
    this.abortNativeFade();
    const slot = this.makeSlot();
    this.disposeCurrent();
    this.disposeStandby();
    this.current = slot;
    slot.promoted = true;
    this.loadInto(slot, url);
    const info = await this.metadata(slot);
    this.rate = info.rate;
    return info;
  }

  /** Offline variant: resolves a committed OPFS audio file to an object
   *  URL and opens it. The URL is revoked when the slot is disposed. */
  async openLocalSource(key: string): Promise<EngineStreamInfo> {
    const objectUrl = await this.localObjectUrl(key);
    if (!objectUrl) throw new Error('Offline audio is missing from local storage.');
    try {
      return await this.openSource(objectUrl);
    } finally {
      if (this.current) this.current.blobUrl = objectUrl;
    }
  }

  /** Reads a committed offline audio file as a File and mints an object
   *  URL for element playback. Returns null when the file is absent. */
  private async localObjectUrl(key: string): Promise<string | null> {
    if (typeof navigator === 'undefined' || !navigator.storage?.getDirectory) return null;
    try {
      const root = await navigator.storage.getDirectory();
      const base = await root.getDirectoryHandle('musicpack-offline-v1');
      const releases = await base.getDirectoryHandle('releases');
      const fh = await releases.getFileHandle(key, { create: false });
      return URL.createObjectURL(await fh.getFile());
    } catch {
      return null;
    }
  }

  /** Port signature (M4). The item rides on the standby slot for
   *  standby/policy agreement at promotion time. */
  async prepareNext(item: PlaybackItem): Promise<EngineStreamInfo | null> {
    if (item.source.kind === 'local-file') {
      return this.prepareNextLocalSource(item.source.url, item);
    }
    return this.prepareNextSource(item.source.url, item);
  }

  /** Offline variant: resolves the OPFS file key to a blob object URL.
   *  Object URLs reference the browser-disk-backed File (no full JS copy);
   *  revoked when the slot is disposed. */
  async prepareNextLocalSource(key: string, item?: PlaybackItem): Promise<EngineStreamInfo | null> {
    const url = await this.localObjectUrl(key);
    if (!url) return null;
    const info = await this.prepareNextSource(url, item);
    if (info === null && url.startsWith('blob:')) URL.revokeObjectURL(url);
    else if (this.standby) this.standby.blobUrl = url;
    return info;
  }

  async prepareNextSource(url: string, item?: PlaybackItem, size = 0): Promise<EngineStreamInfo | null> {
    void size;
    this.disposeStandby();
    const slot = this.makeSlot();
    slot.item = item ?? null;
    this.standby = slot;
    this.loadInto(slot, url);
    try {
      return await this.metadata(slot);
    } catch {
      this.disposeStandby();
      return null;
    }
  }

  private metadata(slot: ElementSlot): Promise<EngineStreamInfo> {
    return new Promise((resolve, reject) => {
      if (slot.el.readyState >= 1 && slot.el.duration > 0 && !Number.isNaN(slot.el.duration)) {
        resolve(this.infoOf(slot));
        return;
      }
      const onMeta = () => {
        cleanup();
        resolve(this.infoOf(slot));
      };
      const onErr = () => {
        cleanup();
        reject(new Error('unplayable'));
      };
      const cleanup = () => {
        slot.el.removeEventListener('loadedmetadata', onMeta);
        slot.el.removeEventListener('error', onErr);
      };
      slot.el.addEventListener('loadedmetadata', onMeta);
      slot.el.addEventListener('error', onErr);
    });
  }

  private infoOf(slot: ElementSlot): EngineStreamInfo {
    const duration = slot.el.duration > 0 ? slot.el.duration : 0;
    return {
      rate: this.rate,
      channels: this.channels,
      version: 0,
      lengthSamples: Math.floor(duration * this.rate),
    };
  }

  /** Promotes the standby element to current (gapless handoff at EOS).
   *  Standby/policy agreement: `expected` is the item the core's CURRENT
   *  queue policy selected. A standby prepared for a different item — or a
   *  null expectation (nothing may follow) — is refused without touching
   *  either lane; the caller recovers by loading `expected` fresh. */
  async advance(expected: PlaybackItem | null): Promise<EngineStreamInfo | null> {
    if (
      !expected ||
      !this.standby ||
      !this.standby.item ||
      !sameItemIdentity(this.standby.item, expected)
    ) {
      return null;
    }
    const promoted = this.standby;
    this.playRequest++;
    this.shouldPlay = false;
    this.standby = null;
    promoted.promoted = true;
    this.disposeCurrent();
    this.current = promoted;
    return this.infoOf(promoted);
  }

  /** Crossfade (M8 Phase A, opt-in): overlap the standby element over the
   *  current one with equal-power ramps scheduled on per-slot GainNodes.
   *  Element-based timing is approximate by nature — the capability flag is
   *  honest about it. Returns null when no standby is loaded, a fade is
   *  already running, or a transport action superseded the ramp (caller
   *  falls back to EOS). */
  async beginCrossfade(next: PlaybackItem, fadeSeconds: number): Promise<CrossfadeResult | null> {
    const outgoing = this.current;
    if (!outgoing || !this.standby || !this.ctx || this.nativeFade) return null;
    // Standby/policy agreement: the caller's `next` is its current policy
    // target. A standby prepared for a different item must never be faded
    // in — refuse and let the normal EOS path recover.
    if (!this.standby.item || !sameItemIdentity(this.standby.item, next)) return null;
    const incoming = this.standby;
    const fade = Math.max(0.25, Math.min(15, fadeSeconds));

    // The old element's natural 'ended' mid-fade must not emit a stale eos.
    outgoing.suppressEnded = true;

    // Start the incoming element silent and ramp it in via AudioParam
    // curves (sample-accurate scheduling on the shared context clock).
    incoming.gain.gain.value = 0;
    try {
      await incoming.el.play();
    } catch {
      return null; // autoplay rejection: fall back to normal handoff
    }
    const n = 64;
    const outCurve = new Float32Array(n);
    const inCurve = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const x = (i + 1) / n;
      outCurve[i] = Math.cos((x * Math.PI) / 2);
      inCurve[i] = Math.sin((x * Math.PI) / 2);
    }
    const t0 = this.ctx.currentTime;
    const f = {
      outgoing,
      incoming,
      completed: false,
      timer: null as ReturnType<typeof setTimeout> | null,
      done: null as (() => void) | null,
    };
    outgoing.gain.gain.setValueCurveAtTime(outCurve, t0, fade);
    incoming.gain.gain.setValueCurveAtTime(inCurve, t0, fade);
    this.nativeFade = f;
    await new Promise<void>((resolve) => {
      f.done = resolve;
      f.timer = setTimeout(() => {
        if (this.nativeFade === f) this.completeNativeFade();
      }, fade * 1000);
    });

    // Whatever happened (natural completion, pause fast-forward, abort),
    // the ramp window is closed. Only a COMPLETED fade promotes; aborts
    // (transport supersede) leave the outgoing track in charge.
    if (!f.completed || this.current !== outgoing) {
      this.disposeStandby();
      return null;
    }
    this.playRequest++;
    this.standby = null;
    incoming.promoted = true;
    this.disposeCurrent();
    this.current = incoming;
    // Respect a pause that raced the completion: the core believes it is
    // paused, so the promoted element must not keep sounding.
    if (!this.shouldPlay) incoming.el.pause();
    // Element timing is approximate: report the nominal fade window as the
    // overlapped frames so the core's effective-length shrink matches what
    // was audibly consumed.
    return { info: this.infoOf(incoming), overlapFrames: Math.round(fade * this.rate) };
  }

  /** Fast-forwards the live fade to its end state (incoming sole owner). */
  private completeNativeFade(): void {
    const f = this.nativeFade;
    if (!f) return;
    this.nativeFade = null;
    if (f.timer) clearTimeout(f.timer);
    f.outgoing.gain.gain.cancelScheduledValues(0);
    f.outgoing.gain.gain.value = 0;
    f.incoming.gain.gain.cancelScheduledValues(0);
    f.incoming.gain.gain.value = 1;
    f.completed = true;
    const done = f.done;
    f.done = null;
    done?.();
  }

  /** Cancels the live fade back to the outgoing track; the incoming
   *  element goes silent and stays in the standby slot for disposal. */
  private abortNativeFade(): void {
    const f = this.nativeFade;
    if (!f) return;
    this.nativeFade = null;
    if (f.timer) clearTimeout(f.timer);
    f.outgoing.gain.gain.cancelScheduledValues(0);
    f.outgoing.gain.gain.value = 1;
    f.incoming.gain.gain.cancelScheduledValues(0);
    f.incoming.gain.gain.value = 0;
    f.incoming.el.pause();
    f.completed = false;
    const done = f.done;
    f.done = null;
    done?.();
  }

  startPumping(): void {
    // Browser-managed decoding needs no producer pump. The controller owns
    // the single audible play() request through the shared Backend contract.
  }

  pausePumping(): void {
    // Native decode is browser-managed; no pump to pause.
  }

  async play(): Promise<void> {
    const slot = this.current;
    if (!slot) return;
    this.shouldPlay = true;
    const request = ++this.playRequest;
    if (this.ctx && this.ctx.state !== 'running') await this.ctx.resume();
    if (request !== this.playRequest || !this.shouldPlay || this.current !== slot) return;
    await slot.el.play();
    if (!this.shouldPlay || this.current !== slot) slot.el.pause();
  }

  async pause(): Promise<void> {
    this.shouldPlay = false;
    // A pause during a live element fade fast-forwards the fade to its end
    // state: the continuation promotes the incoming element and, because
    // shouldPlay is now false, pauses it immediately. No orphaned audio.
    this.completeNativeFade();
    this.playRequest++;
    if (this.current) this.current.el.pause();
  }

  /** Port signature (M4). */
  async seekSample(samples: number): Promise<void> {
    return this.seek(samples);
  }

  async seek(sample: number): Promise<void> {
    if (!this.current) return;
    // A seek abandons the fade boundary: back to the outgoing track, the
    // incoming element goes silent (its standby slot is left for disposal).
    this.abortNativeFade();
    this.shouldPlay = false;
    this.playRequest++;
    this.current.el.currentTime = sample / this.rate;
    this.onPrimed?.();
    this.emitNamed('primed');
  }

  setGain(linear: number): void {
    if (this.gain) this.gain.gain.value = linear;
  }

  getPositionSamples(): number {
    const el = this.current?.el;
    if (!el || Number.isNaN(el.currentTime)) return 0;
    return Math.max(0, Math.floor(el.currentTime * this.rate));
  }

  renderedSamples(): number {
    return this.getPositionSamples();
  }

  getInfo(): EngineStreamInfo | null {
    return this.current ? this.infoOf(this.current) : null;
  }

  get lengthSamples(): number {
    const el = this.current?.el;
    if (!el || Number.isNaN(el.duration)) return 0;
    return Math.floor(el.duration * this.rate);
  }

  private disposeCurrent(): void {
    if (this.current) {
      this.current.el.pause();
      this.current.el.src = '';
      if (this.current.blobUrl) {
        URL.revokeObjectURL(this.current.blobUrl);
        this.current.blobUrl = undefined;
      }
      this.current = null;
    }
  }

  private disposeStandby(): void {
    if (this.standby) {
      this.standby.el.pause();
      this.standby.el.src = '';
      if (this.standby.blobUrl) {
        URL.revokeObjectURL(this.standby.blobUrl);
        this.standby.blobUrl = undefined;
      }
      this.standby = null;
    }
  }

  async close(): Promise<void> {
    this.shouldPlay = false;
    this.playRequest++;
    this.abortNativeFade();
    this.disposeCurrent();
    this.disposeStandby();
    if (this.ctx) {
      await this.ctx.close();
      this.ctx = null;
      this.gain = null;
    }
  }
}
