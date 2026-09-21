// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Restore-time duration-hint repair (web host layer).
//
// Future queue tracks know their length ONLY through
// durationHintSeconds (= Track.duration, state/queue.ts:105). Snapshots
// persisted by hint-less writers (historic bundles, libraries whose
// packages carry no manifest duration) restore items where every
// unopened successor has effective length 0 — collapsing cumulative
// offsets onto the current track's end (the deterministic THRASHER
// jump). repairingStorage() heals eligible snapshots at READ time,
// mirroring the offlineAwareStorage pattern: repair touches
// durationHintSeconds ONLY; offline remapping touches source ONLY, so
// composition order is irrelevant.
//
// Repair contract pinned here:
// - derive exclusively from item.track.duration when it is finite > 0;
// - never overwrite an existing positive hint (stale-but-valid wins);
// - replace absent/invalid/non-positive hints whenever truth exists;
// - invent nothing (invalid Track.duration leaves the field as found);
// - waveform metadata is NOT authority (server backfill owns that);
// - re-encode only when something changed; otherwise byte-for-byte
//   passthrough (idempotent reads).
// Plus controller-level wiring proof (default localStorage port path)
// and persistence of the healed snapshot through a natural persist.

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { repairingStorage } from '../../app/src/lib/playback/duration-repair-storage';
import type { StoragePort } from '../../player-core/src/player';
import { decodeSnapshot, encodeSnapshot } from '../../player-core/src/snapshot';

// ---- helpers -----------------------------------------------------------

type ItemLike = Record<string, unknown> & {
  track?: Record<string, unknown>;
};

/** Modern-era QueueItem wire shape, parameterised over hint/duration. */
function wireItem(opts: {
  id?: number;
  hint?: number | null;
  omitHint?: boolean;
  duration?: number; // undefined = field absent on track
  extraTrack?: Record<string, unknown>;
}): ItemLike {
  const track: Record<string, unknown> = {
    id: opts.id ?? 1,
    title: `T${opts.id ?? 1}`,
    artists: [],
    codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
    audio: { id: 100, size: 1000, url: `/api/v1/tracks/${opts.id ?? 1}/audio` },
    ...(opts.duration !== undefined ? { duration: opts.duration } : {}),
    ...(opts.extraTrack ?? {}),
  };
  return {
    id: `t${opts.id ?? 1}`,
    trackId: opts.id ?? 1,
    source: { kind: 'http-range', url: `/api/v1/tracks/${opts.id ?? 1}/audio`, byteSize: 1000 },
    ...(opts.omitHint ? {} : { durationHintSeconds: opts.hint }),
    title: `T${opts.id ?? 1}`,
    artist: 'A',
    albumTitle: 'AL',
    track,
  };
}

function memoryInner(): StoragePort & { dump: () => string | null } {
  let v: string | null = null;
  return {
    get: () => v,
    set: (n: string | null) => {
      v = n;
    },
    dump: () => v,
  };
}

// ---- A: byte-stable passthrough ------------------------------------------

