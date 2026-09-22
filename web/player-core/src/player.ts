// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The Player (player-core M4): the platform-independent playback
// orchestrator, lifted verbatim from the web PlayerController.
//
// Owns: transport state machine (loading/buffering/playing/paused/ended/
// error), queue orchestration, engine lifecycle + selection, album-absolute
// position bookkeeping, normalization application, session snapshot
// persistence (throttled), and media-control binding — all over injected
// ports. Hosts (web today, Android/iOS later) provide the engine factory,
// storage, media controls, and auth token; they never implement transport
// or queue semantics.
//
// Cancellation discipline (preserved verbatim): loadSeq/transportSeq
// generations, pauseIntent dominance, engine-identity checks. Engine events
// arrive through the Engine port's on() with sender tagging; late events
// from a closed engine are dropped.

import { writable, type Writable } from './store';
import type {
  CrossfadeResult,
  Engine,
  EngineEventName,
  PreloadEngine,
} from './engine';
import { isPreloadEngine, type EngineKind } from './engine';
import type { CrossfadeEngine } from './engine';
import { combinedGain, normalizationGainDb, type NormalizationMode } from './gain';
import { PlayerEventSink, type PlayerEvent, type PlayerListener } from './events';
import {
  decodeSnapshot,
  encodeSnapshot,
  SNAPSHOT_VERSION,
  type SessionSnapshot,
} from './snapshot';
import { PRIME_LEAD_SECONDS } from './transition';
import type { PlaybackItem, StreamInfo } from './types';

export type PlayerState =
  | 'idle'
  | 'loading'
  | 'buffering'
  | 'playing'
  | 'paused'
  | 'ended'
  | 'error';

/** End-of-album tolerance (samples). The output clock renders in small
 *  quanta, so the final render can stop a few samples short of total
 *  length; without tolerance the player would sit in `playing` forever
 *  after the last track. */
export const END_TOLERANCE_SAMPLES = 256;

export interface PlayerModel {
  state: PlayerState;
  current: PlaybackItem | null;
  positionSeconds: number;
  durationSeconds: number;
  /** Album-absolute start time of the current track (0 when none). */
  currentTrackStartSeconds: number;
  /** Per-track duration in seconds (lengthOf(idx)/rate), 0 when unknown. */
  currentTrackDurationSeconds: number;
  volume: number;
  normalizeMode: NormalizationMode;
  normDb: number;
  /** Queue playback policy (mirrored from the queue model for the UI). */
  repeat: 'off' | 'one' | 'all';
  shuffle: boolean;
  /** Crossfade length at natural boundaries; 0 = off (M8, opt-in). */
  crossfadeSeconds: number;
  error?: string;
}

/** Media-control surface (optional port). Web binds navigator.mediaSession;
 *  Android/iOS bind their OS surfaces. Position is TRACK-relative per the
 *  Media Session spec; the core converts, hosts never re-derive it. */
export interface MediaControlsPort {
  bind(handlers: {
    play(): void;
    pause(): void;
    next(): void;
    previous(): void;
    /** TRACK-relative seconds (spec semantics of `seekto`). */
    seek(trackSeconds: number): void;
    seekBy(deltaSeconds: number): void;
  }): void;
  setMetadata(item: PlaybackItem | null): void;
  setPosition(durationSeconds: number, positionSeconds: number): void;
}

/** Snapshot storage port. Same key/throttle discipline as the web v1. */
export interface StoragePort {
  get(): string | null;
  set(value: string | null): void;
}

export interface PlayerPorts {
  /** Resolves the engine kind for an item and builds the engine.
   *  Throwing here surfaces as a player error state. */
  engineFactory(kind: EngineKind, handlers: {
    primed(): void;
    buffering(): void;
    eos(): void;
    error(message: string): void;
    tick(): void;
  }): Engine;
  /** Which engine kind an item needs. Throw for unsupported formats. */
  resolveKind(item: PlaybackItem): EngineKind;
  /** Bearer token (or null for cookie/session hosts). */
  token?(): string | null;
  storage: StoragePort;
  mediaControls?: MediaControlsPort;
  /** Clock for persistence throttling. REQUIRED (purity law 2): the core
   *  never reads ambient time. Hosts pass () => Date.now(). */
  now(): number;
  /** One-shot timer port for the persist trailing write. REQUIRED (purity
   *  law 2): the core never touches ambient timers. Hosts pass
   *  (fn, ms) => setTimeout(fn, ms); returning an opaque cancel handle is
   *  enough when clearPersistTimer is provided. */
  schedulePersist(fn: () => void, ms: number): unknown;
  /** Cancels a handle returned by schedulePersist. */
  clearPersistTimer?(handle: unknown): void;
  /** Persist write throttle interval. Default 2000 ms. */
  persistIntervalMs?: number;
  /** Optional content-aware transition policy (Sweet Fades). When absent,
   *  crossfade uses the legacy fixed-length behavior. Pure function: hosts
   *  resolve content profiles themselves. */
  planTransition?(query: {
    outgoing: PlaybackItem;
    incoming: PlaybackItem;
    maxFadeSeconds: number;
    repeatOne: boolean;
  }): import('./transition').TransitionPlan;
}

export interface PlayerOptions {
  initialVolume?: number;
  initialNormalize?: NormalizationMode;
}

const DEFAULT_PERSIST_MS = 2000;

export class Player {
  readonly model: Writable<PlayerModel>;
  private sink = new PlayerEventSink();

  /** Subscribe to typed player events (integration surface, M7). */
  on(fn: PlayerListener): () => void {
    return this.sink.subscribe(fn);
  }
  private emit(event: PlayerEvent): void {
    this.sink.emit(event);
  }
  private queue: import('./queue').QueueModel;
  private ports: PlayerPorts;
  private engine: Engine | null = null;
  private engineKind: EngineKind | null = null;
  /** Exact decoded length per track id (samples at output rate). Keyed by
   *  id, not queue index, so entries survive loads and navigation. */
  private lengths = new Map<number, number>();
  private resetOffset = 0;
  private pendingEnded = false;
  /** True while an onEos() boundary handoff is between its engine promotion
   *  and the cursor advance (or its reload fallback). The tick catch-up must
   *  stand down inside that window: the promoted stream already sounds at
   *  qi+1 while the cursor is still on qi, so position-based adoption there
   *  would double-advance past the successor. Same ownership contract as
   *  crossfadeInProgress. Single-writer (onEos), so no epoch tracking. */
  private eosInFlight = false;
  /** Fix (c), scoped: the queue index a just-completed crossfade boundary
   *  authoritatively moved the cursor to, and the loadingSeq at that
   *  moment. tick() consumes this exactly once (its very next run) to
   *  verify the rebased position agrees with that index before ANY
   *  polling catch-up gets a chance to quietly paper over a disagreement.
   *  Cleared without checking if a new load supersedes it first. */
  private pendingBoundaryCheck: { idx: number; seq: number } | null = null;
  private loadingSeq = 0;
  private transportSeq = 0;
  private mutating = false;
  private pauseIntent = false;
  private unsub: (() => void) | null = null;
  private lastPersist = 0;
  private persistTimer: unknown = null;
  /** Within-track resume point from a restored session, consumed by the
   *  next load (valid only while the queue cursor is unchanged). */
  private restoredWithin: { index: number; withinSamples: number } | null = null;
  /** Crossfade setting (seconds); 0 = off. */
  private crossfadeSeconds = 0;
  /** Once-guard for the crossfade trigger within one track. */
  private crossfadeArmed = false;
  /** True while an overlapped transition is running. */
  private crossfadeInProgress = false;
  /** Engine-event subscriptions for the current engine. */
  private engineUnsubs: Array<() => void> = [];
  /** Engine identity token for late-event dropping. */
  private engineEpoch = 0;

