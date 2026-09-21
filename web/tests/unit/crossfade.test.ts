// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import { Player } from '../../player-core/src/player';
import { createQueueModel } from '../../player-core/src/queue';
import type { PlayerEvent } from '../../player-core/src/events';
import type { CrossfadeResult } from '../../player-core/src/engine';
import type { EngineStreamInfo } from '../../app/src/lib/playback/musepack-engine';
import type { PlaybackItem } from '../../player-core/src/types';

const RATE = 44100;
const LEN = RATE * 30; // 30 s tracks

const FADE_OK: CrossfadeResult = {
  info: { rate: RATE, channels: 2, version: 0, lengthSamples: LEN },
  overlapFrames: 0,
};

function item(n: number): PlaybackItem {
  return {
    id: `t${n}`,
    trackId: n,
    source: { kind: 'http-range', url: `/api/v1/tracks/${n}/audio`, byteSize: 100 },
    title: `T${n}`,
    artist: 'A',
    albumTitle: 'AL',
  };
}

/** Item with an explicit decoded length in seconds (short-track tests). */
function mk(n: number, secs: number): PlaybackItem {
  return { ...item(n), durationHintSeconds: secs };
}

interface Harness {
  handlers: {
    primed(): void;
    buffering(): void;
    eos(): void;
    error(m: string): void;
    tick(): void;
  };
  beginCrossfade: ReturnType<typeof vi.fn>;
  rendered: number;
  /** Set while a deferred fade is pending (opts.deferredFade). */
  resolveFade?: ((r: CrossfadeResult | null) => void) | null;
  /** Held-open advance() resolver (opts.advanceHold): simulate the real
   *  engine's async standby promotion so ticks can be delivered INSIDE
   *  the eos boundary window. */
  resolveAdvance?: (() => void) | null;
  /** Internal once-latch for advanceHold. */
  advanceArmed?: boolean;
}

/** Fake engine with controllable crossfade support. */
function makePlayer(opts: {
  crossfadeCapable: boolean;
  fadeResult?: CrossfadeResult | null;
  /** Overlap reported by a successful fade (default 0 = legacy accounting). */
  overlapFrames?: number;
  /** Hold the first beginCrossfade call open until resolveFade is invoked. */
  deferredFade?: boolean;
  /** Item id the engine's standby was prepared with. beginCrossfade refuses
   *  (returns null) when the requested target differs — standby/policy
   *  agreement, as the real engines enforce. */
  standbyItemId?: string;
  /** Content-aware policy port under test (Sweet Fades). */
  planTransition?: ConstructorParameters<typeof Player>[1]['planTransition'];
  /** Simulate a failed/gapped standby: prepareNext never yields a length,
   *  so every successor's offset collapses onto the previous track's end. */
  standbyFails?: boolean;
  /** Hold advance() open until harness.resolveAdvance() fires — models
   *  the promote-gap between decoder eos and cursor next(). */
  advanceHold?: boolean;
}): { player: Player; h: Harness; queue: ReturnType<typeof createQueueModel> } {
  const queue = createQueueModel({ rng: () => 0.5 });
  /** Exact decoded length the fake engine reports for an item. */
  const infoFor = (it: PlaybackItem) => ({
    rate: RATE,
    channels: 2,
    version: 0,
    lengthSamples: Math.floor(((it.durationHintSeconds ?? 30) as number) * RATE),
  });
  const h: Harness = {
    handlers: {} as Harness['handlers'],
    // NOTE: `??` would treat null as absent — but null means "the engine
    // DECLINED the fade" and must be returned as-is.
    beginCrossfade: vi.fn(
      async (it: PlaybackItem, _seconds: number): Promise<CrossfadeResult | null> => {
        // Standby/policy agreement (mirrors the real engines): a fade may
        // only start for the item the standby was actually prepared with.
        if (opts.standbyItemId !== undefined && opts.standbyItemId !== it.id) {
          return null;
        }
        if (opts.deferredFade) {
          if (!h.resolveFade) {
            return new Promise<CrossfadeResult | null>((resolve) => {
              h.resolveFade = (r) => {
                h.resolveFade = null;
                resolve(r);
              };
            });
          }
          return null;
        }
        if (opts.fadeResult !== undefined) return opts.fadeResult;
        return {
          info: infoFor(it),
          overlapFrames: opts.overlapFrames ?? 0,
        };
      },
    ),
    rendered: 0,
  };
  const player = new Player(queue as never, {
    engineFactory: (_kind, handlers) => {
      h.handlers = handlers;
      return {
        capabilities: {
          preloadNext: true,
          sampleAccurateGapless: false,
          decodeGate: false,
          crossfade: opts.crossfadeCapable,
        },
        init: async () => undefined,
        open: async (it: PlaybackItem) => infoFor(it),
        prepareNext: async (it: PlaybackItem) => (opts.standbyFails ? null : infoFor(it)),
        advance: async (expected?: PlaybackItem | null) => {
          if (opts.advanceHold && h.resolveAdvance === undefined && h.advanceArmed !== true) {
            h.advanceArmed = true;
            return new Promise<EngineStreamInfo | null>((resolve) => {
              h.resolveAdvance = () => {
                h.resolveAdvance = null;
                resolve(expected ? infoFor(expected!) : null);
              };
            });
          }
          // Standby/policy agreement: without a prepared-item model this
          // fake promotes only when an expectation was supplied (the real
          // core always supplies one at EOS).
          return expected ? infoFor(expected) : null;
        },
        play: async () => undefined,
        pause: async () => undefined,
        seekSample: async () => undefined,
        startPumping: () => undefined,
        pausePumping: () => undefined,
        setGain: () => undefined,
        renderedSamples: () => h.rendered,
        close: async () => undefined,
        on: () => () => undefined,
        beginCrossfade: h.beginCrossfade,
      };
    },
    resolveKind: () => 'musepack',
    storage: { get: () => null, set: () => undefined },
    now: () => Date.now(),
    schedulePersist: () => 0,
    planTransition: opts.planTransition,
  });
  return { player, h, queue };
}

