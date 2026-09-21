// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Web composition facade (M4). All transport/queue/position/persistence
// semantics live in the platform-independent Player (player-core/src/
// player.ts); this file only wires web adapters:
//
//   localStorage            → StoragePort (same key + throttle behavior)
//   navigator.mediaSession  → MediaControlsPort (metadata + track-relative
//                             position; incoming seekto converted by the core)
//   MusepackEngine | NativeBackend → Engine port via resolveKind
//
// The public surface is identical to the pre-M4 PlayerController so the
// Svelte components, bootstrap, e2e debug hooks and unit tests are unchanged.

import { writable } from '../store';
import { Player } from '../../../../player-core/src/player';
import type {
  MediaControlsPort,
  PlayerPorts,
  PlayerState as CorePlayerState,
  StoragePort,
} from '../../../../player-core/src/player';
import type { QueueModel } from '../../../../player-core/src/queue';
import type { Engine, EngineKind } from '../../../../player-core/src/engine';
import type { PlaybackItem } from '../../../../player-core/src/types';
import type { NormalizationMode } from './loudness';
import { bindMediaActions, setMediaMetadata, setMediaPosition } from './media-session';
import { browserSupportsMime, isMusepackCodec } from './capability';
import { MusepackEngine } from './musepack-engine';
import { NativeBackend } from './native-backend';
import { RustPlaybackEngine, type RustPlaybackAssets } from './rust-playback-engine';
import { repairingStorage } from './duration-repair-storage';
import type { QueueItem, QueueStore, SelectionContext } from '../state/queue';
import type { PlayerEvent } from '../../../../player-core/src/events';

export interface PlayerModel {
  state: CorePlayerState;
  current: QueueItem | null;
  positionSeconds: number;
  durationSeconds: number;
  /** Album-absolute start time of the current track. */
  currentTrackStartSeconds: number;
  /** Per-track duration in seconds, 0 when unknown. */
  currentTrackDurationSeconds: number;
  volume: number;
  normalizeMode: NormalizationMode;
  normDb: number;
  /** Queue playback policy (mirrored from the core model). */
  repeat: 'off' | 'one' | 'all';
  shuffle: boolean;
  /** Crossfade seconds at natural boundaries; 0 = off. */
  crossfadeSeconds: number;
  error?: string;
}
export type { CorePlayerState as PlayerState };

const STORAGE_KEY = 'musicpack.player.v1';

function localStoragePort(): StoragePort {
  return {
    get: () => (typeof localStorage === 'undefined' ? null : localStorage.getItem(STORAGE_KEY)),
    set: (value) => {
      if (typeof localStorage === 'undefined') return;
      if (value === null) localStorage.removeItem(STORAGE_KEY);
      else localStorage.setItem(STORAGE_KEY, value);
    },
  };
}

const mediaControls: MediaControlsPort = {
  bind(handlers) {
    bindMediaActions({
      play: handlers.play,
      pause: handlers.pause,
      next: handlers.next,
      previous: handlers.previous,
      seek: handlers.seek,
      seekBy: handlers.seekBy,
    });
  },
  setMetadata(item) {
    const qi = item as QueueItem | null;
    setMediaMetadata(qi, qi?.artworkUrl);
  },
  setPosition(durationSeconds, positionSeconds) {
    setMediaPosition(durationSeconds, positionSeconds);
  },
};

// ---- Playback backend selection -------------------------------------------
//
// Rust/WASM is the decode backend for every codec it supports — online
// (`http-range`) and offline (OPFS `local-file`) alike. The source kind no
// longer changes the engine: the same Rust decoder, driven by a synchronous
// range callback, reads HTTP bytes online and committed OPFS bytes offline.
//
// The frozen Emscripten Musepack decoder survives only as a differential
// *oracle* (`docs/adr/0013-web-offline-rust-playback.md`). It is never
// selected by a URL parameter or any product setting: the only selector is a
// session-scoped key that exists purely for the differential test lane (and
// the in-memory seam below for unit tests). Browser-native `<audio>` remains
// the backend for codecs the Rust engine does not decode.

/** Test-only oracle selector (session-scoped, survives reloads). */
const ORACLE_DECODER_KEY = 'musicpack.oracle-decoder.v1';

/** In-memory test seam; null = consult the session key. */
let oracleOverride: boolean | null = null;

/** Selects the frozen-Emscripten oracle decoder (tests only). Pass `null` to
 *  fall back to the session key. This is a test seam, not a product toggle. */
export function setOracleDecoder(on: boolean | null): void {
  oracleOverride = on;
}

/** True when the oracle lane is active (test/diagnostic accessor). */
export function oracleDecoderEnabled(): boolean {
  if (oracleOverride !== null) return oracleOverride;
  try {
    if (typeof sessionStorage !== 'undefined') {
      const value = sessionStorage.getItem(ORACLE_DECODER_KEY);
      return value === '1' || value === 'true' || value === 'on';
    }
  } catch {
    /* storage unavailable (private mode/SSR) */
  }
  return false;
}