  constructor(queue: import('./queue').QueueModel, ports: PlayerPorts, opts: PlayerOptions = {}) {
    this.queue = queue;
    this.ports = ports;
    this.model = writable<PlayerModel>({
      state: 'idle',
      current: null,
      positionSeconds: 0,
      durationSeconds: 0,
      currentTrackStartSeconds: 0,
      currentTrackDurationSeconds: 0,
      volume: opts.initialVolume ?? 0.8,
      normalizeMode: opts.initialNormalize ?? 'album',
      normDb: 0,
      repeat: 'off',
      shuffle: false,
      crossfadeSeconds: 0,
    });
  }

  init(): void {
    this.ports.mediaControls?.bind({
      play: () => void this.togglePlay(),
      pause: () => void this.pause(),
      next: () => void this.next(),
      previous: () => void this.previous(),
      // Media Session delivers TRACK-relative seek times (see tick());
      // convert to the album-absolute targets the transport speaks.
      seek: (trackSeconds) => void this.seek(this.model.get().currentTrackStartSeconds + trackSeconds),
      seekBy: (d) => this.seek(this.model.get().positionSeconds + d),
    });
    // Restore BEFORE subscribing: a writable store fires its subscriber
    // immediately on registration, and the trailing persist() in that
    // callback would otherwise see an empty player and delete the very
    // state we are about to restore.
    this.restore();
    // React to queue changes made OUTSIDE the player (e.g. the queue panel
    // removing the current item, or clearing the queue). Mutations the
    // player itself performs are flagged `mutating` and skipped here.
    this.unsub = this.queue.subscribe(() => {
      if (this.mutating) return;
      const m = this.model.get();
      const cur = this.queue.current();
      if (!cur && m.state !== 'idle' && m.state !== 'error') {
        void this.stop();
      } else if (cur && m.current && cur.id !== m.current.id && m.state !== 'loading') {
        void this.load();
      }
      this.persist();
    });
  }

  // ---- cross-reload persistence -------------------------------------------

  private persist(): void {
    this.lastPersist = this.now();
    const m = this.model.get();
    const q = this.queue.get();
    if (!m.current || q.items.length === 0) {
      this.ports.storage.set(null);
      return;
    }
    const payload: SessionSnapshot = {
      v: SNAPSHOT_VERSION,
      items: q.items as unknown as SessionSnapshot['items'],
      index: q.index,
      positionSeconds: m.positionSeconds,
      volume: m.volume,
      normalizeMode: m.normalizeMode,
      repeat: this.queue.repeat,
      shuffle: this.queue.shuffle,
      crossfadeSeconds: this.crossfadeSeconds,
    };
    try {
      this.ports.storage.set(encodeSnapshot(payload));
    } catch {
      /* quota exceeded / storage disabled — persistence is best-effort */
    }
  }

  private persistThrottled(): void {
    if (this.now() - this.lastPersist >= (this.ports.persistIntervalMs ?? DEFAULT_PERSIST_MS)) {
      this.persist();
      return;
    }
    // Inside the throttle window: schedule one trailing write so the latest
    // position is never lost (re-arming is cheap; persist() is tiny).
    if (this.persistTimer !== null) return;
    this.persistTimer = this.ports.schedulePersist(() => {
      this.persistTimer = null;
      this.persist();
    }, this.ports.persistIntervalMs ?? DEFAULT_PERSIST_MS);
  }

  private now(): number {
    return this.ports.now();
  }

  /** Rebuilds a paused session from the last persisted state (if any).
   *  Audio itself cannot resume without a user gesture; Play then seeks to
   *  the restored position through the normal load path. */
  private restore(): void {
    let raw: string | null = null;
    try {
      raw = this.ports.storage.get();
    } catch {
      return;
    }
    const parsed = decodeSnapshot(raw);
    if (!parsed) return;

    // Policy must be applied BEFORE the cursor install so shuffle builds its
    // presentation order around the restored current item.
    this.queue.setRepeat(parsed.repeat);
    if (typeof parsed.crossfadeSeconds === 'number' && parsed.crossfadeSeconds > 0) {
      this.crossfadeSeconds = [4, 8, 12].includes(parsed.crossfadeSeconds)
        ? parsed.crossfadeSeconds
        : 0;
    }
    this.mutating = true;
    this.queue.set({ items: parsed.items as import('./types').PlaybackItem[], index: parsed.index });
    this.mutating = false;
    // Enable shuffle AFTER the cursor install: setShuffle rebuilds its
    // current-first presentation order around the restored item.
    if (parsed.shuffle) this.queue.setShuffle(true);
    const item = this.queue.current();
    if (!item) return;

    const rate = this.rate();
    const offs = this.offsets();
    const startSamples = offs[parsed.index] ?? 0;
    const withinSamples = Math.max(
      0,
      Math.min(Math.floor(parsed.positionSeconds * rate), Math.max(0, this.lengthOf(parsed.index) - 1)),
    );
    this.restoredWithin = { index: parsed.index, withinSamples };
    this.model.update((m) => ({
      ...m,
      current: item,
      state: 'paused' as PlayerState,
      positionSeconds: (startSamples + withinSamples) / rate,
      currentTrackStartSeconds: startSamples / rate,
      currentTrackDurationSeconds: this.lengthOf(parsed.index) / rate,
      durationSeconds: this.totalLength() / rate,
      volume: typeof parsed.volume === 'number' ? Math.min(1, Math.max(0, parsed.volume)) : m.volume,
      normalizeMode: parsed.normalizeMode ?? m.normalizeMode,
      repeat: this.queue.repeat,
      shuffle: this.queue.shuffle,
      crossfadeSeconds: this.crossfadeSeconds,
    }));
    this.emit({ t: 'track', item });
    this.setMediaFor(item);
  }

