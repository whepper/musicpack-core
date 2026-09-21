// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Lyrics controller (R3.4, docs/musicpack-lyrics-v1.md §10) — the one
// place that turns a track's server-side `lyrics[]` references into a
// render-ready view:
//
//   refs (track detail; first entry wins per spec §5)
//     → asset bytes (the existing /api/v1/assets/{id} contract)
//     → wasm core parse (lyrics_open / lyrics_doc)
//     → per-tick active line (lyrics_active_line, spec §9)
//
// Framework-free and unit-testable: the network and the wasm binding
// enter through injected ports; the view is a plain snapshot exposed as
// a store. No timers — the position feed is pushed in by the host
// (§9: "the lyrics controller subscribes; it never runs a timer").
// Lyrics are auxiliary content: every failure degrades to the `error`
// view, never to a thrown error that could touch playback.
//
// Staleness/races (§6): `setTrack` bumps an install token; every async
// continuation re-checks it before committing, so a slow response for a
// previous track can never overwrite the current view. Within a track
// nothing refetches; parsed documents are memoized by asset identity
// (`id:sha256`) across tracks for the session — the same semantics the
// server's stable ids + strong ETags provide on the wire.

import { writable } from 'svelte/store';
import type { LyricsRef } from '../api/types';

export type LyricsStateKind = 'no-lyrics' | 'loading' | 'plain' | 'synced' | 'error';

/** One renderable lyric line. `timestampMs` is present only for synced
 *  documents (already normalized by the core; i64 ms per spec §8). */
export interface LyricsLine {
  text: string;
  timestampMs?: number;
}

/** The render-ready view (spec §10: `{state, lines, activeIndex,
 *  isSynced}`). `activeIndex` is −1 when no line applies (before the
 *  first timestamp, plain documents, or no position feed). */
export interface LyricsView {
  state: LyricsStateKind;
  lines: LyricsLine[];
  activeIndex: number;
  isSynced: boolean;
  /** The selected reference's language tag, when the server supplied one. */
  lang?: string;
  /** Error detail for the `error` state (rendered as text, not color). */
  message?: string;
}

const NO_LYRICS: LyricsView = { state: 'no-lyrics', lines: [], activeIndex: -1, isSynced: false };

/** The wasm-core port (see lib/playback/lyrics-core.ts). */
export interface LyricsCore {
  /** Parses LRC bytes under the strict profile; returns a handle.
   *  Throws on malformed input. */
  open(bytes: Uint8Array): number;
  /** The document's static content as canonical JSON. */
  doc(handle: number): string;
  /** Normative timing lookup (spec §9): active line index or −1. */
  activeLine(handle: number, positionMs: number): number;
  /** Releases a document; closed handles must not be used again. */
  close(handle: number): void;
}

/** Fetches one reference's bytes through the existing asset endpoint. */
export type LyricsFetcher = (ref: LyricsRef) => Promise<Uint8Array>;

export interface LyricsControllerOptions {
  fetchAsset: LyricsFetcher;
  /** Resolves the wasm binding (lazily; first use triggers the load). */
  core: () => Promise<LyricsCore>;
  /** Parsed-document memo capacity (session cache, oldest dropped). */
  cacheCapacity?: number;
}

/** A parsed, wasm-resident document plus its static projection. Owned by
 *  the session cache; `handle` is closed on eviction or dispose. */
interface InstalledDoc {
  isSynced: boolean;
  lines: LyricsLine[];
  handle: number;
}

/** Parsed doc JSON shape (musicpack-wasm `LyricsDocs::doc`). */
interface DocJson {
  synced: number;
  lines: Array<{ t?: number; text: string }>;
}

/** §9 position mapping: the player model's album-absolute position
 *  reduced to the current track's milliseconds (floor). `null` when the
 *  playing track is not `trackId` (the document renders without an
 *  active line — the same `isCurrent` rule the waveform seek uses). */
export function withinTrackMs(
  snapshot: {
    currentTrackId: number | undefined;
    currentTrackStartSeconds: number;
    currentTrackDurationSeconds: number;
    positionSeconds: number;
  },
  trackId: number | null,
): number | null {
  if (trackId === null || snapshot.currentTrackId !== trackId) return null;
  const within = Math.max(
    0,
    Math.min(
      snapshot.currentTrackDurationSeconds,
      snapshot.positionSeconds - snapshot.currentTrackStartSeconds,
    ),
  );
  // Spec §9: seconds → ms is floor (`(p_seconds * 1000) | 0`).
  return (within * 1000) | 0;
}

export class LyricsController {
  private readonly store = writable<LyricsView>(NO_LYRICS);
  private readonly opts: LyricsControllerOptions;
  /** Memoized parsed documents keyed by asset identity (`id:sha256`). */
  private readonly cache = new Map<string, InstalledDoc>();
  /** Monotonic install token — staleness guard for async continuations. */
  private install = 0;
  private trackId: number | null = null;
  private refs: LyricsRef[] = [];
  /** Handle of the adopted (currently rendered) document, if any. */
  private handle: number | null = null;
  /** Last active index written to the store (per-tick churn guard). */
  private lastIndex = -2;
  /** The resolved wasm core (non-null once an install reached the core). */
  private coreInstance: LyricsCore | null = null;
  private corePromise: Promise<LyricsCore> | null = null;

