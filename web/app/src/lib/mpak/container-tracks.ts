// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Client-side `.mpak` container tracks → the existing queue.
//
// A container that is already available to the client can be read by Rust
// (`RustPlaybackEngine.openContainer`), which returns its MANF's tracks with
// canonical `mpak:<container>#<member>` sources. This module turns those into
// ordinary `QueueItem`s, so a container album enters the *same* queue, player
// and playback path as a directory-backed one — there is no container queue,
// no container player and no second source kind.
//
// What this file does is field mapping, nothing more: the container, its
// member table, its manifest and its codec hints were all resolved in Rust
// through `musicpack-core`. In particular the `mpak:` key is **not** rebuilt
// here — Rust hands it over and this module passes it through untouched, so the
// two can never disagree about a member's identity.

import type { Track } from '../api/types';
import type { QueueItem } from '../state/queue';
import type { PlaybackSource } from '../../../../player-core/src/types';
import type { ContainerAlbumWire, ContainerTrackWire } from '../playback/rust-playback-engine';

/** The one capability a host must provide to read a container: the Rust
 *  MANF reader, over the same range transport playback uses. Structurally
 *  `RustPlaybackEngine.openContainer`, so the app passes the engine it already
 *  plays with. */
export interface ContainerReader {
  openContainer(
    container: string,
    size: number,
    kind?: PlaybackSource['kind'],
  ): Promise<ContainerAlbumWire>;
}

/** A container track's synthetic server row id, derived from its canonical
 *  source.
 *
 *  A container member has no server row, but `Player` keys its exact-decoded
 *  lengths by `trackId` (player.ts), so two tracks sharing an id would alias
 *  and corrupt the album clock. Hashing the canonical key keeps the id stable
 *  across reloads and unique per member, and the negative range keeps it clear
 *  of real (positive) SQLite rowids. */
function memberRowId(source: string): number {
  // FNV-1a, 32-bit, masked to a positive 31-bit value, then negated.
  let hash = 0x811c9dc5;
  for (let i = 0; i < source.length; i++) {
    hash ^= source.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  const masked = (hash & 0x7fff_ffff) >>> 0;
  return -(masked === 0 ? 1 : masked);
}

/** Builds the `Track`-shaped object a `QueueItem` carries.
 *
 *  `QueueItem.track` is load-bearing today: `format.ts` reads its codec,
 *  `player-core/snapshot.ts` requires `id` and `audio.url`, and the players
 *  read `title`. Synthesizing the shape keeps every one of those readers on
 *  their existing path — a container track degrades exactly like a server
 *  track with no waveform and no alternate representations.
 *
 *  `waveform: null` and an omitted `representations` are deliberate: they are
 *  the existing "nothing extra" values, so no consumer needs a new branch.
 */
function trackOf(track: ContainerTrackWire, id: number): Track {
  return {
    id,
    number: track.number,
    title: track.title,
    artists: [],
    ...(track.durationSeconds !== undefined ? { duration: track.durationSeconds } : {}),
    codec: {
      codec: track.codec ?? 'unknown',
      mimeType: track.mimeType ?? 'application/octet-stream',
    },
    audio: {
      id,
      size: track.size,
      // The canonical container key. Snapshot restore requires a non-empty
      // `audio.url`, and this is the identity that means.
      url: track.source,
      sha256: undefined,
    },
    waveform: null,
  };
}

/** Builds the queue items for a container's tracks, in manifest order.
 *
 *  `source.byteSize` is the **container's** length, not the member's: a
 *  container source is scanned from its tail framing, so the transport needs
 *  the container's size. The member's own length travels on `track.audio.size`
 *  for display and bookkeeping.
 */
export function itemsForContainer(album: ContainerAlbumWire): QueueItem[] {
  const artist = album.artists.join(', ');
  return album.tracks.map((track) => {
    const rowId = memberRowId(track.source);
    return {
      // A distinct id scheme from a server track's `t<n>`, so
      // `sameItemIdentity` and the queue list's keyed `each` stay unambiguous.
      id: `mpak:${track.source}`,
      trackId: rowId,
      source: {
        // `http-range` with the canonical key: the engine and the worker
        // resolve the member from it, and the container is read by range.
        kind: 'http-range' as const,
        url: track.source,
        byteSize: album.size,
      },
      durationHintSeconds: track.durationSeconds,
      title: track.title,
      artist,
      albumTitle: album.title,
      artworkUrl: undefined,
      loudness: undefined,
      albumLoudness: undefined,
      codec: track.codec,
      mimeType: track.mimeType,
      // web-specific fields
      track: trackOf(track, rowId),
      // A container album is not in the server's release/album graph, so there
      // is no release or album row to point at. Both are optional on
      // `QueueItem` and every reader already tolerates their absence
      // (`transition-profiles.ts` guards `releaseId`; `QueuePage` renders
      // `albumTitle` as text when there is no album to link).
      releaseId: undefined,
      albumId: undefined,
    };
  });
}

/** The one call that turns an available container into playable queue items.
 *
 * ```text
 * .mpak → MANF → track/member identity → mpak:<container>#<member>
 *       → existing playback source routing → RangeByteSource
 *       → core MpakBackend → member bytes → existing decoder
 * ```
 *
 * `engine` is any host that can read the container (in the app, the
 * `RustPlaybackEngine` that also plays it). The returned items go into the
 * ordinary queue — `queue.playItems(items)` or `queue.addItems(items)` — and
 * play through the ordinary player with no further container handling.
 *
 * `size` is the container's length in bytes and is required: a container's
 * tail framing is at the end of the file, so it cannot be scanned without it.
 */
export async function containerQueueItems(
  engine: ContainerReader,
  container: string,
  size: number,
  kind: PlaybackSource['kind'] = 'http-range',
): Promise<QueueItem[]> {
  if (!Number.isFinite(size) || size <= 0) {
    throw new Error('A container needs its length in bytes to be read.');
  }
  const album = await engine.openContainer(container, size, kind);
  const items = itemsForContainer(album);
  if (items.length === 0) {
    // A container with no playable tracks is not an empty album: it is a
    // failure, and it must not enter the queue looking like an empty release.
    throw new Error(`Container '${container}' defines no playable tracks.`);
  }
  return items;
}