  destroy(): void {
    this.unsub?.();
    this.unsub = null;
    this.detachEngineListeners();
    void this.engine?.close();
  }

  // ---- state helpers ------------------------------------------------------

  private setState(state: PlayerState): void {
    this.model.update((m) => ({ ...m, state }));
    this.emit({ t: 'state', state });
  }

  private rate(): number {
    return this.engine?.capabilities && 'rate' in this.engine
      ? ((this.engine as unknown as { rate: number }).rate ?? 44100)
      : 44100;
  }

  private lengthOf(i: number): number {
    const item = this.queue.at(i);
    if (!item) return 0;
    const exact = this.lengths.get(item.trackId);
    if (exact !== undefined) return exact;
    return Math.floor((item.durationHintSeconds ?? 0) * this.rate());
  }

  private offsets(): number[] {
    const items = this.queue.get().items;
    const offs: number[] = [];
    let acc = 0;
    for (let i = 0; i < items.length; i++) {
      offs[i] = acc;
      acc += this.lengthOf(i);
    }
    return offs;
  }

  private totalLength(): number {
    const offs = this.offsets();
    const n = offs.length;
    if (n === 0) return 0;
    return (offs[n - 1] ?? 0) + this.lengthOf(n - 1);
  }

  private albumPosition(): number {
    if (!this.engine) return 0;
    const rendered = this.engine.renderedSamples();
    // Pull-decode engines (legacy WASM musepack and Rust) report session-
    // relative frames; the core carries the absolute base via resetOffset.
    if (this.engineKind === 'musepack' || this.engineKind === 'rust') return this.resetOffset + rendered;
    const idx = this.queue.get().index;
    const base = idx >= 0 ? (this.offsets()[idx] ?? 0) : 0;
    return base + rendered;
  }

  // ---- queue actions -------------------------------------------------------

  async playItem(item: PlaybackItem): Promise<void> {
    this.pauseIntent = false;
    this.setState('loading');
    this.mutating = true;
    this.queue.playNow(item);
    this.mutating = false;
    await this.load();
  }

  /** Jumps to an existing queue item without touching the rest of the queue
   *  (the queue-panel click). Deliberately NOT playItem(): replacing the
   *  queue would throw away everything the user had lined up. */
  async playQueueIndex(i: number): Promise<void> {
    if (!this.queue.at(i) || i === this.queue.get().index) return;
    this.pauseIntent = false;
    this.setState('loading');
    this.mutating = true;
    this.queue.moveTo(i);
    this.mutating = false;
    await this.load();
  }

  async playSequence(items: PlaybackItem[], title: string, artist: string, startIndex = 0): Promise<void> {
    this.pauseIntent = false;
    this.setState('loading');
    this.mutating = true;
    this.queue.playSequence(items, startIndex);
    this.mutating = false;
    await this.load();
  }

  async next(): Promise<void> {
    ++this.loadingSeq;
    this.mutating = true;
    const item = this.queue.next();
    this.mutating = false;
    if (item) await this.load();
  }

  async previous(): Promise<void> {
    const m = this.model.get();
    // "Previous" restarts the CURRENT song; only a second press (or pressing
    // it right after the song began) moves to the previous queue item.
    if (m.current && m.positionSeconds - m.currentTrackStartSeconds > 3) {
      await this.restartCurrentTrack();
      return;
    }
    ++this.loadingSeq;
    this.mutating = true;
    const item = this.queue.previous();
    this.mutating = false;
    if (item) await this.load();
  }

  /** Seeks to sample 0 of the CURRENT track. Deliberately not routed through
   *  seek(): an album-absolute target of 0 resolves to queue index 0, which
   *  would restart the whole queue instead of this song. */
  private async restartCurrentTrack(): Promise<void> {
    if (!this.engine || !this.model.get().current) return;
    ++this.transportSeq;
    const seq = ++this.loadingSeq;
    const base = this.offsets()[this.queue.get().index] ?? 0;
    this.resetOffset = base;
    this.setState(this.pauseIntent ? 'paused' : 'buffering');
    this.model.update((mm) => ({ ...mm, positionSeconds: base / this.rate() }));
    await this.engine.seekSample(0);
    if (seq !== this.loadingSeq) return;
    if (!this.pauseIntent) this.gate().start();
  }

  // ---- load / playback -----------------------------------------------------

  private async load(resumeWithinSamples?: number): Promise<boolean> {
    const item = this.queue.current();
    if (!item) return false;
    const seq = ++this.loadingSeq;
    ++this.transportSeq;
    this.pendingEnded = false;
    this.restoredWithin = null;
    this.crossfadeArmed = false;
    // Lengths are keyed by track id, so a replaced queue cannot leak
    // another album's entries (ids are unique per server track row).
    this.model.update((m) => ({ ...m, state: 'loading', error: undefined }));
    try {
      const kind = this.ports.resolveKind(item);
      await this.ensureEngine(kind);
      if (seq !== this.loadingSeq) return false;
      const info = await this.engine!.open(item);
      if (seq !== this.loadingSeq) return false;
      const qi = this.queue.get().index;
      this.lengths.set(item.trackId, info.lengthSamples);
      // Publish track geometry NOW: the seek control reads these fields, and
      // waiting for the first rendered tick leaves a window where its hidden
      // input has max=0 (clicks clamp to seek(0) = wrong song).
      const offs = this.offsets();
      this.model.update((m) => ({
        ...m,
        currentTrackStartSeconds: (offs[qi] ?? 0) / this.rate(),
        currentTrackDurationSeconds: this.lengthOf(qi) / this.rate(),
      }));
      // Prepare the POLICY next track ahead of time (gapless). Under
      // shuffle this follows the presentation order; under repeat-all the
      // wrap target preloads too.
      const next = this.peekPreloadTarget(qi);
      if (next && isPreloadEngine(this.engine!)) {
        const ni = await (this.engine as PreloadEngine).prepareNext(next.item);
        if (seq !== this.loadingSeq) return false;
        if (ni) this.lengths.set(next.item.trackId, ni.lengthSamples);
      }
      const resumeAt =
        resumeWithinSamples && resumeWithinSamples > 0
          ? Math.min(Math.floor(resumeWithinSamples), Math.max(0, this.lengthOf(qi) - 1))
          : 0;
      this.resetOffset = (this.offsets()[qi] ?? 0) + resumeAt;
      this.applyGain(item);
      this.setMediaFor(item);
      this.model.update((m) => ({
        ...m,
        current: item,
        state: this.pauseIntent ? 'paused' : 'buffering',
        positionSeconds: this.resetOffset / this.rate(),
        durationSeconds: this.totalLength() / this.rate(),
      }));
      this.emit({ t: 'track', item });
      await this.engine!.seekSample(resumeAt);
      if (seq !== this.loadingSeq) return false;
      if (!this.pauseIntent) this.gate().start();
      this.persist();
      return true;
    } catch (e) {
      if (seq === this.loadingSeq) this.fail(e instanceof Error ? e.message : String(e));
      return false;
    }
  }

