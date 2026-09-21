// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Representation-selection policy (Phase 4): the pure, deterministic rules
// that decide which of a track's audio representations becomes the source of
// a PlaybackItem. Consumed ONLY through itemForTrack() (state/queue.ts) —
// Player Core never sees representations.
//
// Purity: DOM-free, Node-testable, total (never throws). Playability is an
// injected predicate so browser capability probing stays at the host boundary
// and a future offline host can inject local-availability instead.

import type { RepresentationRef, Track } from '../api/types';

/** The single preference mechanism (one resolver, one precedence rule).
 *  Per-track override maps are deliberately deferred; exactly one active
 *  preference exists. */
export type AudioPreference =
  /** Primary audio only — pre-Phase-4 behavior. */
  | { mode: 'default' }
  /** One explicit representation row (server ids are stable per API spec). */
  | { mode: 'representation'; id: number }
  /** First playable representation of an exact codec family. */
  | { mode: 'codec'; codec: string }
  /** First playable lossless representation; otherwise normal fallback. */
  | { mode: 'lossless' };

/** Codecs classified as lossless with current probed metadata. Closed set;
 *  adding codecs is a deliberate format-adjacent decision. */
export const LOSSLESS_CODECS: readonly string[] = ['flac', 'wav', 'aiff'];

/** Host-injected playability. Receives the same shape chooseBackend judges:
 *  codec strings for musepack, MIME for browser-native probing — plus the
 *  candidate's source identity when the caller is judging a specific audio
 *  object (primary vs one representation). Offline hosts compose local
 *  availability into their predicate using that identity; online-only
 *  hosts ignore it. Optional, so existing predicates keep compiling. */
export type CanPlay = (c: {
  codec?: string;
  mimeType?: string;
  /** Which audio object is being judged: omitted = "a primary" (legacy
   *  callers); 'representation' carries the manifest row id. */
  source?: { kind: 'primary' } | { kind: 'representation'; id: number };
}) => boolean;

/** Accepts everything: used when no availability information exists so that
 *  resolution reduces to manifest order + primary preference only. */
export const acceptAll: CanPlay = () => true;

/** What itemForTrack() needs: the chosen representation (null = primary). */
export interface SelectedAudio {
  representation: RepresentationRef | null;
}

function lower(s: string | undefined): string {
  return (s ?? '').toLowerCase();
}

function playableInOrder(
  candidates: RepresentationRef[],
  canPlay: CanPlay,
): RepresentationRef | null {
  // Manifest position order (the API preserves it) is THE tie-break.
  for (const rep of candidates) {
    if (
      canPlay({
        codec: rep.codec?.codec,
        mimeType: rep.codec?.mimeType,
        source: { kind: 'representation', id: rep.id },
      })
    ) {
      return rep;
    }
  }
  return null;
}

/** Validates an unknown value into a preference; null when unusable. Kept
 *  here (not only in the persistence layer) so the resolver itself tolerates
 *  malformed input from any source. */
export function parseAudioPreference(raw: unknown): AudioPreference | null {
  if (!raw || typeof raw !== 'object') return null;
  const p = raw as Record<string, unknown>;
  switch (p.mode) {
    case 'default':
      return { mode: 'default' };
    case 'representation':
      return typeof p.id === 'number' && Number.isFinite(p.id)
        ? { mode: 'representation', id: p.id }
        : null;
    case 'codec':
      return typeof p.codec === 'string' && p.codec.trim() !== ''
        ? { mode: 'codec', codec: p.codec }
        : null;
    case 'lossless':
      return { mode: 'lossless' };
    default:
      return null;
  }
}

/** Deterministic selection + fallback. Total: always returns a usable
 *  choice (the primary when nothing better resolves), so callers keep
 *  building items and today's unsupported-format failure surfaces at engine
 *  open, unchanged.
 *
 *  1. undefined/{default} → default resolution (step 5).
 *  2. {representation,id} → first candidate with that id, if playable.
 *  3. {codec} → first playable candidate with that codec (case-insensitive).
 *  4. {lossless} → first playable candidate in LOSSLESS_CODECS.
 *  5. Default resolution: playable-or-codec-less primary → primary;
 *     else first playable candidate (rescue); else primary (today's
 *     failure behavior). */
export function resolveAudio(
  track: Track,
  pref: AudioPreference | undefined,
  canPlay: CanPlay = acceptAll,
): SelectedAudio {
  const candidates = track.representations ?? [];
  const primaryPlayable = canPlay({
    codec: track.codec?.codec,
    mimeType: track.codec?.mimeType,
    source: { kind: 'primary' },
  });

  let chosen: RepresentationRef | null = null;
  const valid = parseAudioPreference(pref);
  if (!valid || valid.mode === 'default') {
    chosen = null;
  } else if (valid.mode === 'representation') {
    const match = candidates.find((r) => r.id === valid.id);
    if (
      match &&
      canPlay({
        codec: match.codec?.codec,
        mimeType: match.codec?.mimeType,
        source: { kind: 'representation', id: match.id },
      })
    ) {
      chosen = match;
    }
  } else if (valid.mode === 'codec') {
    const wanted = valid.codec.toLowerCase();
    chosen = playableInOrder(
      candidates.filter((r) => lower(r.codec?.codec) === wanted),
      canPlay,
    );
  } else {
    // 'lossless'
    chosen = playableInOrder(
      candidates.filter((r) => LOSSLESS_CODECS.includes(lower(r.codec?.codec))),
      canPlay,
    );
  }

  if (chosen === null && !primaryPlayable && candidates.length > 0) {
    // Rescue: primary unplayable but a playable alternate exists. Unreachable
    // for pre-representation tracks, and only converts a guaranteed error
    // into playback.
    chosen = playableInOrder(candidates, canPlay);
  }
  return { representation: chosen };
}
