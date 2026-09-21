// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Offline-aware snapshot storage wrapper (plan §7.5).
//
// The player-core session snapshot persists queue items with their REMOTE
// source URLs. On an offline reload those URLs are unreachable; the D1
// local-first rule says locally-held content must play from local storage.
// Rather than teaching the core about offline state, we wrap the host's
// StoragePort: on read, persisted items whose track is installed get their
// source rewritten to the local descriptor. The core cannot tell and needs
// no changes.

import type { StoragePort } from '../../../../player-core/src/player';
import { decodeSnapshot, encodeSnapshot } from '../../../../player-core/src/snapshot';
import type { SnapshotItemBase } from '../../../../player-core/src/snapshot';
import type { OfflineContext } from '../state/queue';

interface RemappableItem extends SnapshotItemBase {
  id?: string;
  source?: { kind: string; url: string; byteSize?: number };
  representationId?: number;
}

/** Wraps a StoragePort so restored items resolve to local-file sources
 *  when the catalog holds them. Write-through is untouched. */
export function offlineAwareStorage(
  inner: StoragePort,
  offline: OfflineContext,
): StoragePort {
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
        const item = item0 as RemappableItem;
        const trackId = item.track?.id;
        if (typeof trackId !== 'number' || !item.source) return item0;
        const candidate =
          item.representationId !== undefined && typeof item.representationId === 'number'
            ? { kind: 'representation' as const, id: item.representationId }
            : { kind: 'primary' as const };
        const key = offline.localKeyFor(trackId, candidate);
        if (key === null || item.source.kind === 'local-file') return item0;
        changed = true;
        return {
          ...item,
          source: { kind: 'local-file', url: key, byteSize: item.source.byteSize },
        };
      });
      if (!changed) return raw;
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