  private async onEos(): Promise<void> {
    if (!this.engine) return;
    const seq = this.loadingSeq;
    this.eosInFlight = true;
    try {
      await this.runEosBoundary(seq);
    } finally {
      this.eosInFlight = false;
    }
  }

  /** The whole natural-boundary handoff: last-chance fade, standby
   *  promotion, cursor advance (or reload/end fallback). Extracted from
   *  onEos() solely so the eosInFlight guard wraps every exit path. */
  private async runEosBoundary(seq: number): Promise<void> {
    // The engine may have been torn down while the boundary event was in
    // flight; a boundary without an engine has no handoff to perform.
    if (!this.engine) return;
    // Last-chance Sweet Fade: the decoder reached EOF while buffered audio
    // is still sounding (decode runs far ahead of playback, especially
    // right after a seek into the fade window — the positional trigger may
    // never observe it). Try an overlapped transition whose overlap is
    // clamped to the audio that is actually left; on decline/failure the
    // normal gapless handoff below runs unchanged.
    if (await this.tryFadeAtEos(seq)) return;
    // This eos's own fade attempt declined, but ANOTHER fade owns this
    // exact boundary (positional trigger priming/mixing): that owner
    // performs the advancement. Running the gapless handoff here would
    // double-step the cursor past its head ("track N -> N+1 -> instantly
    // N+2"). Eoses belonging to LATER tracks are unaffected: by then the
    // owner has finished and crossfadeInProgress is false again.
    if (this.crossfadeInProgress) return;
    // A natural boundary may no longer own the transport: the user can pause
    // (or otherwise take control) while a handoff already dispatched its
    // standby advance is still in flight. An auto-advancing handoff that
    // resumes after a pause would clobber the paused state and restart audio;
    // bail and let the paused session stand (it resumes/advances on play()).
    if (this.pauseIntent) return;
    const engine = this.engine;
    // The policy target as of NOW — not what the standby happened to be
    // prepared with. The engine refuses (and the recovery branch below
    // reloads) whenever queue edits or repeat/shuffle changes moved the
    // target after prepareNext ran.
    const expected = this.peekPreloadTarget(this.queue.get().index);
    let info: StreamInfo | null = null;
    if (isPreloadEngine(engine)) {
      info = await engine.advance(expected ? expected.item : null);
    }
    if (seq !== this.loadingSeq) return;
    // The pause may have landed while the standby advance was awaited above:
    // re-check before touching the cursor, otherwise the handoff resumes into
    // a paused session and restarts audio against the user's intent.
    if (this.pauseIntent) return;

    // Repeat-one: fresh reload of the SAME item (sample-exact via the normal
    // load path; no timeline concept needed).
    if (this.queue.repeat === 'one') {
      this.mutating = true;
      this.queue.moveTo(this.queue.get().index); // cursor unchanged
      this.mutating = false;
      await this.load(0);
      return;
    }

    this.mutating = true;
    const item = this.queue.next();
    this.mutating = false;
    if (!item) {
      // Policy says nothing follows: let the output drain, then end. The
      // advance(null) contract guarantees nothing was promoted here.
      this.pendingEnded = true;
      this.gate().stop();
      this.tick();
      return;
    }
    if (
      !info ||
      !expected ||
      item.id !== expected.item.id ||
      item.trackId !== expected.item.trackId
    ) {
      // No usable standby, or the promoted lane did not match current
      // policy (queue edited / repeat-shuffle changed mid-track): recover
      // by loading the policy-selected item fresh instead of overlaying
      // foreign stream facts onto it — and never end the session while
      // tracks remain. Exactly one advancement happened above.
      await this.load();
      return;
    }
    const qi = this.queue.get().index;
    this.lengths.set(item.trackId, info.lengthSamples);
    // Same as load(): publish geometry synchronously so the seek control
    // never sees the previous track's start/duration after a handoff.
    const handoffOffs = this.offsets();
    this.model.update((m) => ({
      ...m,
      currentTrackStartSeconds: (handoffOffs[qi] ?? 0) / this.rate(),
      currentTrackDurationSeconds: this.lengthOf(qi) / this.rate(),
    }));
    this.applyGain(item);
    this.setMediaFor(item);
    const next = this.peekPreloadTarget(qi);
    if (next && isPreloadEngine(this.engine)) {
      const ni = await (this.engine as PreloadEngine).prepareNext(next.item);
      if (seq !== this.loadingSeq) return;
      if (ni) this.lengths.set(next.item.trackId, ni.lengthSamples);
    }
    this.model.update((m) => ({
      ...m,
      current: item,
      state: this.pauseIntent || m.state === 'paused' ? 'paused' : 'buffering',
      durationSeconds: this.totalLength() / this.rate(),
    }));
    this.emit({ t: 'track', item });
    if (!this.pauseIntent) {
      const engine = this.engine;
      const transportSeq = this.transportSeq;
      this.gate().start();
      try {
        await engine.play();
        if (
          seq === this.loadingSeq &&
          transportSeq === this.transportSeq &&
          !this.pauseIntent &&
          this.engine === engine
        ) {
          this.setState('playing');
        }
      } catch {
        if (
          seq === this.loadingSeq &&
          transportSeq === this.transportSeq &&
          this.engine === engine
        ) {
          this.setState('paused');
        }
      }
    }
  }

  private onPrimed(): void {
    const m = this.model.get();
    if (m.state === 'loading' || m.state === 'buffering') {
      const seq = this.loadingSeq;
      const transportSeq = this.transportSeq;
      const engine = this.engine;
      engine
        ?.play()
        .then(() => {
          const state = this.model.get().state;
          if (
            seq === this.loadingSeq &&
            transportSeq === this.transportSeq &&
            !this.pauseIntent &&
            this.engine === engine &&
            (state === 'loading' || state === 'buffering')
          ) {
            this.setState('playing');
          }
        })
        .catch(() => undefined);
    }
  }

  private onBuffering(): void {
    const state = this.model.get().state;
    if (state === 'loading' || state === 'buffering' || state === 'playing') {
      this.setState('buffering');
    }
  }