async function flush(times = 3): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}

function primeAndPlay(h: Harness): Promise<void> {
  h.handlers.primed();
  return flush();
}

describe('crossfade trigger semantics (M8 Phase A)', () => {
  it('refuses a fade when the standby does not match policy and falls back to the normal boundary', async () => {
    // Standby/policy agreement: the fake engine's standby was prepared for
    // t9 (a stale item), so beginCrossfade(t2) must be declined and the
    // positional trigger must leave the boundary to the EOS path.
    const { player, h, queue } = makePlayer({
      crossfadeCapable: true,
      standbyItemId: 't9',
    });
    player.init();
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);

    h.rendered = 25 * RATE; // inside track 1's fade window
    h.handlers.tick();
    await flush();

    expect(h.beginCrossfade).toHaveBeenCalledOnce();
    expect(h.beginCrossfade.mock.calls[0]?.[0].id).toBe('t2'); // policy target
    // Fade declined: cursor untouched, no error, no duplicate trigger.
    expect(player.model.get().current?.id).toBe('t1');
    expect(player.model.get().state).toBe('playing');

    // The natural boundary still advances exactly once.
    h.handlers.eos();
    await flush();
    expect(queue.get().index).toBe(1);
    expect(player.model.get().current?.id).toBe('t2');
    expect(player.model.get().state).not.toBe('error');
  });

  it('fires once near the end of the current track and advances cursor/policy', async () => {
    const { player, h, queue } = makePlayer({ crossfadeCapable: true });
    player.init();
    const tracks: (PlaybackItem | null)[] = [];
    player.on((e) => { if (e.t === 'track') tracks.push(e.item); });

    // Multi-item queue: a single-item queue has no fade target by design.
    await player.playSequence([item(1), item(2), item(3)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    expect(queue.get().items.length).toBe(3);

    // render to within the 8 s window of track 1 (30 s track)
    h.rendered = 25 * RATE;
    h.handlers.tick();
    await flush();

    expect(h.beginCrossfade).toHaveBeenCalledOnce();
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBe(8);
    expect(tracks.some((t) => t?.id === 't2')).toBe(true);
    expect(player.model.get().current?.id).toBe('t2');
    expect(player.model.get().state).not.toBe('error');

    // Phase B: each track can fade once. The next tick lands in track 2's
    // own fade window (cursor advanced to t2), so a chained fade fires.
    h.rendered = 57 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(2);
  });

  it('does nothing when disabled (default)', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playItem(item(1));
    await primeAndPlay(h);
    h.rendered = 29 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
  });

  it('falls back when the engine lacks the capability', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: false });
    await player.playItem(item(1));
    player.setCrossfade(4);
    await primeAndPlay(h);
    h.rendered = 29 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
  });

  it('falls back and keeps state clean when beginCrossfade returns null', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true, fadeResult: null });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(4);
    await primeAndPlay(h);
    h.rendered = 29 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledOnce();
    // normal EOS path still owns advancement
    expect(player.model.get().current?.id).toBe('t1');
    expect(player.model.get().state).not.toBe('error');
  });

  it('never fades on the last track of the queue', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playItem(item(9)); // single-item queue
    player.setCrossfade(12);
    await primeAndPlay(h);
    h.rendered = 29 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
  });

  it('never fades a single-track repeat-all loop into itself', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playItem(item(5));
    player.setRepeat('all');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 29 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
  });

  it('never fades under repeat-one (BUG-5: reload semantics win)', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    // Multi-item queue so a fade target WOULD exist under the old policy.
    await player.playSequence([item(1), item(2), item(3)], 'AL', 'A');
    player.setRepeat('one');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE; // inside the 8 s window of track 1
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
  });

  it('emits a crossfade event on setCrossfade and persists the setting', () => {
    const { player } = makePlayer({ crossfadeCapable: true });
    const events: PlayerEvent[] = [];
    player.on((e) => events.push(e));
    player.setCrossfade(8);
    expect(events.some((e) => e.t === 'crossfade' && e.seconds === 8)).toBe(true);
    player.setCrossfade(6); // invalid -> clamps to 0
    expect(events.at(-1)).toEqual({ t: 'crossfade', seconds: 0 });
    expect(player.model.get().crossfadeSeconds).toBe(0);
  });

  it('shrinks the outgoing track by the reported overlap (BUG-2: album clock)', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true, overlapFrames: 8 * RATE });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE; // inside the window
    h.handlers.tick();
    await flush();

    const m = player.model.get();
    expect(m.current?.id).toBe('t2');
    // t1 keeps 30 - 8 = 22 s of effective length: t2 starts at 22 s and the
    // album total compresses to 52 s. Positions, seek mapping and end
    // detection all key off these numbers.
    expect(m.currentTrackStartSeconds).toBeCloseTo(22, 3);
    expect(m.durationSeconds).toBeCloseTo(52, 3);
  });

  it('fix (c): does not report boundary-drift when the post-fade position matches the declared boundary', async () => {
    // The crossfade boundary is now fully authoritative (fix a): the real
    // worklet rebase in audio-worklet.ts guarantees the reported position
    // lands exactly on the boundary the fade just declared (22 s here, per
    // the BUG-2 test above). This simulates that healthy production case
    // directly — the engine reporting exactly 22 s on the very next render
    // tick — and pins that the new diagnostic stays silent for it.
    const { player, h } = makePlayer({ crossfadeCapable: true, overlapFrames: 8 * RATE });
    const events: PlayerEvent[] = [];
    player.on((e) => events.push(e));
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE; // inside the window
    h.handlers.tick();
    await flush();

    h.rendered = 22 * RATE; // exactly the boundary beginCrossfadeTransition declared
    h.handlers.tick();
    await flush();

    expect(events.some((e) => e.t === 'boundary-drift')).toBe(false);
  });

  it('fix (c): reports boundary-drift when the reported position disagrees with the declared boundary', async () => {
    // Inverse of the test above: if the engine's reported position ever
    // disagreed with the boundary the fade declared (a bug regression, or
    // an engine that doesn't implement fix (a)'s rebase), this must be
    // surfaced as an explicit, observable event instead of silently
    // falling through to the position-based catch-up heuristic below —
    // which would not even catch THIS direction of disagreement, since it
    // only ever adopts a LATER index, never an earlier one.
    const { player, h } = makePlayer({ crossfadeCapable: true, overlapFrames: 8 * RATE });
    const events: PlayerEvent[] = [];
    player.on((e) => events.push(e));
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE; // inside the window
    h.handlers.tick();
    await flush();

    h.rendered = 20 * RATE; // short of the declared 22 s boundary
    h.handlers.tick();
    await flush();

    const drift = events.find((e) => e.t === 'boundary-drift');
    expect(drift).toMatchObject({ expectedIndex: 1, observedIndex: 0, positionSamples: 20 * RATE });
    // The cursor itself is untouched by the diagnostic — it only ever
    // reports; the catch-up below cannot regress it, so t2 stays current.
    expect(player.model.get().current?.id).toBe('t2');
  });

  it('completes the handoff after a pause mid-fade and never re-triggers (BUG-4)', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true, deferredFade: true });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(4);
    await primeAndPlay(h);

    h.rendered = 27 * RATE; // inside the 4 s window
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledOnce();
    expect(h.resolveFade).toBeTruthy();

    // Pause while the engine-side fade is still awaiting its swap.
    await player.pause();

    // While the attempt is live, no second trigger may fire.
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledOnce();

    // The fade completes (e.g. after resume): the handoff bookkeeping must
    // still apply even though transportSeq moved with the pause.
    h.resolveFade!({ ...FADE_OK });
    await flush();
    expect(player.model.get().current?.id).toBe('t2');
    expect(h.resolveFade).toBeNull();
  });

  it('skips handoff bookkeeping when a load superseded the transition', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true, deferredFade: true });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(4);
    await primeAndPlay(h);

    h.rendered = 27 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledOnce();

    // A seek starts a new load generation while the fade awaits: the fade
    // result is stale and must NOT move the cursor/media/gain state.
    await player.seek(5);
    h.resolveFade!({ ...FADE_OK });
    await flush();

    expect(player.model.get().current?.id).toBe('t1');
    expect(player.model.get().state).not.toBe('error');
  });

  it('honors a gapless plan from the policy port (no fade at the boundary)', async () => {
    const planCalls: unknown[] = [];
    const { player, h } = makePlayer({
      crossfadeCapable: true,
      planTransition: (query) => {
        planCalls.push(query);
        return { type: 'gapless' };
      },
    });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled();
    expect(planCalls.length).toBeGreaterThan(0);
  });

  it('passes the planned overlap to the engine instead of the raw setting', async () => {
    const { player, h } = makePlayer({
      crossfadeCapable: true,
      planTransition: () => ({ type: 'sweet-fade', overlapSeconds: 2 }),
    });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.rendered = 25 * RATE; // remaining ~5 s: inside the 8 s cap, outside 2 s+lead
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).not.toHaveBeenCalled(); // not yet — 2 s + 1 s lead

    h.rendered = 28.5 * RATE; // remaining ~1.5 s ≤ 2 s + lead
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBe(2);
  });

  it('takes a last-chance fade when decoder EOS beats the positional trigger', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    // Decode raced ahead: worker EOS arrives while NO positional tick has
    // observed the window yet (rendered still at 0).
    h.handlers.eos();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    // Overlap clamped to what is actually left: min(cap 8 s, remaining 30 s).
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBe(8);
    expect(player.model.get().current?.id).toBe('t2');
  });

  it('falls back to the normal EOS handoff when the last-chance fade declines', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true, fadeResult: null });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.handlers.eos();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    // Declined: the gapless advance owns the boundary as before.
    expect(player.model.get().current?.id).toBe('t2');
    expect(player.model.get().state).not.toBe('error');
  });

  it('never takes the last-chance fade under repeat-one', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setRepeat('one');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.handlers.eos();
    await flush(6);
    expect(h.beginCrossfade).not.toHaveBeenCalled();
    // Reload semantics: same item stays current through a fresh load.
    expect(player.model.get().current?.id).toBe('t1');
  });

  it('consults the planner on the EOS-race path and uses the planned overlap', async () => {
    const { player, h } = makePlayer({
      crossfadeCapable: true,
      planTransition: () => ({ type: 'sweet-fade', overlapSeconds: 3 }),
    });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    // Decode raced ahead: worker EOS arrives before any positional tick.
    h.handlers.eos();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    // Planner's 3 s is used, NOT the raw 8 s cap.
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBe(3);
    expect(player.model.get().current?.id).toBe('t2');
  });

  it('clamps the planned EOS overlap to the audio actually left', async () => {
    const { player, h } = makePlayer({
      crossfadeCapable: true,
      // Ask for more than remains; the path must clamp to remaining audio.
      planTransition: () => ({ type: 'sweet-fade', overlapSeconds: 12 }),
    });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(12);
    await primeAndPlay(h);
    // Render most of the track so only ~5 s remains at the EOS.
    h.rendered = 25 * RATE;
    h.handlers.eos();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    // Clamped to remaining (~5 s), not the planned 12 s.
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBeCloseTo(5, 1);
    expect(h.beginCrossfade.mock.calls[0]?.[1] ?? 0).toBeLessThan(6);
  });

  it('declines an EOS gapless/hard-cut plan and hands off normally', async () => {
    const { player, h, queue } = makePlayer({
      crossfadeCapable: true,
      planTransition: () => ({ type: 'gapless' }),
    });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.handlers.eos();
    await flush();
    // No overlapped transition: the fade is never started.
    expect(h.beginCrossfade).not.toHaveBeenCalled();
    // The normal EOS handoff still advances exactly once.
    expect(queue.get().index).toBe(1);
    expect(player.model.get().current?.id).toBe('t2');
  });

  it('keeps the fixed-cap fallback on EOS when no planner is wired', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playSequence([item(1), item(2)], 'AL', 'A');
    player.setCrossfade(8);
    await primeAndPlay(h);
    h.handlers.eos();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalledTimes(1);
    // No port → legacy fixed behaviour: min(cap 8 s, remaining 30 s) = 8 s.
    expect(h.beginCrossfade.mock.calls[0]?.[1]).toBe(8);
  });

  it('progresses through short tracks into a normal track without stalling', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playSequence([mk(1, 48), mk(2, 1), mk(3, 1), mk(4, 48)], 'AL', 'A');
    player.setCrossfade(4);
    await primeAndPlay(h);

    // Boundary 1 (normal -> short): positional trigger near the opener end.
    h.rendered = 46.5 * RATE;
    h.handlers.tick();
    await flush();
    expect(h.beginCrossfade).toHaveBeenCalled();
    expect(player.model.get().current?.id).toBe('t2');

    // Boundaries 2 and 3 (short -> short -> normal): each decode-EOS lands
    // while audio remains, so the last-chance fade advances exactly one
    // track per EOS — never zero, never two.
    for (const id of ['t3', 't4']) {
      h.handlers.eos();
      await flush();
      expect(player.model.get().current?.id).toBe(id);
    }

    // Termination: the final track's EOS still ends the queue cleanly.
    h.rendered = 99 * RATE; // past the 98 s compressed total
    h.handlers.eos();
    await flush();
    h.handlers.tick();
    await flush();
    expect(player.model.get().state).toBe('ended');
  });

  it('handles several consecutive short tracks with a fade at every boundary', async () => {
    const { player, h } = makePlayer({ crossfadeCapable: true });
    await player.playSequence([mk(1, 1), mk(2, 1), mk(3, 1), mk(4, 1)], 'AL', 'A');
    player.setCrossfade(12); // window far longer than any track
    await primeAndPlay(h);

    // One logical advancement per decode-EOS, across every boundary,
    // including the final termination.
    for (const id of ['t2', 't3', 't4']) {
      h.handlers.eos();
      await flush();
      expect(player.model.get().current?.id).toBe(id);
    }
    const calls = h.beginCrossfade.mock.calls.length;
    expect(calls).toBeGreaterThanOrEqual(2); // faded, not merely gapless

    h.rendered = 10 * RATE; // past the 4 s total
    h.handlers.eos();
    await flush();
    h.handlers.tick();
    await flush();
    expect(player.model.get().state).toBe('ended');
  });

  // The tick() cursor-catch-up (player-core/player.ts) must not advance the
  // queue while a crossfade owns the boundary, or it races the transition
  // through extra tracks (legacy end-of-track random-jump bug).
  describe('tick cursor catch-up vs crossfade boundary ownership', () => {
    it('advances the cursor via tick catch-up when no crossfade is in progress', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false });
      player.init();
      await player.playSequence([item(1), item(2), item(3)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Render ahead into track 2's region while the cursor is still on track 1.
      // With no crossfade in progress, the catch-up syncs the cursor forward.
      h.rendered = 35 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(1);
    });

    // With every successor's length unknown (no hint + failed standby), the
    // offset table collapses onto the current track's end and currentIndexAt
    // maps ANY later position onto the LAST queue index. One tick past song
    // 1 then teleports the cursor to song 6 (legacy bug reproduced below);
    // the catch-up may adopt at most ONE proven step beyond the cursor.
    it('never rides a collapsed offset table to the last track', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, standbyFails: true });
      player.init();
      await player.playSequence([item(1), item(2), item(3), item(4), item(5), item(6)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Deep inside the collapsed region: every successor shares song 1's
      // end offset here. No proven boundary ahead -> stand down entirely;
      // EOS/load owns the handoff. Legacy code jumped straight to song 6.
      h.rendered = 91 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(0);
      expect(player.model.get().current?.id).toBe('t1');
    });

    it('caps the catch-up at exactly one step when the successor is provable', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, standbyFails: true });
      player.init();
      // Song 2 carries a real hint; songs 3-6 none. At 91 s the offset
      // table still reports song 6 (offs[2..5] share one instant), but
      // song 2's start is provable, so the cap must clamp to it.
      await player.playSequence([item(1), mk(2, 30), item(3), item(4), item(5), item(6)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      h.rendered = 91 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(1);
      // Cursor-only sync: model.current keeps lagging until onEos() runs
      // its full handoff bookkeeping.
      expect(player.model.get().current?.id).toBe('t1');
    });

    it('stands down while no successor duration is known', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, standbyFails: true });
      player.init();
      await player.playSequence([item(1), item(2), item(3), item(4), item(5), item(6)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Repeated far-future ticks across every collapsed instant: the
      // clock can never prove the next boundary, so the cursor must never
      // creep toward the last track.
      for (const secs of [31, 61, 91, 181, 600]) {
        h.rendered = secs * RATE;
        h.handlers.tick();
        await flush();
        expect(queue.get().index).toBe(0);
      }
    });

    it('does not move the cursor via tick catch-up while a crossfade is in progress', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([item(1), item(2), item(3)], 'AL', 'A');
      player.setCrossfade(8);
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Trigger the crossfade (held open by the deferred fade).
      h.rendered = 25 * RATE;
      h.handlers.tick();
      await flush();
      expect(h.beginCrossfade).toHaveBeenCalledOnce();

      // Render ahead into track 2's region. The catch-up must NOT fire because
      // the crossfade owns the boundary.
      h.rendered = 35 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(0); // cursor untouched by catch-up
    });

    it('advances exactly once when the crossfade completes', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([item(1), item(2), item(3)], 'AL', 'A');
      player.setCrossfade(8);
      await primeAndPlay(h);

      h.rendered = 25 * RATE;
      h.handlers.tick();
      await flush();
      expect(h.beginCrossfade).toHaveBeenCalledOnce();
      const callsBefore = h.beginCrossfade.mock.calls.length;

      // Catch-up must not fire while the fade is pending.
      h.rendered = 35 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(0);

      // Resolve the fade: the transition advances exactly once.
      h.resolveFade!(FADE_OK);
      await flush();
      expect(queue.get().index).toBe(1);
      expect(h.beginCrossfade.mock.calls.length).toBe(callsBefore); // no extra trigger
    });

    it('cannot sweep a short-track queue via tick catch-up during a crossfade', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([mk(1, 1), mk(2, 1), mk(3, 1), mk(4, 1)], 'AL', 'A');
      player.setCrossfade(4);
      await primeAndPlay(h);

      // Trigger the crossfade on the 1 s opener.
      h.rendered = 0.8 * RATE;
      h.handlers.tick();
      await flush();
      expect(h.beginCrossfade).toHaveBeenCalledOnce();

      // Attempt to sweep through the remaining tracks via repeated ticks.
      for (const pos of [1.5, 2.5, 3.5]) {
        h.rendered = pos * RATE;
        h.handlers.tick();
        await flush();
        expect(queue.get().index).toBe(0); // catch-up suppressed
      }

      // Resolve: exactly one advance to track 2.
      h.resolveFade!(FADE_OK);
      await flush();
      expect(queue.get().index).toBe(1);
    });

    it('cannot override the crossfade presentation-order successor via tick catch-up in shuffle', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([item(1), item(2), item(3), item(4)], 'AL', 'A');
      queue.setShuffle(true);
      queue.setPresentationOrderForTest([0, 2, 1, 3]); // next after t1 is t3 (canonical 2)
      player.setCrossfade(8);
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Trigger the crossfade; it targets the presentation successor t3 (index 2).
      h.rendered = 25 * RATE;
      h.handlers.tick();
      await flush();
      expect(h.beginCrossfade).toHaveBeenCalledOnce();
      expect(h.beginCrossfade.mock.calls[0]?.[0].id).toBe('t3');

      // Render into t2's canonical region. Without the guard the catch-up would
      // move the cursor to canonical t2 (index 1), overriding the shuffle target.
      h.rendered = 35 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(0); // catch-up suppressed, cursor stays on t1

      // Resolve: the transition advances to the shuffle successor t3 (index 2).
      h.resolveFade!(FADE_OK);
      await flush();
      expect(queue.get().index).toBe(2);
      expect(player.model.get().current?.id).toBe('t3');
    });

    // Duration-hint repair coexistence (see duration-repair-storage):
    // with every successor's hint present, offsets stay strictly increasing
    // and boundary ownership walks EXACTLY one track per handoff — the
    // pre-repair collapse geometry is impossible on healthy data.
    it('healthy hinted queues keep offsets increasing and walk sequentially', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30), mk(4, 30), mk(5, 30), mk(6, 30)], 'AL', 'A');
      await primeAndPlay(h);
      expect(player.model.get().durationSeconds).toBeCloseTo(180, 3);

      const starts: number[] = [0];
      for (let i = 1; i < 6; i++) {
        if (i === 1) {
          h.rendered = 31 * RATE;
          h.handlers.tick();
          await flush();
        } else {
          h.handlers.eos();
          await flush();
        }
        expect(queue.get().index).toBe(i); // exactly one step per boundary
        const start = player.model.get().currentTrackStartSeconds;
        expect(start).toBeCloseTo(30 * i, 3);
        starts.push(start);
        expect(starts[starts.length - 1]!).toBeGreaterThan(starts[starts.length - 2]!);
      }
      expect(player.model.get().durationSeconds).toBeCloseTo(180, 3);

      // Termination still ends the session cleanly at the drained end.
      h.rendered = 181 * RATE;
      h.handlers.eos();
      await flush();
      h.handlers.tick();
      await flush();
      expect(player.model.get().state).toBe('ended');
    });

    // Repair supplies truth for the IMMEDIATE successor while later tracks
    // stay unknown: the catch-up may adopt that one provable step (positive
    // length under it) yet must never ride the remaining collapsed region.
    it('adopting a repaired successor never unlocks the collapsed tail (coexistence)', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, standbyFails: true });
      player.init();
      await player.playSequence([item(1), mk(2, 25), item(3), item(4), item(5), item(6)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      for (const secs of [31, 61, 91, 181]) {
        h.rendered = secs * RATE;
        h.handlers.tick();
        await flush();
        expect(queue.get().index).toBe(1); // provable step taken once; tail locked
      }
      // EOS chain remains the sole owner past the provable boundary.
      for (const id of ['t3', 't4', 't5', 't6']) {
        h.handlers.eos();
        await flush();
        expect(queue.get().index).toBe(queue.get().items.findIndex((it) => it.id === id));
      }
    });
  });

  // Boundary ownership must hold for BOTH directions and BOTH movers.
  // previous() awaits a slow engine re-open while the OLD engine keeps
  // rendering the old (higher-offset) track; onEos() awaits engine.advance
  // BEFORE advancing the cursor. In both await-gaps a rendered tick sees
  // "position ahead of cursor" — the legacy catch-up adopted it, bouncing
  // backward navigation forward (random-feeling jumps) or letting the eos
  // advance land two tracks past the boundary ("next track skipped").
  describe('tick catch-up vs manual navigation and eos windows', () => {
    it('R1: ticks during a pending previous() never bounce toward the abandoned position', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false });
      player.init();
      // Start on song 4 of 6; load() already settled, within-track 0s.
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30), mk(4, 30), mk(5, 30), mk(6, 30)], 'AL', 'A', 3);
      await primeAndPlay(h);
      expect(queue.get().index).toBe(3);

      void player.previous(); // -> index 2, engine re-open pending
      // Stale OLD-frame render arrives mid-load: album clock still maps
      // into song 5's region (old base + high rendered).
      h.rendered = 121 * RATE;
      h.handlers.tick();
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(2);
      expect(player.model.get().current?.id).toBe('t3');
    });

    it('R2: repeated backward skips with interleaved stale ticks never creep forward', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30), mk(4, 30), mk(5, 30), mk(6, 30)], 'AL', 'A', 4);
      await primeAndPlay(h);
      expect(queue.get().index).toBe(4);

      void player.previous(); // target 3
      h.rendered = 155 * RATE; // maps into song 6's region vs the new base
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(3);
      const afterFirst = queue.get().index;

      void player.previous(); // target 2
      h.rendered = 125.5 * RATE;
      h.handlers.tick();
      await flush();
      expect(queue.get().index).toBe(2);
      expect(queue.get().index).toBeLessThanOrEqual(afterFirst - 1);
    });

    it('R3: a tick inside the eos advance-gap cannot steal the cursor advancement', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, advanceHold: true });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30)], 'AL', 'A');
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // EOS fired; onEos is parked INSIDE `await engine.advance(...)`
      // while the promoted stream has already begun sounding.
      h.handlers.eos();
      await flush();
      h.rendered = 31 * RATE;
      h.handlers.tick(); // promotion window tick — must NOT adopt qi+1
      await flush();

      h.resolveAdvance?.();
      await flush();
      expect(queue.get().index).toBe(1); // exactly one advance, owned by eos
      expect(player.model.get().current?.id).toBe('t2');
    });

    it('R5: repeat-one reload window likewise holds the cursor still', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false, advanceHold: true });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30)], 'AL', 'A');
      player.setRepeat('one');
      await primeAndPlay(h);

      h.handlers.eos();
      await flush();
      h.rendered = 31 * RATE; // during held advance/reload: would adopt t2
      h.handlers.tick();
      await flush();

      h.resolveAdvance?.();
      await flush();
      expect(queue.get().index).toBe(0);
      expect(player.model.get().current?.id).toBe('t1'); // reload semantics win
    });

    // Crossfade ON, chained boundaries (the 2 -> 3 -> "instantly" 4 report):
    // a second decoder eos arriving WHILE a fade owns the current boundary
    // must be swallowed by the owner, never run as a competing gapless
    // handoff over the same stale cursor/standby pair.
    it('R6: an eos arriving while a fade owns the boundary is swallowed', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30)], 'AL', 'A');
      player.setCrossfade(4);
      await primeAndPlay(h);
      expect(queue.get().index).toBe(0);

      // Positional trigger starts the 2->3 fade; beginCrossfade parks on
      // resolveFade (fade OWNS this boundary from here).
      h.rendered = 27 * RATE;
      h.handlers.tick();
      await flush();
      expect(h.beginCrossfade).toHaveBeenCalledOnce();

      // Duplicate/late eos lands during the held fade (synthetic-eos
      // misfire class). It must not run a second boundary handoff.
      h.handlers.eos();
      await flush();
      expect(queue.get().index).toBe(0);

      // Owner completes: exactly ONE advance results for the whole episode.
      h.resolveFade!({ ...FADE_OK });
      await flush();
      expect(queue.get().index).toBe(1);
      expect(player.model.get().current?.id).toBe('t2');
      expect(h.beginCrossfade).toHaveBeenCalledOnce(); // no re-entrant fade
    });

    it('R7: coalesced late eos after completion cannot stack a second advance', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: true, deferredFade: true });
      player.init();
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30)], 'AL', 'A');
      player.setCrossfade(4);
      await primeAndPlay(h);

      h.rendered = 27 * RATE;
      h.handlers.tick();
      await flush();
      void h.handlers.eos(); // duplicate DURING hold
      await flush();
      h.resolveFade!({ ...FADE_OK }); // owner finishes 1->2
      await flush();
      const cursorAfterOwner = queue.get().index;
      expect(cursorAfterOwner).toBe(1);

      h.handlers.eos(); // SAME window's duplicate arriving LATE
      await flush();
      h.handlers.eos(); // …and once more for good measure
      await flush();
      // Still one advancement total: duplicates belong to t1's spent eos.
      expect(queue.get().index).toBe(1);
      expect(player.model.get().current?.id).toBe('t2');
    });

    // EOS-race fade accounting: the blend starts AT decode-eos and the swap
    // happens after only `overlap` seconds — the buffered tail beyond the
    // blend window is discarded by the swap. The declared offsets must
    // place the successor at (raw end − remaining-at-eos), NOT at
    // (raw end − overlap); otherwise the engine clock runs ahead of the
    // declared offsets and the positional trigger instant-fades the
    // successor ("2 fades into 3, instantly 4").
    it('R8: eos-race fade declares the discarded tail in the offsets', async () => {
      const { player, h } = makePlayer({
        crossfadeCapable: true,
        planTransition: () => ({ type: 'sweet-fade', overlapSeconds: 1 }),
      });
      player.init();
      player.setCrossfade(4); // planner overlap capped to 1 s below
      await player.playSequence([mk(1, 30), mk(2, 30), mk(3, 30)], 'AL', 'A');
      await primeAndPlay(h);

      // Decoder eos arrives with 2 s still buffered (rendered = 28 s).
      h.rendered = 28 * RATE;
      h.handlers.eos();
      await flush();

      expect(player.model.get().current?.id).toBe('t2');
      // Declared start of track 2 = raw end (30) − remaining-at-eos (2) = 28.
      // (The old bookkeeping shrank by the 1 s blend → 29, and by overlap=0
      // fake variants → 30; both left a fade-window-sized wedge.)
      expect(player.model.get().currentTrackStartSeconds).toBeCloseTo(28, 1);
    });

    // Stale decode-eos after removing the PAUSED playing item: replacing the
    // discarded stream with open(successor) can still surface the old track's
    // end-of-stream. A natural-boundary handoff that fires on that stale eos
    // must NOT advance the cursor or restart audio — the pause intent stands.
    // Without the pauseIntent guard in runEosBoundary the handoff advanced to
    // the next track (leaving the e2e player in `loading`/`playing`), so the
    // paused removal test failed on slow CI.
    it('R9: a stale eos after removing the paused playing item does not advance the cursor', async () => {
      const { player, h, queue } = makePlayer({ crossfadeCapable: false });
      player.init();
      await player.playSequence([item(1), item(2), item(3), item(4)], 'AL', 'A');
      await primeAndPlay(h);

      // Off track 1 so the playing item is identifiable.
      h.handlers.eos();
      await flush();
      expect(player.model.get().current?.id).toBe('t2');

      // Pause, then remove the playing item through the queue store (the
      // panel's ✕ button): the player reloads the successor under pause intent.
      await player.pause();
      expect(player.model.get().state).toBe('paused');
      const removedId = queue.current()!.id; // t2
      queue.removeAt(queue.get().index);
      await flush();
      expect(player.model.get().state).toBe('paused');
      const successorId = player.model.get().current?.id;
      expect(successorId).not.toBe(removedId); // t3 loaded, cursor moved off t2

      // Stale decode-eos from the discarded stream fires now. The successor
      // (t3) still has a following track (t4), so without the guard the
      // handoff would advance to it and clobber the paused state.
      h.handlers.eos();
      await flush();

      // The paused session must stand — no auto-advance, no `loading`.
      expect(player.model.get().state).toBe('paused');
      expect(player.model.get().current?.id).toBe(successorId);
    });
  });
});