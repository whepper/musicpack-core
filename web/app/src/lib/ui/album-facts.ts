// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Pure derived facts for the album page (`AlbumPage.svelte`). Everything
// here is a function of the API payloads — no stores, no fetching, no DOM —
// so the page component stays a composition surface and these rules are
// unit-testable. Values are API values only; nothing is invented.

import type { ArtworkRef, ReleaseDetail, Track } from '../api/types';
import { codecName, formatBytes, mediumLabel, qualityLine } from '../format';
import { normalizationGainDb } from '../playback/loudness';

/** One deduplicated representation "family" across the release. */
export interface RepGroup {
  key: string;
  title: string;
  sub: string;
  codec: string;
}

export interface ReleaseFacts {
  /** All tracks across discs, in payload order. */
  allTracks: Track[];
  trackCount: number;
  discCount: number;
  /** Sum of per-track durations in seconds (0 when unknown). */
  totalDuration: number;
  /** First track of the first medium — the release's "primary" audio. */
  primary: Track | undefined;
  primarySize: string;
  totalSize: string;
  /** Tracks carrying a waveform envelope. */
  waveCount: number;
  /** Total alternate representations across all tracks. */
  repCount: number;
  /** Deduplicated representation families (label or codec name). */
  repGroups: RepGroup[];
  isMpc: boolean;
  /** `SV8` for Musepack primaries, otherwise the quality line. */
  primaryTileSub: string;
  /** Comma-joined distinct medium labels ("CD, SACD"). */
  mediums: string;
  /** Tracks carrying an ISRC, in payload order. */
  trackIsrcs: Array<{ title: string; isrc: string }>;
}

export function releaseFacts(rel: ReleaseDetail | null | undefined): ReleaseFacts {
  const allTracks = rel ? rel.media.flatMap((m) => m.tracks) : [];
  const primary = rel?.media[0]?.tracks[0];
  const isMpc = Boolean(primary && primary.codec.codec.toLowerCase().startsWith('musepack'));

  const seen = new Map<string, RepGroup>();
  for (const track of allTracks) {
    for (const rep of track.representations ?? []) {
      const key = (rep.label ?? rep.codec.codec).toLowerCase();
      seen.set(key, {
        key,
        title: rep.label ?? codecName(rep.codec.codec),
        sub: qualityLine({
          codec: rep.codec.codec,
          sampleRate: rep.codec.sampleRate,
          channels: rep.codec.channels,
        }),
        codec: rep.codec.codec.toLowerCase(),
      });
    }
  }

  return {
    allTracks,
    trackCount: allTracks.length,
    discCount: rel?.media.length ?? 0,
    totalDuration: allTracks.reduce((n, t) => n + (t.duration ?? 0), 0),
    primary,
    primarySize: formatBytes(
      (rel?.media ?? []).reduce((sum, disc) => sum + disc.tracks.reduce((s, t) => s + t.audio.size, 0), 0),
    ),
    totalSize: formatBytes(
      (rel?.media ?? []).reduce(
        (sum, disc) =>
          sum +
          disc.tracks.reduce(
            (s, t) => s + t.audio.size + (t.representations ?? []).reduce((r, rep) => r + rep.size, 0),
            0,
          ),
        0,
      ),
    ),
    waveCount: allTracks.filter((t) => t.waveform).length,
    repCount: allTracks.reduce((n, t) => n + (t.representations?.length ?? 0), 0),
    repGroups: [...seen.values()],
    isMpc,
    primaryTileSub:
      isMpc && primary?.codec.streamVersion
        ? `SV${primary.codec.streamVersion}`
        : primary
          ? qualityLine({ codec: primary.codec.codec, sampleRate: primary.codec.sampleRate, channels: primary.codec.channels })
          : '',
    mediums: rel
      ? rel.media
          .map((m) => mediumLabel(m.format))
          .filter((v, i, a) => a.indexOf(v) === i)
          .join(', ')
      : '',
    trackIsrcs: allTracks.filter((t) => t.isrc).map((t) => ({ title: t.title, isrc: t.isrc ?? '' })),
  };
}

/** Front-cover artwork, else the first image (hero + viewer pick). */
export function heroArtwork(artwork: ArtworkRef[] | undefined): ArtworkRef | undefined {
  return artwork?.find((a) => a.role === 'front') ?? artwork?.[0];
}

/** Album-mode normalization gain preview for the release, or null when the
 *  release carries no loudness measurement (player policy: −16 LUFS target). */
export function albumNormalizationGain(rel: ReleaseDetail | null | undefined): number | null {
  if (!rel?.loudness) return null;
  return normalizationGainDb('album', undefined, {
    albumLufs: rel.loudness.albumLufs,
    albumTruePeakDb: rel.loudness.albumTruePeakDb,
  });
}

/** The rail's waveform peek: the playing track when it belongs to this
 *  release, otherwise the first track that carries an envelope. */
export function waveformPeekTrack(
  rel: ReleaseDetail | null | undefined,
  allTracks: Track[],
  currentTrackId: number | undefined,
): Track | null {
  if (!rel) return null;
  if (currentTrackId && allTracks.some((t) => t.id === currentTrackId)) {
    return allTracks.find((t) => t.id === currentTrackId) ?? null;
  }
  return allTracks.find((t) => t.waveform) ?? null;
}
