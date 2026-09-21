// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Lyrics controller unit tests (R3.4, docs/musicpack-lyrics-v1.md §10/§13
// web-unit layer): the state machine around refs → bytes → wasm parse →
// position ticks. The wasm binding and the network enter as fakes; the
// parser itself is Rust-tested (R3.2) and the e2e suite covers the real
// module.

import { describe, expect, it, vi } from 'vitest';
import { LyricsController, withinTrackMs, type LyricsCore } from '../../app/src/lib/playback/lyrics';
import type { LyricsRef } from '../../app/src/lib/api/types';

const REF_A: LyricsRef = {
  id: 11,
  url: '/api/v1/assets/11',
  size: 42,
  mimeType: 'text/plain',
  sha256: 'aa'.repeat(32),
  lang: 'en',
};
const REF_B: LyricsRef = { id: 12, url: '/api/v1/assets/12', size: 7, mimeType: 'text/plain' };
const REF_FR: LyricsRef = { ...REF_A, id: 13, url: '/api/v1/assets/13', lang: 'fr' };

const SYNCED_LRC = '[ti:One]\n[00:01.00]first line\n[00:04.00]second line\n';
const PLAIN_TXT = 'just words\nmore words\n';

/** A minimal LRC-ish decoder mirroring the wasm core's observable
 *  contract: open→doc→activeLine→close over handles; malformed input
 *  throws. Lines starting `[mm:ss.xx]` become timed (synced). */
function fakeCore(): LyricsCore & {
  opened: number[];
  closed: number[];
} {
  let next = 1;
  const docs = new Map<number, { synced: boolean; lines: Array<{ t?: number; text: string }> }>();
  return {
    opened: [],
    closed: [],
    open(bytes) {
      const text = new TextDecoder().decode(bytes);
      if (text === 'MALFORMED [') throw new Error('malformed timestamp at line 1');
      // Metadata tags ([ti:, [ar:, …) are captured by the real core and
      // never surface as content lines; mirror that here.
      const lines = text
        .split('\n')
        .filter((l) => l.length > 0 && !/^\[[a-z]+:/i.test(l))
        .map((l) => {
          const m = /^\[(\d+):(\d+)\.(\d+)\](.*)$/.exec(l);
          if (!m) return { text: l };
          const t = Number(m[1]) * 60_000 + Number(m[2]) * 1000 + Number(m[3]);
          return { t, text: m[4] ?? '' };
        });
      const synced = lines.some((l) => l.t !== undefined);
      const handle = next++;
      docs.set(handle, { synced, lines });
      this.opened.push(handle);
      return handle;
    },
    doc(handle) {
      const d = docs.get(handle);
      if (!d) throw new Error('invalid lyrics handle');
      return JSON.stringify({
        synced: d.synced ? 1 : 0,
        wordTagsStripped: 0,
        lines: d.lines.map((l) => (l.t !== undefined ? { t: l.t, text: l.text } : { text: l.text })),
      });
    },
    activeLine(handle, positionMs) {
      const d = docs.get(handle);
      if (!d) throw new Error('invalid lyrics handle');
      let active = -1;
      d.lines.forEach((l, i) => {
        if (l.t !== undefined && l.t <= positionMs) active = i;
      });
      return active;
    },
    close(handle) {
      docs.delete(handle);
      this.closed.push(handle);
    },
  };
}

interface Harness {
  controller: LyricsController;
  core: ReturnType<typeof fakeCore>;
  fetches: string[];
  resolveFetch: (url: string, body?: string | Error) => void;
}

/** Wires a controller whose fetches settle only when the test resolves
 *  them (deterministic async); `instant` settles on the next microtask. */
function harness(opts: { instant?: boolean; cacheCapacity?: number } = {}): Harness {
  const core = fakeCore();
  const fetches: string[] = [];
  const pending = new Map<string, (v: Uint8Array | Error) => void>();
  const controller = new LyricsController({
    fetchAsset: (ref) => {
      fetches.push(ref.url);
      if (opts.instant) {
        const bytes = ref.url.includes('11')
          ? SYNCED_LRC
          : ref.url.includes('13')
            ? SYNCED_LRC.replace('One', 'Un')
            : PLAIN_TXT;
        return Promise.resolve(new TextEncoder().encode(bytes));
      }
      return new Promise((resolve, reject) => {
        pending.set(ref.url, (body) => {
          if (body instanceof Error) reject(body);
          else resolve(body);
        });
      });
    },
    core: async () => core,
    cacheCapacity: opts.cacheCapacity,
  });
  return {
    controller,
    core,
    fetches,
    resolveFetch: (url, body = SYNCED_LRC) => {
      const r = pending.get(url);
      if (!r) throw new Error(`no pending fetch for ${url}`);
      pending.delete(url);
      r(body instanceof Error ? body : new TextEncoder().encode(body));
    },
  };
}

const view = (c: LyricsController) => c.get();

describe('lyrics controller — states', () => {
  it('reports no-lyrics without refs and never fetches or loads the core', () => {
    const h = harness();
    h.controller.setTrack(1, undefined);
    h.controller.setTrack(2, []);
    expect(view(h.controller).state).toBe('no-lyrics');
    expect(h.fetches).toEqual([]);
    h.controller.setTrack(null, [REF_A]);
    expect(view(h.controller).state).toBe('no-lyrics');
  });

  it('loads and exposes a synced document (lines, timestamps, active −1)', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_A]);
    await Promise.resolve();
    await Promise.resolve();
    const v = view(h.controller);
    expect(v.state).toBe('synced');
    expect(v.isSynced).toBe(true);
    expect(v.activeIndex).toBe(-1); // before the first timestamp (§9)
    expect(v.lines.map((l) => l.text)).toEqual(['first line', 'second line']);
    expect(v.lines[0]?.timestampMs).toBe(1000);
    expect(v.lang).toBe('en');
  });

  it('exposes a plain document as plain (never styled as synced)', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_B]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    const v = view(h.controller);
    expect(v.isSynced).toBe(false);
    expect(v.activeIndex).toBe(-1);
    expect(v.lines.map((l) => l.text)).toEqual(['just words', 'more words']);
  });

  it('shows loading while the bytes are in flight', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_A]);
    expect(view(h.controller).state).toBe('loading');
    h.resolveFetch(REF_A.url);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
  });

  it('treats a parsed document with zero lines as plain-empty, not error', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_B]);
    h.resolveFetch(REF_B.url, '');
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    expect(view(h.controller).lines).toEqual([]);
  });
});

