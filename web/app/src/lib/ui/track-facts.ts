// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Pure derived facts for the track page (`TrackPage.svelte`). Functions of
// the API payloads and the player snapshot only — no stores, no DOM — so
// the page component stays a composition surface and these rules are
// unit-testable.

import type { ReleaseDetail, Track, TrackDetail } from '../api/types';
import { codecName, formatBytes, qualityLine } from '../format';
import { normalizationGainDb } from '../playback/loudness';

export interface TrackFacts {
  leadArtist: Track['artists'][number] | undefined;
  /** The disc of the owning release the track sits on. */
  trackDisc: ReleaseDetail['media'][number] | null;
  /** Track index within the whole release (disc order), −1 when unknown. */
  trackIndexInRelease: number;
  allTracks: Track[];
  trackCount: number;
  waveCount: number;
  repCount: number;
  isMpc: boolean;
  primaryFormat: string;
  /** Identifier rows the API actually carries (currently ISRC only). */
  identifiers: Array<[string, string]>;
  /** "Disc x of y · Track n of m" payload, or null before context exists. */
  positionLine: {
    disc: number;
    of: number;
    pos: number;
    of2: number;
    title?: string;
  } | null;
}

export function trackFacts(
  detail: TrackDetail | null,
  rel: ReleaseDetail | null,
): TrackFacts {
  const t = detail?.track ?? null;
  const ctx = detail?.context;
  const allTracks = rel ? rel.media.flatMap((m) => m.tracks) : [];

  let trackIndexInRelease = -1;
  if (rel && t) {
    let i = 0;
    for (const disc of [...rel.media].sort((a, b) => a.disc - b.disc)) {
      for (const track of disc.tracks) {
        if (track.id === t.id) {
          trackIndexInRelease = i;
          break;
        }
        i += 1;
      }
      if (trackIndexInRelease >= 0) break;
    }
  }

  const trackDisc = rel && t && ctx ? (rel.media.find((m) => m.disc === ctx.disc) ?? null) : null;

  let positionLine: TrackFacts['positionLine'] = null;
  if (ctx && rel) {
    positionLine = {
      disc: ctx.disc,
      of: rel.media.length,
      pos: trackIndexInRelease + 1,
      of2: allTracks.length,
      title: trackDisc?.title,
    };
  }

  return {
    leadArtist: t?.artists[0],
    trackDisc,
    trackIndexInRelease,
    allTracks,
    trackCount: allTracks.length,
    waveCount: allTracks.filter((tr) => tr.waveform).length,
    repCount: allTracks.reduce((n, tr) => n + (tr.representations?.length ?? 0), 0),
    isMpc: Boolean(t && t.codec.codec.toLowerCase().startsWith('musepack')),
    primaryFormat: t
      ? qualityLine({ codec: t.codec.codec, sampleRate: t.codec.sampleRate, channels: t.codec.channels })
      : '',
    identifiers: t?.isrc ? ([['ISRC', t.isrc]] as Array<[string, string]>) : [],
    positionLine,
  };
}

/** One row per audio variant (primary first), display facts only. */
export interface RepRow {
  /** Stable row identity: `'primary'` or the representation id as string. */
  id: string;
  label: string;
  sub: string;
  size?: number;
  sha?: string;
}

export function representationRows(t: Track | null): RepRow[] {
  if (!t) return [];
  const rows: RepRow[] = [];
  rows.push({
    id: 'primary',
    label: codecName(t.codec.codec),
    sub:
      t.codec.codec.toLowerCase().startsWith('musepack') && t.codec.streamVersion
        ? `SV${t.codec.streamVersion}`
        : qualityLine({ codec: t.codec.codec, sampleRate: t.codec.sampleRate, channels: t.codec.channels }),
    size: t.audio.size,
    sha: t.audio.sha256,
  });
  for (const rep of t.representations ?? []) {
    rows.push({
      id: String(rep.id),
      label: rep.label ?? codecName(rep.codec.codec),
      sub: qualityLine({ codec: rep.codec.codec, sampleRate: rep.codec.sampleRate, channels: rep.codec.channels }),
      size: rep.size,
      sha: rep.sha256,
    });
  }
  return rows;
}

/** Track-relative seek timeline from a player model snapshot. Pure. */
export interface SeekTimeline {
  /** Album-absolute start of this track (0 unless it is playing). */
  trackStart: number;
  /** Duration to show (player's repaired value while playing, else API). */
  trackDur: number;
  /** Position within the track, clamped. */
  withinPos: number;
  /** Range fill percentage 0–100 (0 when duration unknown). */
  rangeFill: number;
  /** Whether this track is the one currently loaded. */
  isCurrent: boolean;
}

export function seekTimeline(
  snapshot: {
    currentTrackId: number | undefined;
    currentTrackStartSeconds: number;
    currentTrackDurationSeconds: number;
    positionSeconds: number;
  },
  t: Track | null,
): SeekTimeline {
  const isCurrent = snapshot.currentTrackId !== undefined && snapshot.currentTrackId === t?.id;
  const trackStart = isCurrent ? snapshot.currentTrackStartSeconds : 0;
  const trackDur = isCurrent ? snapshot.currentTrackDurationSeconds : (t?.duration ?? 0);
  const withinPos = Math.max(0, Math.min(trackDur, snapshot.positionSeconds - trackStart));
  const rangeFill = trackDur > 0 ? Math.max(0, Math.min(100, (withinPos / trackDur) * 100)) : 0;
  return { trackStart, trackDur, withinPos, rangeFill, isCurrent };
}

/** Album/track normalization gain previews (null without measurements). */
export function trackNormalizationGains(
  t: Track | null,
  rel: ReleaseDetail | null,
): { album: number | null; track: number | null } {
  const album = rel?.loudness
    ? normalizationGainDb('album', undefined, {
        albumLufs: rel.loudness.albumLufs,
        albumTruePeakDb: rel.loudness.albumTruePeakDb,
      })
    : null;
  const track = t?.loudness ? normalizationGainDb('track', t.loudness) : null;
  return { album, track };
}

/** Human size for a row (shared by the audio section and the rail). */
export function repRowSize(row: RepRow): string | undefined {
  return row.size ? formatBytes(row.size) : undefined;
}
