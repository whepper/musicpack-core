// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Music Similarity client state (Slice 0, ADR 0017 §5.9). A store alongside
// the library store with an explicit `unavailable` state: capability absence
// is a first-class, expected condition — never an empty result that reads as
// "nothing is similar" (FORMAT_SPEC §13, ADR 0017 §5.8).
//
// The store owns the capability probe and the per-track similar-tracks
// fetch. It knows nothing about the embedding model: it consumes the
// `{track, release, score, rank}` list the server returns.

import { writable } from '../store';
import type { ApiClient } from '../api/client';
import type { SimilarTrack, SimilarityStatus } from '../api/types';

/** Capability state. `unavailable` is the explicit absence state the server
 *  reports when there is no active non-empty similarity index. */
export type SimilarityCapability =
  | { state: 'loading' }
  | { state: 'available'; status: SimilarityStatus }
  | { state: 'unavailable'; status: SimilarityStatus }
  | { state: 'error'; message: string };

/** Per-track similar-tracks state. Distinct from the capability: a track can
 *  be in an available index yet have no vector of its own. */
export type SimilarTracksState =
  | { state: 'idle' }
  | { state: 'loading' }
  | { state: 'ready'; neighbors: SimilarTrack[]; profileId?: string }
  | { state: 'unavailable' }
  | { state: 'error'; message: string };

export function createSimilarityStore(api: ApiClient) {
  const capability = writable<SimilarityCapability>({ state: 'loading' });
  const tracks = writable<SimilarTracksState>({ state: 'idle' });

  async function loadStatus(): Promise<void> {
    capability.set({ state: 'loading' });
    try {
      const status = await api.similarityStatus();
      capability.set(
        status.available
          ? { state: 'available', status }
          : { state: 'unavailable', status },
      );
    } catch (e) {
      capability.set({
        state: 'error',
        message: e instanceof Error ? e.message : 'Could not load similarity status.',
      });
    }
  }

  /** Fetch similar tracks for one track. A 404 `similarity_unavailable`
   *  (no index, or this track has no vector) maps to the explicit
   *  `unavailable` state — never to an empty list. */
  async function loadSimilar(trackId: number | string): Promise<void> {
    tracks.set({ state: 'loading' });
    try {
      const res = await api.trackSimilar(trackId);
      tracks.set({
        state: 'ready',
        neighbors: res.neighbors,
        profileId: res.profileId,
      });
    } catch (e) {
      const code =
        e instanceof Error && 'code' in e ? (e as { code?: string }).code : undefined;
      if (code === 'similarity_unavailable') {
        tracks.set({ state: 'unavailable' });
        return;
      }
      tracks.set({
        state: 'error',
        message: e instanceof Error ? e.message : 'Could not load similar tracks.',
      });
    }
  }

  /** Load the capability probe and, when available, the per-track neighbours.
   *  When the capability is unavailable the tracks state is set to the
   *  explicit `unavailable` state so the section never reads as "loading"
   *  forever and never as an empty result. */
  async function loadForTrack(trackId: number | string): Promise<void> {
    await loadStatus();
    if (capability.get().state === 'available') {
      await loadSimilar(trackId);
    } else {
      tracks.set({ state: 'unavailable' });
    }
  }

  return {
    capability,
    tracks,
    loadStatus,
    loadSimilar,
    loadForTrack,
  };
}

export type SimilarityStore = ReturnType<typeof createSimilarityStore>;
