// Cross-reload session snapshot codec (player-core M1; v2 in M6).
//
// Pure serialize/validate/normalize extracted from the web
// PlayerController.persist()/restore(). Storage I/O and throttling stay
// host-side; this module owns only the wire format and its invariants.
//
// Versions:
//   v1 (shipped): {v:1, items, index, positionSeconds, volume, normalizeMode}
//     — no playback policy; a restored session defaults to repeat 'off',
//       shuffle off.
//   v2 (M6): adds `repeat` ('off'|'one'|'all') and `shuffle` (boolean).
//
// The codec validates only what restoration relies on and clamps the rest;
// decode failures are non-fatal by design (best-effort resume).

import type { NormalizationMode } from './gain';
import type { RepeatMode } from './order';

export const SNAPSHOT_VERSION = 2;
/** The version the web client originally shipped. */
export const SNAPSHOT_VERSION_V1 = 1;

export interface SnapshotItemBase {
  /** Minimal identity contract for restoration; hosts may persist richer
   *  item shapes verbatim (the web persists QueueItems). */
  track?: { id?: number; audio?: { url?: string }; duration?: number };
  /** Modern item identity. Writers since the player-core extraction
   *  (c5bf447) always emit these; the restorer requires them so payloads
   *  persisted by PRE-extraction bundles (bare `{track,...}` items) are
   *  dropped wholesale instead of restoring unplayable queue entries that
   *  a later persist would write back over good storage. */
  id?: string;
  source?: { kind?: string; url?: string };
}

export interface SessionSnapshotV1 extends SnapshotItemBase {
  v: typeof SNAPSHOT_VERSION_V1;
  items: SnapshotItemBase[];
  index: number;
  positionSeconds: number;
  volume: number;
  normalizeMode: NormalizationMode;
}

export interface SessionSnapshot {
  v: typeof SNAPSHOT_VERSION;
  items: SnapshotItemBase[];
  index: number;
  positionSeconds: number;
  volume: number;
  normalizeMode: NormalizationMode;
  /** Playback policy (v2). Always present after decode (v1 → defaults). */
  repeat: RepeatMode;
  shuffle: boolean;
  /** Crossfade seconds; 0 = off. Optional for forward/backward tolerance:
   *  absent or invalid → 0. (M8) */
  crossfadeSeconds?: number;
}

function isRestorableItem(item: unknown): item is SnapshotItemBase {
  return (
    typeof item === 'object' &&
    !!item &&
    typeof (item as SnapshotItemBase).id === 'string' &&
    ((item as SnapshotItemBase).id as string).length > 0 &&
    !!(item as SnapshotItemBase).source &&
    typeof (item as SnapshotItemBase).source?.kind === 'string' &&
    typeof (item as SnapshotItemBase).source?.url === 'string' &&
    ((item as SnapshotItemBase).source?.url as string).length > 0 &&
    typeof (item as SnapshotItemBase).track?.id === 'number' &&
    !!(item as SnapshotItemBase).track?.audio?.url
  );
}

function clampIndex(index: number, length: number): number {
  return Math.min(Math.max(0, Math.floor(index) || 0), Math.max(0, length - 1));
}

function parseRepeat(v: unknown): RepeatMode {
  return v === 'one' || v === 'all' ? v : 'off';
}

/** Parses a v1 or v2 payload. Returns null when absent/corrupt/empty —
 *  restoration then simply does not happen (never fatal). */
export function decodeSnapshot(raw: string | null | undefined): SessionSnapshot | null {
  if (!raw) return null;
  let candidate: SessionSnapshotV1 | SessionSnapshot | null = null;
  try {
    candidate = JSON.parse(raw) as SessionSnapshotV1 | SessionSnapshot;
  } catch {
    return null;
  }
  if (
    !candidate ||
    (candidate.v !== SNAPSHOT_VERSION && candidate.v !== SNAPSHOT_VERSION_V1) ||
    !Array.isArray(candidate.items)
  ) {
    return null;
  }
  const items = candidate.items.filter(isRestorableItem);
  if (items.length === 0) return null;
  // v1 payloads carry no policy: default to repeat off / shuffle off.
  const legacy = candidate as Partial<SessionSnapshot>;
  // Crossfade: absent/invalid → 0 (off). Only the shipped cycle values are
  // honored (4/8/12); anything else is treated as off.
  const rawCf = legacy.crossfadeSeconds;
  const decoded: SessionSnapshot = {
    v: SNAPSHOT_VERSION,
    items,
    index: clampIndex(candidate.index, items.length),
    positionSeconds: candidate.positionSeconds,
    volume: candidate.volume,
    normalizeMode: candidate.normalizeMode,
    repeat: parseRepeat(legacy.repeat),
    shuffle: legacy.shuffle === true,
    crossfadeSeconds: typeof rawCf === 'number' && [4, 8, 12].includes(rawCf) ? rawCf : 0,
  };
  return decoded;
}

export function encodeSnapshot(snapshot: SessionSnapshot): string {
  return JSON.stringify(snapshot);
}

export { clampIndex };