  constructor(opts: LyricsControllerOptions) {
    this.opts = opts;
  }

  subscribe = this.store.subscribe;

  get(): LyricsView {
    let v: LyricsView = NO_LYRICS;
    this.store.subscribe((s) => (v = s))();
    return v;
  }

  /** Installs the references for a track (from the track-detail payload).
   *  Bumps the install token first, so any in-flight work for the
   *  previous track drops itself instead of overwriting this view. */
  setTrack(trackId: number | null, refs: LyricsRef[] | undefined): void {
    this.install += 1;
    this.trackId = trackId;
    this.refs = refs && refs.length > 0 ? [...refs] : [];
    this.handle = null;
    this.lastIndex = -2;
    if (this.trackId === null || this.refs.length === 0) {
      this.store.set(NO_LYRICS);
      return;
    }
    void this.install_(this.install, this.trackId, this.refs);
  }

  /** Explicit refetch — the error state's retry affordance. */
  retry(): void {
    if (this.trackId === null || this.refs.length === 0) return;
    this.install += 1;
    void this.install_(this.install, this.trackId, this.refs);
  }

  /** Position feed (§9). Recomputes the active line of the adopted
   *  synced document; a no-op for plain/loading/error views and before
   *  the wasm core is up. The store updates only when the index changes
   *  — the guard precedes the store write, because a Svelte store emits
   *  for every object set, tick after tick. */
  setPositionMs(positionMs: number): void {
    const core = this.coreInstance;
    if (core === null || this.handle === null) return;
    const index = core.activeLine(this.handle, positionMs);
    if (index === this.lastIndex) return;
    this.lastIndex = index;
    this.store.update((v) => ({ ...v, activeIndex: index }));
  }

  /** Releases every cached document and returns to the no-lyrics view. */
  dispose(): void {
    this.install += 1;
    this.trackId = null;
    this.refs = [];
    this.handle = null;
    for (const doc of this.cache.values()) this.coreInstance?.close(doc.handle);
    this.cache.clear();
    this.store.set(NO_LYRICS);
  }

  // ---- internals ---------------------------------------------------------

  /** The wasm binding, resolved once; load failures become the error view. */
  private core(): Promise<LyricsCore> {
    this.corePromise ??= this.opts.core();
    return this.corePromise;
  }

  private async install_(install: number, trackId: number, refs: LyricsRef[]): Promise<void> {
    // Selection (spec §5): the first entry in server/manifest order —
    // deterministic; preference-driven selection is future work (§18).
    const ref = refs[0];
    if (!ref) {
      this.store.set(NO_LYRICS);
      return;
    }
    const fresh = () => install === this.install && this.trackId === trackId;
    this.store.set({ state: 'loading', lines: [], activeIndex: -1, isSynced: false, lang: ref.lang });

    const key = `${ref.id}:${ref.sha256 ?? ''}`;
    const cached = this.cache.get(key);
    if (cached) {
      if (!fresh()) return;
      this.adopt(cached, ref.lang);
      return;
    }

    try {
      const bytes = await this.opts.fetchAsset(ref);
      if (!fresh()) return;
      const core = await this.core();
      this.coreInstance = core;
      if (!fresh()) return;
      const handle = core.open(bytes);
      let doc: DocJson;
      try {
        doc = JSON.parse(core.doc(handle)) as DocJson;
      } catch (e) {
        core.close(handle);
        throw e;
      }
      if (!fresh()) {
        core.close(handle);
        return;
      }
      const lines: LyricsLine[] = (doc.lines ?? []).map((l) =>
        typeof l.t === 'number' ? { text: String(l.text), timestampMs: l.t } : { text: String(l.text) },
      );
      const installed: InstalledDoc = { isSynced: doc.synced === 1, lines, handle };
      this.remember(key, installed);
      this.adopt(installed, ref.lang);
    } catch (e) {
      if (!fresh()) return;
      this.store.set({
        state: 'error',
        lines: [],
        activeIndex: -1,
        isSynced: false,
        lang: ref.lang,
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }

  /** Adopts an installed document as the current view (handle stays
   *  cache-owned; `setPositionMs` drives its active line). */
  private adopt(doc: InstalledDoc, lang?: string): void {
    this.handle = doc.handle;
    this.lastIndex = -2; // force the first position write to emit
    this.store.set({
      // A parsed document with zero lines (an empty file) renders as an
      // empty plain document — a degenerate document is content, not a
      // failure (§7: no noisy errors for normal absence).
      state: doc.isSynced ? 'synced' : 'plain',
      lines: doc.lines,
      activeIndex: -1,
      isSynced: doc.isSynced,
      lang,
    });
  }

  private remember(key: string, doc: InstalledDoc): void {
    const capacity = this.opts.cacheCapacity ?? 48;
    while (this.cache.size >= capacity) {
      const oldest = this.cache.keys().next().value;
      if (oldest === undefined) break;
      const evicted = this.cache.get(oldest);
      this.cache.delete(oldest);
      if (evicted && evicted.handle !== this.handle) this.coreInstance?.close(evicted.handle);
    }
    this.cache.set(key, doc);
  }
}
