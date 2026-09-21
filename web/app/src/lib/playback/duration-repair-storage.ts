// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Restore-time duration-hint repair (session-storage hygiene).
//
// A future queue track's length is known only through
// `durationHintSeconds` (= Track.duration at itemForTrack() time). When
// that field never made it into a persisted queue item — historic client
// bundles, libraries imported from packages whose manifests carry no
// durations — every unopened successor has effective length 0 and the
// cumulative offset table collapses onto the current track's end.
//
// This StoragePort wrapper heals eligible payloads at READ time so the
// player restores queue knowledge the metadata always had, without
// touching player-core logic and without inventing any duration: the
// repair value comes exclusively from the same Track the item was built
// from (`track.duration`), i.e. no new authority is introduced.
//
// Mirrors offlineAwareStorage (offline/snapshot-storage.ts): a narrow,
// host-side read-time transform over the wrapped port, write-through
// untouched. The two transforms are disjoint — this repairs only
// durationHintSeconds, the offline remap rewrites only source.kind/url —
// so their composition order does not matter.

import type { StoragePort } from '../../../../player-core/src/player';
import { decodeSnapshot, encodeSnapshot } from '../../../../player-core/src/snapshot';
import type { SnapshotItemBase } from '../../../../player-core/src/snapshot';

/** Items we may touch carry an optional numeric hint on top of the base
 *  identity contract; everything else stays structurally opaque. */
interface HintRepairableItem extends SnapshotItemBase {
  durationHintSeconds?: number;
}

function positive(n: unknown): n is number {
  return typeof n === 'number' && Number.isFinite(n) && n > 0;
}

function authoritativeTrackDuration(item: SnapshotItemBase): number | undefined {
  const d = item.track?.duration;
  return positive(d) ? d : undefined;
}

export function repairingStorage(inner: StoragePort): StoragePort {
  return {
    get(): string | null {
      const raw = inner.get();
      if (raw === null) return null;
      let parsed: ReturnType<typeof decodeSnapshot> = null;
      try {
        parsed = decodeSnapshot(raw);
      } catch {
        return raw;
      }
      if (!parsed) return raw;
      let changed = false;
      const items = parsed.items.map((item0) => {
        const item = item0 as HintRepairableItem;
        // Valid positive hint wins verbatim — never second-guess it even
        // when track.duration disagrees (e.g. newer manifest values).
        if (positive(item.durationHintSeconds)) return item0;
        const duration = authoritativeTrackDuration(item);
        // No truth available: leave the field exactly as found. An absent
        // hint plus absent duration means genuinely unknown length, which
        // player-core already handles defensively (never advances past a
        // zero-length successor on position alone).
        if (duration === undefined) return item0;
        changed = true;
        return { ...item, durationHintSeconds: duration };
      });
      if (!changed) return raw; // byte-stable passthrough; reads stay idempotent
      try {
        return encodeSnapshot({ ...parsed, items });
      } catch {
        return raw;
      }
    },
    set(value: string | null): void {
      inner.set(value);
    },
  };
}
