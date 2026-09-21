// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Application composition root: wires the API client, session, stores,
// playback controller and router into singletons the Svelte components use.
import { ApiClient } from './api/client';
import { bindSession, setOfflineContentProbe, type SessionStore } from './auth/session';
import { browserCanPlay } from './playback/capability';
import { createLibraryStore, type LibraryStore } from './state/library';
import { createAudioPreferenceStore } from './state/preferences';
import {
  createQueueStore,
  type QueueStore,
  type SelectionContext,
} from './state/queue';
import {
  PlayerController,
  createWebEngine,
  oracleDecoderEnabled,
  setOracleDecoder,
} from './playback/controller';
import type { RustPlaybackAssets, RustPlaybackEngine } from './playback/rust-playback-engine';
import { createTransitionPlanner } from './playback/transition-profiles';
import { createRouter, type Router } from './router';
import { createOfflineManager } from './offline/manager';
import { offlineAwareStorage } from './offline/snapshot-storage';
import { repairingStorage } from './playback/duration-repair-storage';

export const api = new ApiClient({});
export const session: SessionStore = bindSession(api);
export const library: LibraryStore = createLibraryStore(api);
// Representation preference (Phase 4): persisted web-app setting; selection
// happens only at itemForTrack() construction time.
export const audioPreference = createAudioPreferenceStore();
// Offline downloads (D1 local-first): the manager owns catalog/storage
// lifecycle; itemForTrack() applies the local-first source rule through
// SelectionContext.offline AFTER resolveAudio() has chosen the candidate.
// canPlay remains pure browser capability — the two concerns compose at
// this single composition point without a second selection policy.
export const offline = createOfflineManager();
/** Reactive per-release offline UI states (install progress, stale/damaged
 *  badges). Exported for components that need to re-derive on changes. */
export const offlineStates = offline.states;
/** The one SelectionContext source both item-construction paths share. */
export function currentSelection(): SelectionContext {
  return {
    preference: audioPreference.get(),
    canPlay: browserCanPlay,
    offline: offline.enabled
      ? {
          localKeyFor: (trackId, candidate) =>
            offline.availability.localKeyFor(trackId, candidate),
        }
      : undefined,
  };
}
export const queue: QueueStore = createQueueStore({ selection: currentSelection });
// Content-aware transitions: profiles come from the tracks' waveform
// envelopes; without data the planner degrades to the legacy fixed fade.
const transitionPlanner = createTransitionPlanner({
  base: api.baseUrl,
  token: () => api.getToken(),
});
export const player = new PlayerController(queue, {
  planTransition: (query) => transitionPlanner.plan(query),
  selection: currentSelection,
  // Restored sessions play installed content locally (D1) without the
  // core learning anything about offline state. Repair of hint-less
  // durations composes INSIDE the offline remap: the transforms are
  // disjoint (hints vs source), so nesting order is irrelevant.
  storage: offline.enabled
    ? offlineAwareStorage(
        repairingStorage({
          get: () => (typeof localStorage === 'undefined' ? null : localStorage.getItem('musicpack.player.v1')),
          set: (v) => {
            if (typeof localStorage === 'undefined') return;
            if (v === null) localStorage.removeItem('musicpack.player.v1');
            else localStorage.setItem('musicpack.player.v1', v);
          },
        }),
        { localKeyFor: (trackId, candidate) => offline.availability.localKeyFor(trackId, candidate) },
      )
    : undefined,
});
// Offline subsystem boot: hydrate availability + sweep orphaned staging.
void offline.init();
// Offline session probe: lets the boot path distinguish "signed out" from
// "network gone but installed content available" (AuthState 'offline').
setOfflineContentProbe(() => offline.availability.hasInstalled());
// Reconnect (offline UX): when connectivity returns during an 'offline'
// session, re-probe. session.reattempt() contractually never demotes, so
// flapping online events are harmless. navigator.onLine is still never
// consulted at BOOT (unchanged policy). A slow interval covers the
// Chromium case where a page booted while offline never receives the
// 'online' event when connectivity returns.
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => {
    void session.reattempt();
  });
  const RECONNECT_PROBE_MS = 30_000;
  const reconnectTimer = setInterval(() => {
    if (session.get().state !== 'offline') {
      clearInterval(reconnectTimer);
      return;
    }
    void session.reattempt();
  }, RECONNECT_PROBE_MS);
}
// Prefetch boundary profiles for the current and next item as playback
// advances so plans are content-aware by the time a boundary approaches.
player.on((event) => {
  if (event.t !== 'track') return;
  const q = queue.get();
  const current = q.items[q.index] ?? null;
  if (current) transitionPlanner.prime(current);
  const next = q.items[q.index + 1] ?? null;
  if (next) transitionPlanner.prime(next);
});
/** The player model as a store (subscribe via `$playerModel`). */
export const playerModel = player.model;
export const router: Router = createRouter();

// Stop playback and dispose the backend when the session ends (sign-out or
// expiry), so audio and Media Session state never leak across the auth
// boundary. The controller rebuilds its backend on the next play.
// 'offline' is a degraded-authenticated state: installed content stays
// playable, so playback is NOT torn down on entering it.
{
  let wasAuthenticated = false;
  session.subscribe((m) => {
    const usable = m.state === 'authenticated' || m.state === 'offline';
    if (wasAuthenticated && !usable) {
      void player.teardown();
      queue.clear();
    }
    wasAuthenticated = usable;
  });
}

/** Test/dev instrumentation exposed for Playwright + perf reporting. */
export interface MusicPackDebug {
  api: ApiClient;
  session: SessionStore;
  library: LibraryStore;
  queue: QueueStore;
  player: PlayerController;
  router: Router;
  audioPreference: ReturnType<typeof createAudioPreferenceStore>;
  offline: ReturnType<typeof createOfflineManager>;
  /** The decoder lane this page uses. `'rust'` is the only product backend
   *  for supported codecs; `'legacy'` means the test-only frozen-Emscripten
   *  differential lane was selected through the debug hook. The concrete
   *  per-item engine kind is `player.getBackendKind()`. */
  readonly playbackBackend: 'rust' | 'legacy';
  /** Test-only: selects the frozen Emscripten decoder for differential runs.
   *  Not a product setting and not reachable from URL/localStorage. */
  selectOracleDecoder(on: boolean): void;
  /** Phase 14H-1 test hook: builds the Rust playback adapter through the same
   *  `createWebEngine` seam the controller uses. Lower-level tests only; the
   *  Phase 14H-2 Rust scenarios run through `player`. */
  createRustEngine: (assets?: RustPlaybackAssets) => RustPlaybackEngine;
}

declare global {
  interface Window {
    __musicpack?: MusicPackDebug;
  }
}

const rustEngineHandlers = {
  primed: () => undefined,
  buffering: () => undefined,
  eos: () => undefined,
  error: () => undefined,
  tick: () => undefined,
};

export function exposeDebug(): void {
  window.__musicpack = {
    api,
    session,
    library,
    queue,
    player,
    router,
    audioPreference,
    offline,
    get playbackBackend() {
      return oracleDecoderEnabled() ? 'legacy' : 'rust';
    },
    selectOracleDecoder: (on: boolean) => setOracleDecoder(on),
    createRustEngine: (assets?: RustPlaybackAssets) =>
      createWebEngine('rust', rustEngineHandlers, assets) as RustPlaybackEngine,
  };
}