describe('lyrics controller — selection', () => {
  it('selects the first reference in server order (spec §5)', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_A, REF_FR, REF_B]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    expect(h.fetches).toEqual([REF_A.url]);
    expect(view(h.controller).lang).toBe('en');
  });

  it('carries lang metadata only when the server supplied it', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_B]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    expect(view(h.controller).lang).toBeUndefined();
  });
});

describe('lyrics controller — errors degrade, never throw', () => {
  it('maps a missing asset (404) to the error view with retry metadata', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_A]);
    h.resolveFetch(REF_A.url, new Error('not_found'));
    await vi.waitFor(() => expect(view(h.controller).state).toBe('error'));
    expect(view(h.controller).message).toContain('not_found');
  });

  it('maps network failure to the error view', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_A]);
    h.resolveFetch(REF_A.url, new TypeError('fetch failed'));
    await vi.waitFor(() => expect(view(h.controller).state).toBe('error'));
  });

  it('maps malformed content (wasm open throws) to the error view', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_A]);
    h.resolveFetch(REF_A.url, 'MALFORMED [');
    await vi.waitFor(() => expect(view(h.controller).state).toBe('error'));
  });

  it('retry refetches after an error and can succeed', async () => {
    const h = harness();
    h.controller.setTrack(7, [REF_A]);
    h.resolveFetch(REF_A.url, new Error('boom'));
    await vi.waitFor(() => expect(view(h.controller).state).toBe('error'));
    h.controller.retry();
    expect(view(h.controller).state).toBe('loading');
    h.resolveFetch(REF_A.url);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
  });
});

describe('lyrics controller — track lifecycle and races', () => {
  it('track A → track B installs B', async () => {
    const h = harness();
    h.controller.setTrack(1, [REF_A]);
    h.controller.setTrack(2, [REF_B]);
    h.resolveFetch(REF_B.url, PLAIN_TXT);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    expect(h.fetches).toEqual([REF_A.url, REF_B.url]);
    // A's response is still pending; when it lands it must be dropped.
    h.resolveFetch(REF_A.url);
    expect(view(h.controller).state).toBe('plain');
    expect(view(h.controller).isSynced).toBe(false); // B (plain) stays installed
  });

  it('a stale response for the previous track cannot overwrite the view (A→B)', async () => {
    const h = harness();
    h.controller.setTrack(1, [REF_A]);
    h.controller.setTrack(2, [REF_B]);
    h.resolveFetch(REF_A.url); // A resolves late
    h.resolveFetch(REF_B.url, PLAIN_TXT);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    expect(h.fetches).toEqual([REF_A.url, REF_B.url]);
    expect(view(h.controller).isSynced).toBe(false); // B (plain) stays installed
  });

  it('the same guard holds for B→A and rapid switching', async () => {
    const h = harness();
    h.controller.setTrack(2, [REF_B]);
    h.controller.setTrack(1, [REF_A]);
    h.controller.setTrack(2, [REF_B]);
    h.controller.setTrack(1, [REF_A]);
    h.resolveFetch(REF_B.url, PLAIN_TXT);
    h.resolveFetch(REF_A.url);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    // Only the LAST setTrack (track 1) may win; both B fetches are stale.
    expect(view(h.controller).lang).toBe('en');
  });

  it('clears lyrics when moving from a lyric-bearing track to one without', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(1, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setTrack(2, undefined);
    expect(view(h.controller).state).toBe('no-lyrics');
    expect(view(h.controller).lines).toEqual([]);
  });
});