  private tick(): void {
    if (!this.engine) return;
    const pos = this.albumPosition();
    const dur = this.totalLength();
    const qi = this.queue.get().index;
    const idx = this.currentIndexAt(pos);
    // Fix (c), scoped: consume the one-shot post-crossfade boundary check
    // BEFORE any catch-up logic runs, so a real disagreement is reported
    // as itself rather than silently blended into the heuristic below.
    // A superseded load (seq mismatch) discards the check without judging
    // it — the boundary it was about is no longer the one being played.
    if (this.pendingBoundaryCheck !== null) {
      const check = this.pendingBoundaryCheck;
      this.pendingBoundaryCheck = null;
      if (check.seq === this.loadingSeq && idx !== check.idx) {
        this.emit({
          t: 'boundary-drift',
          expectedIndex: check.idx,
          observedIndex: idx,
          positionSamples: pos,
        });
      }
    }
    // onEos() owns forward advancement. Only a LATER index may be adopted
    // here (never a regression): the output ring renders ahead, so at a
    // gapless handoff the rendered position can still describe the previous
    // track for a moment while the cursor has already advanced. While a
    // crossfade owns the boundary the transition itself advances the cursor,
    // so the catch-up must stand down to avoid racing it through extra tracks.
    // It is also capped at ONE step beyond the cursor, and only onto a
    // track whose own length is known: when later tracks' lengths are
    // unknown their offsets collapse onto the current end, so
    // currentIndexAt() reports the LAST such index — adopting it skipped
    // straight to that track (song 1 -> last-song jump). Without a real
    // length under the target the album clock simply cannot prove a
    // boundary there; EOS/load owns the handoff instead.
    //
    // Two further stand-downs close the remaining adoption windows where
    // "position ahead of cursor" does NOT mean a real boundary was crossed:
    // - navigation: previous()/next()/seek hold a transport load open on
    //   state 'loading'/'buffering' while the OLD engine still renders the
    //   old higher-offset track; adopting there bounced backward skips
    //   forward toward the abandoned position.
    // - eos promotion: while eosInFlight the promoted standby sounds at
    //   qi+1 before next() moved the cursor; adopting stole one advance
    //   so the natural boundary landed two tracks ahead ("next skipped").
    if (
      this.model.get().state === 'playing' &&
      idx > qi &&
      !this.pendingEnded &&
      !this.crossfadeInProgress &&
      !this.eosInFlight
    ) {
      const target = Math.min(idx, qi + 1);
      if (this.lengthOf(target) > 0) {
        this.mutating = true;
        this.queue.moveTo(target);
        this.mutating = false;
      }
    }
    const offs = this.offsets();
    const trackStart = offs[idx] ?? 0;
    const trackDur = this.lengthOf(idx);
    const rate = this.rate();
    // Crossfade trigger (M8, opt-in; once per track): near the end of the
    // CURRENT item, playing, with a policy next item and a capable engine.
    // Never for repeat-one (reload semantics win) and never on the queue's
    // last track. With a planTransition port, the overlap is content-aware
    // and gapless/hard-cut plans simply let the EOS path own the boundary.
    if (
      !this.crossfadeArmed &&
      !this.crossfadeInProgress &&
      // BUG-1 Fix A: while an eos/boundary handoff is in flight the
      // transport belongs to that handoff — the cursor may already have
      // advanced while the successor's standby/profile is still being
      // prepared, so a fade attempted here races it and the engine
      // legitimately declines mid-handoff. Mirror the cursor catch-up's
      // stand-down: the handoff's own EOS path owns this boundary.
      !this.eosInFlight &&
      !this.pendingEnded &&
      this.crossfadeSeconds > 0 &&
      this.queue.repeat !== 'one' &&
      this.model.get().state === 'playing'
    ) {
      const remaining = this.trackRemainingSamples(idx, pos);
      const fadeFrames = Math.floor(this.crossfadeSeconds * rate);
      const singleTrackRepeatAll =
        this.queue.repeat === 'all' && this.queue.get().items.length === 1;
      if (
        remaining <= fadeFrames &&
        remaining > 0 &&
        !singleTrackRepeatAll &&
        this.engine.capabilities.crossfade
      ) {
        const target = this.peekPreloadTarget(qi);
        if (target) {
          const plan = this.ports.planTransition
            ? this.ports.planTransition({
                outgoing: this.queue.at(qi)!,
                incoming: target.item,
                maxFadeSeconds: this.crossfadeSeconds,
                // The outer guard already excluded repeat-one.
                repeatOne: false,
              })
            : { type: 'sweet-fade' as const, overlapSeconds: this.crossfadeSeconds };
          if (plan.type === 'sweet-fade') {
            // Arm slightly early so the engine can prime its lane.
            const lead = Math.floor((plan.overlapSeconds + PRIME_LEAD_SECONDS) * rate);
            if (remaining <= lead) {
              this.crossfadeArmed = true;
              void this.beginCrossfadeTransition(plan.overlapSeconds);
            }
          }
          // gapless / hard-cut plans: nothing to do — the natural EOS path
          // owns the boundary, which is exactly what those plans describe.
        }
      }
    }
    // End of queue: the positional drain is the normal signal, but a
    // crossfade compresses the album clock (overlap consumes both lanes in
    // one pass), so the engine's drained signal (decoder done + output dry)
    // is an equally valid end marker.
    const drained =
      typeof (this.engine as unknown as { isOutputDrained?: () => boolean }).isOutputDrained === 'function'
        ? (this.engine as unknown as { isOutputDrained(): boolean }).isOutputDrained()
        : false;
    const ended = this.pendingEnded && (pos >= dur - END_TOLERANCE_SAMPLES || drained);
    this.model.update((m) => ({
      ...m,
      positionSeconds: pos / rate,
      durationSeconds: dur / rate,
      currentTrackStartSeconds: trackStart / rate,
      currentTrackDurationSeconds: trackDur / rate,
      state: ended ? 'ended' : m.state,
    }));
    // Media Session position state is TRACK-relative per spec: the OS
    // scrubber shows/requests positions inside the current song, not the
    // album. tick() therefore reports within-track values and converts
    // incoming seekto times back to album-absolute in init().
    const trackSeconds = trackDur / rate;
    const withinSeconds = Math.max(0, pos - trackStart) / rate;
    this.emit({
      t: 'position',
      positionSeconds: withinSeconds,
      trackStartSeconds: trackStart / rate,
      trackDurationSeconds: trackSeconds,
    });
    this.ports.mediaControls?.setPosition(trackSeconds, withinSeconds);
    if (ended) {
      this.pendingEnded = false;
      void this.engine.pause();
      this.gate().stop();
    }
    this.persistThrottled();
  }

