// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Web queue store (M3): a thin adapter over the platform-independent
// QueueModel (player-core/src/queue.ts). All semantics live in the core;
// this file adds only the web item construction from API types.
//
// QueueItem = core PlaybackItem plus the API `Track` verbatim and release
// identity. The core fields are the player-facing identity/source/metadata
// view; the `track` field remains until the orchestrator extraction (M4)
// moves all readers onto the core shape.

import { createQueueModel, type QueueModel } from '../../../../player-core/src/queue';
import type { PlaybackItem } from '../../../../player-core/src/types';
import type { AlbumLoudness, ReleaseDetail, RepresentationRef, Track } from '../api/types';
import {
  acceptAll,
  resolveAudio,
  type AudioPreference,
  type CanPlay,
} from './representation-selection';

export interface QueueItem extends PlaybackItem {
  track: Track;
  releaseId: number;
  albumId: number;
  /** Set iff an alternate representation was selected for this item
   *  (Phase 4); absent = the track's primary audio. */
  representationId?: number;
}

/** Everything itemForTrack() needs to resolve audio: the active preference
 *  plus the host playability predicate. Omitted entirely ⇒ pre-Phase-4
 *  behavior, byte-identical. `offline` adds the D1 local-first source rule
 *  (installed candidates resolve to local-file sources). */
export interface SelectionContext {
  preference?: AudioPreference;
  canPlay?: CanPlay;
  offline?: OfflineContext;
}

export interface QueueState {
  items: QueueItem[];
  /** Index of the current item (-1 when the queue is empty). */
  index: number;
}

/** Builds the play sequence for a release in media/disc-major, track order. */
export function tracksOfRelease(release: ReleaseDetail): Track[] {
  const out: Track[] = [];
  for (const disc of [...release.media].sort((a, b) => a.disc - b.disc)) {
    for (const track of [...disc.tracks].sort((a, b) => a.number - b.number)) {
      out.push(track);
    }
  }
  return out;
}

/** Optional offline context (D1 local-first): when provided, itemForTrack()
 *  consults it AFTER representation selection so the chosen candidate can
 *  be served from the committed download catalog instead of the network.
 *  Omitted entirely ⇒ online-only behavior, byte-identical. */
export interface OfflineContext {
  /** Local file key for a locally-held candidate, or null. */
  localKeyFor(trackId: number, candidate: { kind: 'primary' } | { kind: 'representation'; id: number }): string | null;
}

function buildSource(
  track: Track,
  rep: RepresentationRef | null,
  offline?: OfflineContext,
): PlaybackItem['source'] {
  const url = rep ? rep.url : track.audio.url;
  const byteSize = rep ? rep.size : track.audio.size;
  if (offline) {
    const key = offline.localKeyFor(
      track.id,
      rep ? { kind: 'representation', id: rep.id } : { kind: 'primary' },
    );
    if (key !== null) return { kind: 'local-file', url: key, byteSize };
  }
  return { kind: 'http-range', url, byteSize };
}

/** Builds a single web QueueItem for one track of a release. Shared by the
 *  album builders and the per-track add-to-queue action so item construction
 *  AND representation selection (Phase 4) live in exactly one place. Without
 *  `sel` the output is byte-identical to the pre-Phase-4 default-only
 *  behavior. */
export function itemForTrack(
  release: ReleaseDetail,
  track: Track,
  title: string,
  artist: string,
  sel?: SelectionContext,
): QueueItem {
  const artworkUrl = release.artwork[0]?.url;
  const chosen = resolveAudio(track, sel?.preference, sel?.canPlay ?? acceptAll);
  const rep = chosen.representation;
  return {
    // core PlaybackItem fields (identity, source, policy metadata)
    id: rep ? `t${track.id}r${rep.id}` : `t${track.id}`,
    trackId: track.id,
    source: buildSource(track, rep, sel?.offline),
    durationHintSeconds: track.duration,
    title: track.title,
    artist,
    albumTitle: title,
    edition: release.edition,
    artworkUrl,
    loudness: track.loudness,
    albumLoudness: release.loudness,
    codec: rep ? rep.codec.codec : track.codec.codec,
    mimeType: rep ? rep.codec.mimeType : track.codec.mimeType,
    ...(rep ? { representationId: rep.id } : {}),
    // web-specific fields
    track,
    releaseId: release.id,
    albumId: release.album.id,
  };
}

function itemsFor(
  release: ReleaseDetail,
  title: string,
  artist: string,
  sel?: SelectionContext,
): QueueItem[] {
  return tracksOfRelease(release).map((track) =>
    itemForTrack(release, track, title, artist, sel),
  );
}

/** Pure builder (M4): constructs web QueueItems for a release without
 *  installing them — used by the playback facade's playAlbum. */
export function itemsForRelease(
  release: ReleaseDetail,
  title: string,
  artist: string,
  sel?: SelectionContext,
): QueueItem[] {
  return itemsFor(release, title, artist, sel);
}

/** The web queue store: QueueModel methods (delegated verbatim) plus the
 *  web-shaped playAlbum builder. NOTE: `{...model}` would snapshot the
 *  model's getters as static values (repeat/shuffle would freeze at their
 *  initial state), so the policy surface is forwarded explicitly. */
export function createQueueStore(opts: { selection?: () => SelectionContext } = {}) {
  // Hosts own randomness (core purity law 2): the web passes Math.random.
  const model = createQueueModel<QueueItem>({ rng: Math.random });
  return {
    ...model,
    get repeat() {
      return model.repeat;
    },
    setRepeat(mode: 'off' | 'one' | 'all'): void {
      model.setRepeat(mode);
    },
    get shuffle() {
      return model.shuffle;
    },
    setShuffle(on: boolean): void {
      model.setShuffle(on);
    },
    getPresentationOrder(): number[] {
      return model.getPresentationOrder();
    },
    /** Test-only seam: pin the shuffle presentation order deterministically
     *  (see QueueModel.setPresentationOrderForTest). */
    setPresentationOrderForTest(next: number[]): void {
      model.setPresentationOrderForTest(next);
    },
    /** Play the given release from `startIndex` (default first track). */
    playAlbum(
      release: ReleaseDetail,
      title: string,
      artist: string,
      startIndex = 0,
    ): QueueItem {
      const items = itemsFor(release, title, artist, opts.selection?.());
      if (items.length === 0) throw new Error('This release has no playable tracks.');
      return model.playSequence(items, startIndex);
    },
    /** Append a single item to the end of the current queue. */
    addItem(item: QueueItem): void {
      model.enqueue(item);
    },
    /** Append all tracks of a release to the end of the current queue. */
    addAlbum(
      release: ReleaseDetail,
      title: string,
      artist: string,
    ): void {
      model.enqueueMany(itemsFor(release, title, artist, opts.selection?.()));
    },
  };
}

export type QueueStore = ReturnType<typeof createQueueStore>;
