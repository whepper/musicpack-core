// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Download planning (offline downloads, plan §5 policy).
//
// v1 distribution policy (approved): the complete audio surface — every
// track's primary audio AND all declared representations — plus per-track
// waveforms (they feed both the seek control and Sweet-Fade planning),
// referenced release artwork, and — since R3.6 — every track-linked lyric
// document carried by the release-level `trackLyrics[]` index
// (docs/musicpack-lyrics-v1.md §7.4). Booklet/extras/analysis stay
// deferred (D3): no client feature consumes them.
//
// The planner stays a **pure, no-fetch function**: the R3.6 index exists
// precisely so it never has to fetch per-track details (see
// docs/r3.6-offline-lyrics-contract.md §13). Root-level lyrics remain
// deferred (D3, no consumer).
//
// Pure function over ReleaseDetail: Node-testable, no fetch, no storage.

import type { PlannedAsset } from './types';

function assetKey(parts: Array<string | number>): string {
  return parts.join('.');
}

/** The stable offline key of one track's lyric document (R3.6): identity
 *  is `(trackId, lyric asset id)` — never an array position — so adding,
 *  removing, reordering or re-tagging lyric refs never re-keys a staged
 *  asset. Shared with the update check so the two can never drift. */
export function lyricsAssetKey(trackId: number, lyricsId: number): string {
  return assetKey(['t', trackId, 'lyr', lyricsId]);
}

/** Builds the deterministic download plan for a release in canonical
 *  media/track order. Assets without a usable size are skipped: size is
 *  what staging pre-allocates and verifies against. */
export function planReleaseAssets(release: {
  id: number;
  media: Array<{
    tracks: Array<{
      id: number;
      audio: { url: string; size: number; sha256?: string };
      representations?: Array<{
        id: number;
        url: string;
        size: number;
        sha256?: string;
      }>;
      waveform?: { url: string; points: number; sha256?: string } | null;
    }>;
  }>;
  artwork: Array<{ id: number; url: string; mimeType?: string; sha256?: string }>;
  /** Release-level track-lyric index (R3.6). Optional so pre-R3.6
   *  payloads and lyric-less releases plan exactly as before. */
  trackLyrics?: Array<{
    trackId: number;
    id: number;
    url: string;
    size: number;
    sha256?: string;
    lang?: string;
  }>;
}): PlannedAsset[] {
  const out: PlannedAsset[] = [];
  // Media/track iteration follows the API's canonical manifest order; it
  // affects staging order only, never identity (keys are content-keyed).
  for (const disc of release.media) {
    for (const track of disc.tracks) {
      if (track.audio?.size > 0) {
        out.push({
          key: assetKey(['t', track.id, 'primary']),
          kind: 'audio-primary',
          trackId: track.id,
          url: track.audio.url,
          size: track.audio.size,
          sha256: track.audio.sha256,
        });
      }
      for (const rep of track.representations ?? []) {
        if (rep.size > 0) {
          out.push({
            key: assetKey(['t', track.id, 'r', rep.id]),
            kind: 'audio-representation',
            trackId: track.id,
            representationId: rep.id,
            url: rep.url,
            size: rep.size,
            sha256: rep.sha256,
          });
        }
      }
      if (track.waveform && track.waveform.points > 0) {
        out.push({
          key: assetKey(['t', track.id, 'w']),
          kind: 'waveform',
          trackId: track.id,
          url: track.waveform.url,
          size: track.waveform.points * 2, // peak-rms-u8: 2 bytes/bucket
          sha256: track.waveform.sha256,
        });
      }
    }
  }
  // Track-linked lyrics, in the server's deterministic (trackId, id)
  // order. An entry without the authoritative hash cannot be staged
  // (spec §7.4) and is skipped rather than hashed client-side; the
  // declared size is used as-is (an empty document is legal, so size 0
  // is not a skip condition — the hash still verifies the bytes).
  for (const lyric of release.trackLyrics ?? []) {
    if (!lyric.sha256) continue;
    out.push({
      key: lyricsAssetKey(lyric.trackId, lyric.id),
      kind: 'lyrics',
      trackId: lyric.trackId,
      lyricsId: lyric.id,
      url: lyric.url,
      size: lyric.size,
      sha256: lyric.sha256,
    });
  }
  for (const art of release.artwork) {
    out.push({
      key: assetKey(['art', art.id]),
      kind: 'artwork',
      artworkId: art.id,
      url: art.url,
      size: 0, // artwork size is not declared by the API; staged then measured
      sha256: art.sha256,
    });
  }
  return out;
}