/** Codecs the Rust demand-range engine can decode in-browser. This is a
 *  backend-capability predicate, not representation policy: WHICH source
 *  plays is already decided by the queue's representation selection; this
 *  only decides which engine plays the chosen source. */
function rustDecodesCodec(codec?: string): boolean {
  return isMusepackCodec(codec) || codec === 'flac' || codec === 'wav';
}

/**
 * Backend selection (web policy). Rust is the backend for the codecs it
 * decodes, for network and offline sources alike. Two residual lanes:
 *   - the test-only oracle lane (frozen Emscripten decoder) for differential
 *     runs — never selected in production;
 *   - codecs Rust cannot decode: the browser-native backend.
 * Representation selection is untouched.
 */
export function chooseBackend(item: PlaybackItem): EngineKind {
  if (!oracleDecoderEnabled() && rustDecodesCodec(item.codec)) return 'rust';
  if (isMusepackCodec(item.codec)) return 'musepack';
  if (browserSupportsMime(item.mimeType)) return 'native';
  throw new Error('This format is not supported by this browser.');
}

export interface HandlerSet {
  primed(): void;
  buffering(): void;
  eos(): void;
  error(message: string): void;
  tick(): void;
}

/** Engine kinds the web composition root can construct. `chooseBackend`
 *  yields 'rust' for every supported codec and 'native' for browser-decodable
 *  codecs. 'musepack' is the frozen-decoder **oracle** lane, reachable only
 *  through the test hook (`setOracleDecoder`), never from product config. */
export type WebEngineKind = 'musepack' | 'native' | 'rust';

/** Constructs an engine for a resolved kind. Exported so the Rust adapter can
 *  be built through the exact seam the controller uses (test hook only). */
export function createWebEngine(
  kind: WebEngineKind,
  h: HandlerSet,
  rustAssets?: RustPlaybackAssets,
): Engine {
  if (kind === 'rust') {
    return new RustPlaybackEngine({
      handlers: {
        primed: h.primed,
        buffering: h.buffering,
        eos: h.eos,
        error: h.error,
        tick: () => h.tick(),
      },
      assets: rustAssets,
    });
  }
  if (kind === 'musepack') {
    return new MusepackEngine({
      primed: h.primed,
      buffering: h.buffering,
      eos: h.eos,
      error: h.error,
      tick: () => h.tick(),
    });
  }
  const native = new NativeBackend();
  native.onPrimed = h.primed;
  native.onBuffering = h.buffering;
  native.onEos = h.eos;
  native.onError = h.error;
  native.onPosition = h.tick;
  return native;
}

export interface ControllerOptions {
  token?: () => string | null;
  initialVolume?: number;
  initialNormalize?: NormalizationMode;
  /** Test seam: replace engine construction. */
  backendFactory?: (kind: EngineKind, events: HandlerSet) => Engine;
  /** Storage for cross-reload player persistence (defaults to localStorage). */
  storage?: { get: () => string | null; set: (value: string | null) => void };
  /** Content-aware transition policy (Sweet Fades); see transition-profiles. */
  planTransition?: PlayerPorts['planTransition'];
  /** Representation-selection context provider (Phase 4): consulted at item
   *  construction time so PlaybackItems carry the preferred audio source.
   *  Omitted ⇒ default-only behavior. */
  selection?: () => SelectionContext;
}

interface InternalControllerOptions extends ControllerOptions {
  /** Test seam (unit tests): inject the whole ports object. When present it
   *  overrides engineFactory/storage below. */
  portsOverride?: Partial<PlayerPorts>;
}

export class PlayerController {
  readonly model = writable<PlayerModel>({
    state: 'idle',
    current: null,
    positionSeconds: 0,
    durationSeconds: 0,
    currentTrackStartSeconds: 0,
    currentTrackDurationSeconds: 0,
    volume: 0.8,
    normalizeMode: 'album',
    normDb: 0,
    repeat: 'off',
    shuffle: false,
    crossfadeSeconds: 0,
  });
  private core: Player;
  private queue: QueueStore;
  private readonly selection?: () => SelectionContext;