  /** The item EOS/load should preload: canonical qi+1, or the policy target
   *  (shuffle order / repeat wrap). Null when nothing follows. */
  private peekPreloadTarget(qi: number): { item: PlaybackItem } | null {
    const s = this.queue.get();
    if (this.queue.shuffle) {
      const order = this.queue.getPresentationOrder();
      const pos = order.indexOf(qi);
      if (pos >= 0 && pos + 1 < order.length) {
        const idx = order[pos + 1]!;
        const it = this.queue.at(idx);
        return it ? { item: it } : null;
      }
      if (this.queue.repeat === 'all') {
        const first = order[0];
        const it = first === undefined ? null : this.queue.at(first);
        return it ? { item: it } : null;
      }
      return null;
    }
    if (qi + 1 < s.items.length) {
      const it = this.queue.at(qi + 1);
      return it ? { item: it } : null;
    }
    if (this.queue.repeat === 'all' && s.items.length > 0) {
      const it = this.queue.at(0);
      return it ? { item: it } : null;
    }
    return null;
  }

  /** Samples remaining in item `idx` at absolute position `pos` (>=0). */
  private trackRemainingSamples(idx: number, pos: number): number {
    const offs = this.offsets();
    const start = offs[idx] ?? 0;
    return Math.max(0, (start + this.lengthOf(idx)) - pos);
  }


  /** Attempts an overlapped transition to the policy next item. On success
   *  performs the same bookkeeping as the EOS handoff, shrinking the
   *  outgoing track's effective length by the reported overlap so offsets,
   *  positions and end detection stay truthful. On failure leaves
   *  everything untouched so the normal EOS path runs.
   *
   *  Epoch discipline: only a NEW LOAD (loadingSeq) or an engine swap
   *  invalidates a running transition — pause/resume deliberately do NOT.
   *  The engine-side fade freezes with the suspended AudioContext and
   *  completes on resume, so its handoff must still be applied afterwards;
   *  re-triggering is prevented by crossfadeInProgress staying set for the
   *  whole attempt (pause no longer clears it). */
  private async beginCrossfadeTransition(
    overlapSeconds?: number,
    seqHint?: number,
  ): Promise<void> {
    const engine = this.engine;
    if (!engine || !engine.capabilities.crossfade) return;
    const cf = engine as unknown as CrossfadeEngine;
    if (typeof cf.beginCrossfade !== 'function') return;
    const requested = Math.max(0.25, Math.min(15, overlapSeconds ?? this.crossfadeSeconds));
    const seqAtStart = seqHint ?? this.loadingSeq;
    const qi = this.queue.get().index;
    const target = this.peekPreloadTarget(qi);
    // No target: end-of-queue or repeat-one — no fade.
    if (!target) return;
    const prevItem = this.queue.at(qi);
    const prevLen = prevItem ? this.lengths.get(prevItem.trackId) : undefined;
    this.crossfadeInProgress = true;
    try {
      const result: CrossfadeResult | null = await cf.beginCrossfade(target.item, requested);
      if (!result) return; // fall back to normal EOS
      // Superseded by a load/engine swap while the fade ran: that path owns
      // all state now. (Pause/resume intentionally do not land here.)
      if (seqAtStart !== this.loadingSeq || this.engine !== engine) return;
      // Same handoff bookkeeping as onEos(): lengths, geometry, gain, media,
      // next standby. Cursor advance WITHOUT queue.next() policy re-entry —
      // we already resolved the target.
      const nextItem = target.item;
      const idxAfter = this.queue
        .get()
        .items.findIndex((it) => it.id === nextItem.id && it.trackId === nextItem.trackId);
      if (idxAfter < 0) return;
      // Compress the album clock by the actual overlap: the faded-out track
      // keeps `length - overlap` effective frames, so every later offset —
      // position, seek mapping, Media Session, end-of-queue — stays exact.
      if (prevItem && result.overlapFrames > 0 && prevLen !== undefined) {
        this.lengths.set(prevItem.trackId, Math.max(0, prevLen - result.overlapFrames));
      }
      this.mutating = true;
      this.queue.moveTo(idxAfter);
      this.mutating = false;
      // Fix (c), scoped: arm the one-shot boundary check for the very next
      // tick(). seqAtStart is fine here — we already returned above if a
      // newer load/engine swap superseded this attempt.
      this.pendingBoundaryCheck = { idx: idxAfter, seq: seqAtStart };
      this.lengths.set(nextItem.trackId, result.info.lengthSamples);
      const offs = this.offsets();
      const nqi = this.queue.get().index;
      this.model.update((m) => ({
        ...m,
        currentTrackStartSeconds: (offs[nqi] ?? 0) / this.rate(),
        currentTrackDurationSeconds: this.lengthOf(nqi) / this.rate(),
        durationSeconds: this.totalLength() / this.rate(),
        current: nextItem,
      }));
      this.emit({ t: 'track', item: nextItem });
      this.applyGain(nextItem);
      this.setMediaFor(nextItem);
      // The fade moved us onto a NEW track: allow its own boundary to fade.
      this.crossfadeArmed = false;
      const next2 = this.peekPreloadTarget(nqi);
      if (next2 && isPreloadEngine(engine)) {
        const ni = await (engine as PreloadEngine).prepareNext(next2.item);
        if (ni) this.lengths.set(next2.item.trackId, ni.lengthSamples);
      }
      this.persist();
    } catch {
      /* fall back to normal EOS */
    } finally {
      this.crossfadeInProgress = false;
    }
  }