describe('lyrics controller — caching', () => {
  it('does not refetch the same asset identity across tracks', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(1, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setTrack(2, [{ ...REF_A }]); // same id+sha, different track
    expect(view(h.controller).state).toBe('synced');
    expect(h.fetches).toEqual([REF_A.url]);
  });

  it('refetches when the content hash changes (id reused, new sha)', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(1, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setTrack(1, [{ ...REF_A, sha256: 'bb'.repeat(32) }]);
    await vi.waitFor(() => expect(h.fetches.length).toBe(2));
  });

  it('evicts the oldest document beyond capacity and closes its handle', async () => {
    const h = harness({ instant: true, cacheCapacity: 1 });
    h.controller.setTrack(1, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setTrack(2, [REF_B]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    // The evicted document (A) was closed through the core.
    expect(h.core.closed).toEqual([h.core.opened[0]]);
  });

  it('dispose closes every cached handle and resets the view', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(1, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setTrack(2, [REF_B]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('plain'));
    h.controller.dispose();
    expect(view(h.controller).state).toBe('no-lyrics');
    expect(h.core.closed.length).toBe(h.core.opened.length);
  });
});

describe('lyrics controller — position feed (§9)', () => {
  it('recomputes the active line from pushed milliseconds', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    h.controller.setPositionMs(1500);
    expect(view(h.controller).activeIndex).toBe(0);
    h.controller.setPositionMs(4000);
    expect(view(h.controller).activeIndex).toBe(1);
    // Backwards seek recomputes identically (no hysteresis).
    h.controller.setPositionMs(0);
    expect(view(h.controller).activeIndex).toBe(-1);
  });

  it('does not churn the store when the index is unchanged', async () => {
    const h = harness({ instant: true });
    h.controller.setTrack(7, [REF_A]);
    await vi.waitFor(() => expect(view(h.controller).state).toBe('synced'));
    let emissions = 0;
    const unsub = h.controller.subscribe(() => (emissions += 1));
    const baseline = emissions; // the store contract emits once on subscribe
    h.controller.setPositionMs(1500);
    h.controller.setPositionMs(1999);
    h.controller.setPositionMs(1999.5);
    expect(view(h.controller).activeIndex).toBe(0);
    expect(emissions - baseline).toBe(1); // only the index change emits
    unsub();
  });

  it('is inert for plain documents and before the core resolves', () => {
    const h = harness();
    h.controller.setPositionMs(1000); // no track/core at all
    expect(view(h.controller).state).toBe('no-lyrics');
    h.controller.setTrack(7, [REF_B]);
    h.resolveFetch(REF_B.url, PLAIN_TXT);
    return vi.waitFor(() => {
      expect(view(h.controller).state).toBe('plain');
      h.controller.setPositionMs(4000); // plain: no active-line concept
      expect(view(h.controller).activeIndex).toBe(-1);
    });
  });
});

describe('withinTrackMs (§9 position mapping)', () => {
  const snap = {
    currentTrackId: 9,
    currentTrackStartSeconds: 100,
    currentTrackDurationSeconds: 10,
    positionSeconds: 103.5,
  };

  it('maps the album-absolute position into floored track milliseconds', () => {
    expect(withinTrackMs(snap, 9)).toBe(3500);
  });

  it('returns null when another track is playing', () => {
    expect(withinTrackMs(snap, 8)).toBeNull();
    expect(withinTrackMs({ ...snap, currentTrackId: undefined }, 9)).toBeNull();
    expect(withinTrackMs(snap, null)).toBeNull();
  });

  it('clamps to the track window', () => {
    expect(withinTrackMs({ ...snap, positionSeconds: 50 }, 9)).toBe(0);
    expect(withinTrackMs({ ...snap, positionSeconds: 500 }, 9)).toBe(10_000);
  });

  it('floors partial milliseconds (§9 conversion rule)', () => {
    expect(withinTrackMs({ ...snap, positionSeconds: 103.9999 }, 9)).toBe(3999);
  });
});