describe('repairingStorage (restore-time duration-hint repair)', () => {
  it('A: modern snapshot with valid hints round-trips byte-for-byte', () => {
    const payload = {
      v: 2,
      items: [
        wireItem({ id: 1, hint: 191.12959, duration: 191.12959 }),
        wireItem({ id: 2, hint: 198.5, duration: 200 }), // stale-but-valid wins
        wireItem({ id: 3, hint: 3.25, duration: undefined }),
      ],
      index: 0,
      positionSeconds: 12,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
      crossfadeSeconds: 0,
    };
    const raw = JSON.stringify(payload);
    const inner = memoryInner();
    inner.set(raw);
    const port = repairingStorage(inner);
    // Nothing missing => decode+map produce no changes => ORIGINAL string.
    expect(port.get()).toBe(raw);
    // Idempotent: repeated reads converge identically.
    expect(port.get()).toBe(raw);
  });

  // ---- B: repair from Track.duration -------------------------------------

  it('B: hydrates missing hints from positive track.duration (THRASHER-like)', () => {
    // Real durations from the canonical reproduction package.
    const DURS = [191.12959, 198.01533, 250.92846];
    const payload = {
      v: 2,
      items: [
        wireItem({ id: 1, omitHint: true, duration: DURS[0] }),
        wireItem({ id: 2, omitHint: true, duration: DURS[1] }),
        // Present-but-valid hint must survive repair untouched…
        wireItem({ id: 3, hint: 20, duration: DURS[2] }),
      ],
      index: 0,
      positionSeconds: 30,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
    };
    const inner = memoryInner();
    inner.set(JSON.stringify(payload));
    const out = decodeSnapshot(repairingStorage(inner).get())!;
    expect(out).not.toBeNull();
    expect((out.items[0] as ItemLike).durationHintSeconds).toBe(DURS[0]);
    expect((out.items[1] as ItemLike).durationHintSeconds).toBe(DURS[1]);
    expect((out.items[2] as ItemLike).durationHintSeconds).toBe(20);

    // Cumulative offsets downstream can no longer collapse: simulate the
    // lengthOf() fallback across ALL SIX thrasher-like successors.
    const six = [191.12959, 198.01533, 250.92846, 231.35172, 226.43998, 295.01143].map((s, i) =>
      wireItem({ id: i + 1, omitHint: true, duration: s }),
    );
    const sixRaw = JSON.stringify({
      v: 2,
      items: six,
      index: 0,
      positionSeconds: 0,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
    });
    const sixInner = memoryInner();
    sixInner.set(sixRaw);
    const repaired = decodeSnapshot(
      repairingStorage(sixInner).get(),
    ) as { items: Array<{ durationHintSeconds?: number }> };
    const lens = repaired.items.map((i) => Math.floor((i.durationHintSeconds ?? 0) * 44100));
    expect(lens.every((l) => l > 0)).toBe(true);
    const offs: number[] = [];
    let acc = 0;
    for (let i = 0; i < lens.length; i++) {
      offs.push(acc);
      acc += lens[i]!;
    }
    const monotonic = offs.every((o, i) => i === 0 || o > offs[i - 1]!);
    expect(monotonic).toBe(true); // no shared boundary instants remain
  });

  it('C: existing positive hints are never overwritten (even if stale)', () => {
    const payload = {
      v: 2,
      items: [
        wireItem({ id: 1, hint: 999, duration: 100 }), // stale wins over truth
        wireItem({ id: 2, hint: 0.0000001, duration: 500 }), // still positive
      ],
      index: 0,
      positionSeconds: 0,
      volume: 1,
      normalizeMode: 'off',
      repeat: 'off',
      shuffle: false,
    };
    const raw = JSON.stringify(payload);
    const inner = memoryInner();
    inner.set(raw);
    expect(repairingStorage(inner).get()).toBe(raw);
  });

  // ---- D: no invention ----------------------------------------------------

  it('D: unavailable/invalid track.duration never produces an invented hint', () => {
    for (const badDur of [undefined, 0, -3, Number.NaN]) {
      const payload = {
        v: 2,
        items: [wireItem({ id: 1, omitHint: true, duration: badDur })],
        index: 0,
        positionSeconds: 0,
        volume: 1,
        normalizeMode: 'off',
        repeat: 'off',
        shuffle: false,
      };
      const raw = JSON.stringify(payload);
      const inner = memoryInner();
      inner.set(raw);
      const out = decodeSnapshot(repairingStorage(inner).get())!;
      const item = out.items[0] as ItemLike;
      expect('durationHintSeconds' in item && item.durationHintSeconds !== undefined).toBe(false);
      expect(item.durationHintSeconds).toBeUndefined();
    }
  });

  it('B2: invalid hints (NaN/negative/zero) ARE replaced when truth exists', () => {
    const payload = {
      v: 2,
      items: [
        wireItem({ id: 1, hint: Number.NaN, duration: 111 }),
        wireItem({ id: 2, hint: -4, duration: 222 }),
        wireItem({ id: 3, hint: 0, duration: 333 }),
      ],
      index: 0,
      positionSeconds: 0,
      volume: 1,
      normalizeMode: 'off',
      repeat: 'off',
      shuffle: false,
    };
    const inner = memoryInner();
    inner.set(JSON.stringify(payload));
    const out = decodeSnapshot(repairingStorage(inner).get())!;
    expect((out.items[0] as ItemLike).durationHintSeconds).toBe(111);
    expect((out.items[1] as ItemLike).durationHintSeconds).toBe(222);
    expect((out.items[2] as ItemLike).durationHintSeconds).toBe(333);
  });

  // ---- E: corrupt payloads pass through -----------------------------------

  it('E: corrupt/non-snapshot payloads pass through untouched', () => {
    const inner = memoryInner();
    const port = repairingStorage(inner);
    for (const junk of ['not json {', '{"v":99,"items":[]}', '""']) {
      inner.set(junk);
      expect(port.get()).toBe(junk);
    }
    // set() is pure passthrough regardless of content.
    // set() is pure passthrough regardless of content.
    port.set('{"v":99}');
    expect(inner.dump()).toBe('{"v":99}');
    port.set(null);
    expect(inner.dump()).toBeNull();
  });

  // ---- F: waveform is not authority ---------------------------------------

  it('F: waveform envelope presence does NOT authorize a fabricated hint', () => {
    const payload = {
      v: 2,
      items: [
        wireItem({
          id: 1,
          omitHint: true,
          duration: undefined,
          extraTrack: { waveform: { points: 1912, intervalMs: 100, version: 1 } },
        }),
      ],
      index: 0,
      positionSeconds: 0,
      volume: 1,
      normalizeMode: 'off',
      repeat: 'off',
      shuffle: false,
    };
    const raw = JSON.stringify(payload);
    const inner = memoryInner();
    inner.set(raw);
    // Only server-side ingest backfill may introduce these durations.
    expect(repairingStorage(inner).get()).toBe(raw);
    expect(decodeSnapshot(raw)).not.toBeNull(); // payload itself restorable
  });
});