  /** Attempts a fade when the decoder's EOS arrives with buffered audio
   *  still to play (decode beats the positional trigger). Returns true iff
   *  the transition was taken — the caller then skips the normal handoff.
   *  All policy guards mirror the positional trigger. */
  private async tryFadeAtEos(seq: number): Promise<boolean> {
    if (this.crossfadeSeconds <= 0 || this.queue.repeat === 'one') return false;
    if (this.pauseIntent || this.pendingEnded || this.crossfadeInProgress) return false;
    const engine = this.engine;
    if (!engine || !engine.capabilities.crossfade) return false;
    if (!isPreloadEngine(engine)) return false;
    const cf = engine as unknown as CrossfadeEngine;
    if (typeof cf.beginCrossfade !== 'function') return false;
    const qi = this.queue.get().index;
    const target = this.peekPreloadTarget(qi);
    if (!target) return false;
    const remaining = this.trackRemainingSamples(qi, this.albumPosition());
    // Only while the faded-out track still has audible content buffered:
    // otherwise the boundary already happened and the EOS path owns it.
    const rate = this.rate();
    if (remaining <= END_TOLERANCE_SAMPLES) return false;
    // Content-aware like the positional trigger: ask the planner for the
    // overlap, then clamp to the audio that is actually left. Without a port
    // (or with no content data) the planner degrades to the fixed cap below.
    const maxOverlap = Math.min(this.crossfadeSeconds, remaining / rate);
    let overlapSeconds = maxOverlap;
    let decline = false;
    if (this.ports.planTransition) {
      const outgoing = this.queue.at(qi);
      const plan = outgoing
        ? this.ports.planTransition({
            outgoing,
            incoming: target.item,
            maxFadeSeconds: maxOverlap,
            // The outer guard already excluded repeat-one.
            repeatOne: false,
          })
        : null;
      if (plan?.type === 'sweet-fade') {
        // The planner is bounded by maxOverlap, but clamp defensively: the
        // fade can never exceed the audio that is actually left to play.
        overlapSeconds = Math.min(plan.overlapSeconds, maxOverlap);
      } else {
        // gapless / hard-cut: no overlapped transition — let the normal EOS
        // handoff own the boundary.
        decline = true;
      }
    }
    if (decline || overlapSeconds <= 0) return false;
    // The eos-race blend starts AT decode-eos: the worklet plays the last
    // `overlap` frames of this track blended with the successor's head, then
    // swaps — discarding whatever buffered audio remained beyond the blend
    // window. The declared offsets must place the successor at
    // (raw end − remaining-at-eos) so the model matches the engine clock;
    // shrinking by only the blended overlap leaves a fade-window wedge that
    // made the positional trigger instant-fade the successor right after
    // this boundary ("2 -> 3 -> almost instantly 4").
    const remainingSamplesAtEos = remaining;
    const rawLenSamples = this.lengthOf(qi);
    await this.beginCrossfadeTransition(overlapSeconds, seq);
    const taken = this.queue.get().index !== qi;
    if (taken && rawLenSamples > 0) {
      const outgoing = this.queue.at(qi);
      if (outgoing) {
        const declaredEnd = Math.max(0, rawLenSamples - remainingSamplesAtEos);
        this.lengths.set(outgoing.trackId, declaredEnd);
        const offs = this.offsets();
        const qiNow = this.queue.get().index;
        this.model.update((m) => ({
          ...m,
          currentTrackStartSeconds: (offs[qiNow] ?? 0) / this.rate(),
          currentTrackDurationSeconds: this.lengthOf(qiNow) / this.rate(),
          durationSeconds: this.totalLength() / this.rate(),
        }));
      }
    }
    return taken;
  }

  private currentIndexAt(pos: number): number {
    const offs = this.offsets();
    let idx = 0;
    for (let i = 0; i < offs.length; i++) {
      if (pos >= (offs[i] ?? 0)) idx = i;
    }
    return idx;
  }

  // ---- transport ------------------------------------------------------------

  async togglePlay(): Promise<void> {
    const m = this.model.get();
    if (m.state === 'playing') {
      await this.pause();
    } else if (m.state === 'paused') {
      if (!this.engine && m.current) {
        // Restored session: rebuild the engine and seek to the saved spot
        // in one go (the click satisfies the browser's gesture requirement).
        const within =
          this.restoredWithin && this.restoredWithin.index === this.queue.get().index
            ? this.restoredWithin.withinSamples
            : undefined;
        await this.load(within);
        return;
      }
      await this.resume();
    } else if (m.state === 'ended' || m.state === 'idle' || m.state === 'error') {
      if (m.current) {
        this.pauseIntent = false;
        await this.load();
      }
    } else {
      await this.resume();
    }
  }

  async pause(): Promise<void> {
    ++this.transportSeq;
    this.pauseIntent = true;
    // NOTE: crossfadeInProgress is deliberately NOT cleared here. A pause
    // during a running fade freezes it (suspended context) and the handoff
    // must complete on resume; clearing would allow a second, overlapping
    // trigger that corrupts the live transition.
    this.setState('paused');
    this.gate().stop();
    await this.engine?.pause();
    this.persist();
  }

  async resume(): Promise<void> {
    if (!this.engine) {
      // Restored (or never-opened) session: togglePlay owns the load path.
      const m = this.model.get();
      if (m.state === 'paused' && m.current) await this.togglePlay();
      return;
    }
    const seq = ++this.transportSeq;
    const engine = this.engine;
    this.pauseIntent = false;
    this.gate().start();
    await engine.play();
    if (seq !== this.transportSeq || this.pauseIntent || this.engine !== engine) return;
    this.setState('playing');
  }

  async seek(seconds: number): Promise<void> {
    if (!this.engine || !this.model.get().current) return;
    ++this.transportSeq;
    let seq = ++this.loadingSeq;
    const posSamples = Math.max(0, Math.floor(seconds * this.rate()));
    const items = this.queue.get().items;
    if (items.length === 0) return;
    // Resolve the album-absolute target to a (track, within-track offset) and
    // seek there directly; a target in another track must switch to it rather
    // than clamping inside the current track.
    const offs = this.offsets();
    let qi = 0;
    for (let i = 0; i < items.length; i++) {
      if (posSamples >= (offs[i] ?? 0)) qi = i;
    }
    const base = offs[qi] ?? 0;
    const trackLen = Math.max(0, this.lengthOf(qi) - 1);
    const within = Math.min(Math.max(0, posSamples - base), trackLen);

    if (qi !== this.queue.get().index) {
      this.mutating = true;
      this.queue.moveTo(qi);
      this.mutating = false;
      if (!(await this.load())) return;
      seq = this.loadingSeq;
      // load() opened the target track, seeked to 0 and started pumping;
      // now move inside it (and re-align the offset bookkeeping) so the
      // reported position matches where the audio actually decodes from.
      if (qi === this.queue.get().index) {
        this.resetOffset = base + within;
        this.setState(this.pauseIntent ? 'paused' : 'buffering');
        this.model.update((m) => ({ ...m, positionSeconds: this.resetOffset / this.rate() }));
        await this.engine!.seekSample(within);
        if (seq !== this.loadingSeq) return;
        if (!this.pauseIntent) this.gate().start();
      }
      return;
    }

    this.resetOffset = base + within;
    this.setState(this.pauseIntent ? 'paused' : 'buffering');
    this.model.update((m) => ({ ...m, positionSeconds: this.resetOffset / this.rate() }));
    await this.engine.seekSample(within);
    if (seq !== this.loadingSeq) return;
    if (!this.pauseIntent) this.gate().start();
  }