  constructor(queue: QueueStore, opts: ControllerOptions = {}) {
    this.queue = queue;
    this.selection = opts.selection;
    const o = opts as InternalControllerOptions;
    const ports: PlayerPorts = {
      engineFactory: (kind, handlers) =>
        o.portsOverride?.engineFactory
          ? o.portsOverride.engineFactory(kind, handlers)
          : opts.backendFactory
            ? opts.backendFactory(kind, handlers)
            : createWebEngine(kind, handlers),
      resolveKind: chooseBackend,
      token: opts.token ?? (() => null),
      storage:
        (opts.storage as StoragePort | undefined) ??
        o.portsOverride?.storage ??
        repairingStorage(localStoragePort()),
      mediaControls,
      // Host-owned clock + timer (core purity law 2). Looked up dynamically
      // so fake timers in tests intercept these as intended.
      now: () => Date.now(),
      schedulePersist: (fn, ms) => setTimeout(fn, ms),
      clearPersistTimer: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
    };
    this.core = new Player(
      queue as unknown as QueueModel,
      o.portsOverride
        ? { ...ports, ...o.portsOverride }
        : { ...ports, planTransition: opts.planTransition },
      { initialVolume: opts.initialVolume, initialNormalize: opts.initialNormalize },
    );    // Mirror the core model into the UI-facing writable with QueueItem-typed
    // `current` (the web queue only ever holds QueueItems). The model stays
    // the full state snapshot; M7 events are exposed alongside for event-
    // driven consumers (media controls, future scrobbling/analytics).
    this.core.model.subscribe((m) => this.model.set(m as PlayerModel));
  }

  /** Subscribe to typed player events (M7 integration surface). Returns an
   *  unsubscribe function. Events: state / track / position / policy /
   *  gain / error — see player-core/src/events.ts. */
  on(fn: (event: PlayerEvent) => void): () => void {
    return this.core.on(fn);
  }

  init(): void {
    this.core.init();
  }

  destroy(): void {
    this.core.destroy();
  }

  // ---- queue actions --------------------------------------------------------

  async playItem(item: QueueItem): Promise<void> {
    await this.core.playItem(item);
  }

  /** Jumps to an existing queue item without touching the rest of the queue
   *  (the queue-panel click). Deliberately NOT playItem(): replacing the
   *  queue would throw away everything the user had lined up. */
  async playQueueIndex(i: number): Promise<void> {
    await this.core.playQueueIndex(i);
  }

  async playAlbum(
    release: Parameters<QueueStore['playAlbum']>[0],
    title: string,
    artist: string,
    startIndex = 0,
  ): Promise<void> {
    // Historical flow preserved verbatim: the CONTROLLER performs the queue
    // mutation (flagged internal), then loads.
    await this.core.playSequence(this.queueItemsFor(release, title, artist), title, artist, startIndex);
  }

  /** The core Player mutates the generic QueueModel; playAlbum needs the
   *  built items BEFORE handing them over, so build + install here through
   *  the store's builder while still routing the install through the core. */
  private queueItemsFor(
    release: Parameters<QueueStore['playAlbum']>[0],
    title: string,
    artist: string,
  ): QueueItem[] {
    // Build items without installing: reuse the store's public builder by
    // calling playAlbum on a THROWAWAY probe? No — simpler: the store keeps
    // its builder; expose it as a pure function instead. See state/queue.ts
    // `itemsForRelease` export used here. Representation selection (Phase 4)
    // rides along through the injected context provider.
    return itemsForRelease(release, title, artist, this.selection?.());
  }

  async next(): Promise<void> {
    await this.core.next();
  }

  async previous(): Promise<void> {
    await this.core.previous();
  }

  // ---- transport -------------------------------------------------------------

  async togglePlay(): Promise<void> {
    await this.core.togglePlay();
  }

  async pause(): Promise<void> {
    await this.core.pause();
  }

  async resume(): Promise<void> {
    await this.core.resume();
  }

  async seek(seconds: number): Promise<void> {
    await this.core.seek(seconds);
  }

  async stop(): Promise<void> {
    await this.core.stop();
  }

  /** Stops playback and disposes the backend + Media Session state. Used on
   *  sign-out / session expiry so audio never leaks across auth boundaries.
   *  The controller stays usable: the next play rebuilds its backend. */
  async teardown(): Promise<void> {
    await this.core.teardown();
  }

  // ---- settings ---------------------------------------------------------------

  setVolume(v: number): void {
    this.core.setVolume(v);
  }

  setNormalizeMode(mode: NormalizationMode): void {
    this.core.setNormalizeMode(mode);
  }

  setRepeat(mode: 'off' | 'one' | 'all'): void {
    this.core.setRepeat(mode);
  }

  setShuffle(on: boolean): void {
    this.core.setShuffle(on);
  }

  setCrossfade(seconds: number): void {
    this.core.setCrossfade(seconds);
  }

  getBackendKind(): 'musepack' | 'native' | 'rust' | null {
    return this.core.getEngineKind();
  }

  /** Compressed bytes fetched by the demand reader (dev/perf instrumentation). */
  getServedBytes(): number {
    return this.core.getServedBytes();
  }
}

// Imported late to avoid a cycle at module-eval time (state/queue imports
// nothing from playback).
import { itemsForRelease } from '../state/queue';