// ---- J: production wiring through PlayerController -----------------------

describe('controller restore applies repairingStorage (default port wiring)', () => {
  const KEY = 'musicpack.player.v1';
  let backing = new Map<string, string>();
  const shim = {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
  };

  beforeEach(() => {
    backing = new Map();
    (globalThis as { localStorage?: unknown }).localStorage = shim;
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
  });

  async function bootController() {
    const [{ PlayerController }, { createQueueStore }] = await Promise.all([
      import('../../app/src/lib/playback/controller'),
      import('../../app/src/lib/state/queue'),
    ]);
    const store = createQueueStore();
    const player = new PlayerController(store as never, {});
    player.init();
    await new Promise((r) => setTimeout(r, 0));
    return { player, store };
  }

  it('restores a hint-less payload repaired from track.duration', async () => {
    const DURS = [191.12959, 198.01533];
    const items = DURS.map((d, i) => wireItem({ id: i + 1, omitHint: true, duration: d }));
    backing.set(KEY, JSON.stringify({
      v: 2,
      items,
      index: 0,
      positionSeconds: 10,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
      crossfadeSeconds: 0,
    }));
    const { store } = await bootController();
    expect(store.get().items[0]?.durationHintSeconds).toBe(DURS[0]);
    expect(store.get().items[1]?.durationHintSeconds).toBe(DURS[1]);
  });

  it('heals storage after a natural (non-mutating) queue persist', async () => {
    const items = [wireItem({ id: 1, omitHint: true, duration: 191.12959 })];
    backing.set(KEY, JSON.stringify({
      v: 2,
      items,
      index: 0,
      positionSeconds: 10,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
      crossfadeSeconds: 0,
    }));
    const { store } = await bootController();

    // Trailing persist without touching the player-owned mutating flag.
    store.enqueue({
      ...wireItem({ id: 9, hint: 42, duration: 42 }),
      trackId: 9,
    } as never);
    await new Promise((r) => setTimeout(r, 5));

    const healedRaw = backing.get(KEY)!;
    expect(healedRaw).toContain('"durationHintSeconds":191.12959');
    const second = decodeSnapshot(healedRaw)!;
    expect((second.items[0] as ItemLike).durationHintSeconds).toBe(191.12959);
    expect(second.items).toHaveLength(2);
    expect(store.get().items).toHaveLength(2);
  });

  it('second restore over healed storage is idempotent', async () => {
    const items = [wireItem({ id: 1, omitHint: true, duration: 191.12959 })];
    backing.set(KEY, JSON.stringify({
      v: 2,
      items,
      index: 0,
      positionSeconds: 10,
      volume: 1,
      normalizeMode: 'album',
      repeat: 'off',
      shuffle: false,
      crossfadeSeconds: 0,
    }));
    await bootController();
    const once = backing.get(KEY)!;
    // Fresh controller over whatever is stored now (as a reload would see).
    await bootController();
    const twice = backing.get(KEY)!;
    const a = decodeSnapshot(once)!.items.map((i) => (i as ItemLike).durationHintSeconds);
    const b = decodeSnapshot(twice)!.items.map((i) => (i as ItemLike).durationHintSeconds);
    expect(b).toEqual(a);
    expect(b[0]).toBe(191.12959);
  });
});

// ---- Synthetic regression corpus (THRASHER-like geometry, small) ----------

describe('thrasher-like synthetic corpus integrity', () => {
  const ROOT = fileURLToPath(new URL('../../../tests/reference/', import.meta.url));
  const DIR = `${ROOT}thrasher-like-10x3s.mpack`;

  it.skipIf(!existsSync(DIR))('manifest carries ten positive authored durations', () => {
    const m = JSON.parse(readFileSync(`${DIR}/manifest.json`, 'utf8'));
    const tracks = m.media.flatMap((d: { tracks: Array<{ duration?: number }> }) => d.tracks);
    expect(tracks).toHaveLength(10);
    for (const t of tracks) {
      expect(typeof t.duration).toBe('number');
      expect(t.duration).toBeGreaterThan(0);
    }
  });
});