  async stop(): Promise<void> {
    const seq = ++this.loadingSeq;
    ++this.transportSeq;
    const engine = this.engine;
    this.pauseIntent = true;
    this.gate().stop();
    this.pendingEnded = false;
    this.model.update((m) => ({ ...m, state: 'idle', positionSeconds: 0 }));
    await engine?.pause();
    if (seq !== this.loadingSeq || this.engine !== engine) return;
    await engine?.seekSample(0);
    this.persist();
  }

  /** Stops playback and disposes the engine + Media Session state. Used on
   *  sign-out / session expiry so audio never leaks across auth boundaries.
   *  The player stays usable: the next play rebuilds its engine. */
  async teardown(): Promise<void> {
    ++this.loadingSeq;
    ++this.transportSeq;
    this.pauseIntent = false;
    this.gate().stop();
    this.pendingEnded = false;
    this.lengths.clear();
    this.resetOffset = 0;
    this.restoredWithin = null;
    this.detachEngineListeners();
    await this.engine?.close();
    this.engine = null;
    this.engineKind = null;
    this.ports.mediaControls?.setMetadata(null);
    this.ports.storage.set(null);
    this.model.update((m) => ({
      ...m,
      state: 'idle',
      current: null,
      positionSeconds: 0,
      durationSeconds: 0,
      error: undefined,
    }));
    this.emit({ t: 'state', state: 'idle' });
    this.emit({ t: 'track', item: null });
  }

  // ---- settings --------------------------------------------------------------

  setVolume(v: number): void {
    const vol = Math.max(0, Math.min(1, v));
    this.model.update((m) => ({ ...m, volume: vol }));
    this.applyGain(this.model.get().current);
    this.persistThrottled();
  }

  setNormalizeMode(mode: NormalizationMode): void {
    this.model.update((m) => ({ ...m, normalizeMode: mode }));
    this.applyGain(this.model.get().current);
    this.persist();
  }

  /** Repeat mode: 'off' | 'one' | 'all'. Repeat-one reloads the current
   *  track at EOS (fresh load, sample-exact); repeat-all wraps the queue
   *  (standby target computed modulo length); 'off' ends at the last track. */
  setRepeat(mode: 'off' | 'one' | 'all'): void {
    this.queue.setRepeat(mode);
    this.syncPolicy();
    this.persist();
  }

  /** Shuffle: reorders presentation via the queue model's order policy.
   *  Timeline rebases on the NEXT load/handoff (order changes are user
   *  actions between tracks; no audible discontinuity is possible because
   *  the current track keeps playing and offsets only matter at boundaries). */
  setShuffle(on: boolean): void {
    this.queue.setShuffle(on);
    this.resetOffset = this.offsets()[this.queue.get().index] ?? 0;
    const m = this.model.get();
    if (m.current) {
      this.model.update((mm) => ({
        ...mm,
        currentTrackStartSeconds: this.resetOffset / this.rate(),
        positionSeconds: this.albumPosition() / this.rate(),
      }));
    }
    this.syncPolicy();
    this.persist();
  }

  /** Crossfade length at natural track boundaries. 0 disables (default).
   *  Only applies where an engine supports it; manual next/previous/seek
   *  never fade. Persisted in the session snapshot. */
  setCrossfade(seconds: number): void {
    const allowed = [0, 4, 8, 12];
    this.crossfadeSeconds = allowed.includes(seconds) ? seconds : 0;
    this.model.update((m) => ({ ...m, crossfadeSeconds: this.crossfadeSeconds }));
    this.emit({ t: 'crossfade', seconds: this.crossfadeSeconds });
    this.persist();
  }

  private syncPolicy(): void {
    this.model.update((m) => ({
      ...m,
      repeat: this.queue.repeat,
      shuffle: this.queue.shuffle,
    }));
    this.emit({ t: 'policy', repeat: this.queue.repeat, shuffle: this.queue.shuffle });
  }

  private applyGain(item: PlaybackItem | null): void {
    const m = this.model.get();
    const normDb = normalizationGainDb(m.normalizeMode, item?.loudness, item?.albumLoudness);
    const gain = combinedGain(m.volume, normDb);
    this.engine?.setGain(gain);
    this.model.update((mm) => ({ ...mm, normDb }));
    this.emit({ t: 'gain', normDb });
  }

  // ---- engine selection -----------------------------------------------------

  private async ensureEngine(kind: EngineKind): Promise<void> {
    if (this.engine && this.engineKind === kind) return;
    this.detachEngineListeners();
    await this.engine?.close();
    this.engine = null;
    this.engineKind = kind;
    const epoch = ++this.engineEpoch;
    const engine = this.ports.engineFactory(kind, {
      primed: () => this.withLiveEngine(epoch, () => this.onPrimed()),
      buffering: () => this.withLiveEngine(epoch, () => this.onBuffering()),
      eos: () => this.withLiveEngine(epoch, () => void this.onEos()),
      error: (msg) => this.withLiveEngine(epoch, () => this.fail(msg)),
      tick: () => this.withLiveEngine(epoch, () => this.tick()),
    });
    this.engine = engine;
    await engine.init(this.ports.token ? this.ports.token() : null);
  }

  /** Drops callbacks from engines that were closed/replaced (generalized
   *  engine-identity guard; plan §A.5 delivery rule 2). */
  private withLiveEngine(epoch: number, fn: () => void): void {
    if (epoch === this.engineEpoch) fn();
  }

  private detachEngineListeners(): void {
    ++this.engineEpoch;
    this.engineUnsubs.forEach((u) => u());
    this.engineUnsubs = [];
  }

  /** The decode gate capability, or a no-op gate when the engine lacks it. */
  private gate(): { start(): void; stop(): void } {
    const e = this.engine as unknown as { startPumping?: () => void; pausePumping?: () => void };
    if (typeof e?.startPumping === 'function' && typeof e?.pausePumping === 'function') {
      return { start: () => e.startPumping!(), stop: () => e.pausePumping!() };
    }
    return { start: () => undefined, stop: () => undefined };
  }

  private setMediaFor(item: PlaybackItem): void {
    this.ports.mediaControls?.setMetadata(item);
  }

  private fail(msg: string): void {
    this.gate().stop();
    this.model.update((m) => ({ ...m, state: 'error', error: msg }));
    // setState would double-write the model; emit the state event directly.
    this.emit({ t: 'state', state: 'error' });
    this.emit({ t: 'error', message: msg });
  }

  getEngineKind(): EngineKind | null {
    return this.engineKind;
  }

  /** Compressed bytes fetched by the demand reader (dev/perf instrumentation
   *  on engines that expose it). */
  getServedBytes(): number {
    const e = this.engine as unknown as { getServedBytes?: () => number };
    if (this.engine && (this.engineKind === 'musepack' || this.engineKind === 'rust')) {
      return e.getServedBytes?.() ?? 0;
    }
    return 0;
  }
}
