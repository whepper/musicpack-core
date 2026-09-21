// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import { MusepackEngine } from '../../app/src/lib/playback/musepack-engine';
import type { EngineStreamInfo } from '../../app/src/lib/playback/musepack-engine';
import type { CrossfadeResult } from '../../player-core/src/engine';
import type { WorkletReport } from '../../app/src/lib/playback/worklet-protocol';
import type { PlaybackItem } from '../../player-core/src/types';

function item(url: string): PlaybackItem {
  return { id: `t${url}`, trackId: 1, source: { kind: 'http-range', url }, title: url, artist: 'a', albumTitle: 'al' };
}

interface EngineHarness {
  generation: number;
  transitioning: boolean;
  current: {
    worker: { postMessage: ReturnType<typeof vi.fn> };
    info: EngineStreamInfo | null;
    sourceInfo: EngineStreamInfo | null;
    eos: boolean;
    nextUrl: null;
    item?: import('../../player-core/src/types').PlaybackItem | null;
    cancelOpen?: (() => void) | null;
  };
  standby: EngineHarness['current'] | null;
  ctx: AudioContext | null;
  node: { port: { postMessage: ReturnType<typeof vi.fn> } } | null;
  makeWorker(): EngineHarness['current'];
  onWorkletMessage(message: WorkletReport): void;
  onWorkerMessage(handle: EngineHarness['current'], message: Record<string, unknown>): void;
}

class AsyncWorker {
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  messages: Record<string, unknown>[] = [];
  terminated = false;

  postMessage(message: Record<string, unknown>): void {
    this.messages.push(message);
    if (message.type === 'close') {
      queueMicrotask(() => this.onmessage?.({ data: { type: 'closed' } } as MessageEvent));
    } else if (message.type === 'seek') {
      queueMicrotask(() =>
        this.onmessage?.({
          data: {
            type: 'seeked',
            sample: message.sample,
            generation: message.generation,
          },
        } as MessageEvent),
      );
    }
  }

  terminate(): void {
    this.terminated = true;
  }

  emitInfo(generation: number, rate = 44100, channels = 2, lengthSamples = rate * 10): void {
    this.onmessage?.({
      data: { type: 'info', rate, channels, version: 8, lengthSamples, generation },
    } as MessageEvent);
  }
}

describe('MusepackEngine backpressure reports', () => {
  it('keeps the newest worker when overlapping opens complete out of order', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const workers: AsyncWorker[] = [];
    harness.ctx = {} as AudioContext;
    harness.node = { port: { postMessage: vi.fn() } };
    harness.makeWorker = () => {
      const worker = new AsyncWorker();
      workers.push(worker);
      return {
        worker: worker as unknown as { postMessage: ReturnType<typeof vi.fn> },
        info: null,
        sourceInfo: null,
        eos: false,
        nextUrl: null,
        cancelOpen: null,
      };
    };

    const first = engine.open(item('/first.mpc')).then(
      () => 'resolved',
      () => 'rejected',
    );
    await vi.waitFor(() => expect(workers).toHaveLength(1));
    const second = engine.open(item('/second.mpc'));
    await vi.waitFor(() => expect(workers).toHaveLength(2));
    workers[1]?.emitInfo(2);

    await expect(second).resolves.toMatchObject({ rate: 44100, lengthSamples: 441000 });
    await expect(first).resolves.toBe('rejected');
    expect(workers[0]?.terminated).toBe(true);
    expect((harness.current.worker as unknown as AsyncWorker)).toBe(workers[1]);

    workers[0]?.emitInfo(1);
    expect((harness.current.worker as unknown as AsyncWorker)).toBe(workers[1]);
  });

  it('pauses at high water, resumes below low water, and reports underruns', () => {
    const onBuffering = vi.fn();
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: onBuffering,
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const postMessage = vi.fn();
    harness.current = {
      worker: { postMessage },
      info: null,
      sourceInfo: null,
      eos: false,
      nextUrl: null,
    };

    engine.startPumping();
    postMessage.mockClear();
    harness.onWorkletMessage({ type: 'full', frames: 0, available: 64, generation: 0 });
    engine.startPumping(); // transport resume must not bypass the full latch
    harness.onWorkletMessage({ type: 'accepted', frames: 0, available: 64, generation: 0 });
    harness.onWorkletMessage({ type: 'need', frames: 49, available: 15, generation: 0 });
    harness.onWorkletMessage({ type: 'underrun', frames: 64, available: 0, generation: 0 });

    expect(postMessage.mock.calls.map(([message]) => message)).toEqual([
      { type: 'pause', generation: 0 },
      { type: 'play', generation: 0 },
      // M8 repair: an underrun also proves any outstanding decode request
      // is gone, so the credit is released and a fresh play is posted
      // instead of silently blocking the pump forever.
      { type: 'play', generation: 0 },
    ]);
    expect(onBuffering).toHaveBeenCalledOnce();
  });

  it('ignores stale worklet reports and stale worker PCM', () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const workerPost = vi.fn();
    const workletPost = vi.fn();
    harness.generation = 2;
    harness.current = {
      worker: { postMessage: workerPost },
      info: null,
      sourceInfo: null,
      eos: false,
      nextUrl: null,
    };
    harness.node = { port: { postMessage: workletPost } };

    harness.onWorkletMessage({ type: 'need', frames: 0, generation: 1 });
    harness.onWorkletMessage({ type: 'full', frames: 0, generation: 1 });
    harness.onWorkerMessage(harness.current, {
      type: 'pcm',
      generation: 1,
      samples: Float32Array.of(1),
    });
    expect(workerPost).not.toHaveBeenCalled();
    expect(workletPost).not.toHaveBeenCalled();

    const current = Float32Array.of(2);
    harness.onWorkerMessage(harness.current, { type: 'pcm', generation: 2, samples: current });
    expect(workletPost).toHaveBeenCalledWith(
      { type: 'samples', buffer: current.buffer, generation: 2 },
      [current.buffer],
    );
  });

  it('does not pull PCM from reset reports while a generation transition is pending', () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const postMessage = vi.fn();
    harness.generation = 3;
    harness.transitioning = true;
    harness.current = {
      worker: { postMessage },
      info: null,
      sourceInfo: null,
      eos: false,
      nextUrl: null,
    };
    engine.startPumping();

    harness.onWorkletMessage({ type: 'underrun', frames: 0, generation: 3, available: 0 });
    harness.onWorkletMessage({ type: 'need', frames: 0, generation: 3, available: 0 });
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('normalizes stream timing to the AudioContext rate and converts seeks to source frames', async () => {
    const onPosition = vi.fn();
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: onPosition,
    });
    const harness = engine as unknown as EngineHarness;
    const workers: AsyncWorker[] = [];
    harness.ctx = { sampleRate: 48000 } as AudioContext;
    harness.node = { port: { postMessage: vi.fn() } };
    harness.makeWorker = () => {
      const worker = new AsyncWorker();
      workers.push(worker);
      return {
        worker: worker as unknown as { postMessage: ReturnType<typeof vi.fn> },
        info: null,
        sourceInfo: null,
        eos: false,
        nextUrl: null,
        cancelOpen: null,
      };
    };

    const opening = engine.open(item('/441.mpc'));
    await vi.waitFor(() => expect(workers).toHaveLength(1));
    workers[0]?.emitInfo(1, 44100, 2, 441000);
    await expect(opening).resolves.toMatchObject({ rate: 48000, channels: 2, lengthSamples: 480000 });
    if (workers[0]) {
      workers[0].onmessage = (event) => harness.onWorkerMessage(harness.current, event.data);
    }

    const seeking = engine.seek(48000);
    const seekMessage = workers[0]?.messages.find((message) => message.type === 'seek');
    expect(seekMessage).toMatchObject({ sample: 44100, generation: 2 });
    await seeking;

    harness.onWorkletMessage({ type: 'rendered', frames: 4800, generation: 2 });
    expect(engine.getPositionSamples()).toBe(52800);
    expect(engine.getRenderedSamples()).toBe(4800);
    expect(onPosition).toHaveBeenLastCalledWith(52800);
  });

  it('does not resume decoder pumping after a paused seek', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const postMessage = vi.fn();
    harness.generation = 4;
    harness.current = {
      worker: { postMessage },
      info: null,
      sourceInfo: { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 },
      eos: false,
      nextUrl: null,
    };
    harness.node = { port: { postMessage: vi.fn() } };
    engine.pausePumping();
    postMessage.mockClear();

    const seeking = engine.seek(48000);
    harness.onWorkerMessage(harness.current, { type: 'seeked', sample: 44100, generation: 5 });
    await seeking;
    expect(postMessage.mock.calls.map(([message]) => message.type)).toEqual(['seek']);
  });

  it('does not restore pumping when pause arrives during an in-flight seek', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const postMessage = vi.fn();
    harness.generation = 8;
    harness.current = {
      worker: { postMessage },
      info: null,
      sourceInfo: { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 },
      eos: false,
      nextUrl: null,
    };
    harness.node = { port: { postMessage: vi.fn() } };
    engine.startPumping();
    postMessage.mockClear();

    const seeking = engine.seek(44100);
    engine.pausePumping();
    harness.onWorkerMessage(harness.current, { type: 'seeked', sample: 44100, generation: 9 });
    await seeking;
    expect(postMessage.mock.calls.map(([message]) => message.type)).toEqual(['seek', 'pause']);
  });

  it('re-suspends the AudioContext when pause wins an in-flight resume race', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    let finishResume = () => {};
    const ctx = {
      state: 'suspended',
      resume: vi.fn(() => new Promise<void>((resolve) => {
        finishResume = () => {
          ctx.state = 'running';
          resolve();
        };
      })),
      suspend: vi.fn(async () => {
        ctx.state = 'suspended';
      }),
    };
    harness.ctx = ctx as unknown as AudioContext;

    const playing = engine.play();
    const pausing = engine.pause();
    finishResume();
    await Promise.all([playing, pausing]);
    expect(ctx.state).toBe('suspended');
    expect(ctx.suspend).toHaveBeenCalled();
  });

  it('finishes the prior track before switching a gapless stream to a new source format', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const portPost = vi.fn();
    const currentWorker = new AsyncWorker();
    const standbyWorker = new AsyncWorker();
    harness.generation = 7;
    harness.node = { port: { postMessage: portPost } };
    harness.current = {
      worker: currentWorker as unknown as { postMessage: ReturnType<typeof vi.fn> },
      info: { rate: 48000, channels: 2, version: 8, lengthSamples: 480000 },
      sourceInfo: { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 },
      eos: true,
      nextUrl: null,
    };
    harness.standby = {
      worker: standbyWorker as unknown as { postMessage: ReturnType<typeof vi.fn> },
      info: { rate: 48000, channels: 2, version: 8, lengthSamples: 240000 },
      sourceInfo: { rate: 48000, channels: 1, version: 8, lengthSamples: 240000 },
      eos: false,
      nextUrl: null,
      item: { ...item('/next') },
    };

    // The core may only promote a standby matching its current policy
    // target — here the prepared item IS the expected one.
    const advancing = engine.advance(item('/next'));
    expect(portPost).toHaveBeenCalledWith({ type: 'end', generation: 7 });
    expect(harness.current.worker).toBe(
      currentWorker as unknown as { postMessage: ReturnType<typeof vi.fn> },
    );

    harness.onWorkletMessage({ type: 'trackEnded', frames: 100, generation: 7 });
    await expect(advancing).resolves.toMatchObject({ rate: 48000, lengthSamples: 240000 });
    expect(portPost).toHaveBeenLastCalledWith({
      type: 'track',
      sourceRate: 48000,
      sourceChannels: 1,
      generation: 7,
    });
    expect(portPost.mock.calls.some(([message]) => message.type === 'reset')).toBe(false);
  });

  it(
    'promotes a matching standby, refuses mismatch/null expectations without flushing',
    { timeout: 15000 },
    async () => {
      const engine = new MusepackEngine({
        primed: vi.fn(),
        buffering: vi.fn(),
        eos: vi.fn(),
        error: vi.fn(),
        tick: vi.fn(),
      });
      const harness = engine as unknown as EngineHarness;
      const portPost = vi.fn();
      harness.generation = 7;
      harness.node = { port: { postMessage: portPost } };
      harness.current = {
        worker: new AsyncWorker() as unknown as EngineHarness['current']['worker'],
        info: null,
        sourceInfo: null,
        eos: false,
        nextUrl: null,
        item: item('/current.mpc'),
        cancelOpen: null,
      };
      harness.standby = {
        worker: new AsyncWorker() as unknown as EngineHarness['current']['worker'],
        info: { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 },
        sourceInfo: { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 },
        eos: false,
        nextUrl: null,
        item: item('/next.mpc'),
        cancelOpen: null,
      };

      // Mismatched expectation: refused BEFORE any flush ('end' never sent),
      // the standby stays in its slot untouched.
      await expect(engine.advance(item('/other.mpc'))).resolves.toBeNull();
      expect(portPost.mock.calls.some(([m]) => (m as Record<string, unknown>).type === 'end')).toBe(false);
      expect(harness.standby?.item?.id).toBe(item('/next.mpc').id);

      // Null expectation (nothing may follow): also refused, no promotion.
      await expect(engine.advance(null)).resolves.toBeNull();

      // Matching expectation: the normal gapless promotion runs. The
      // 'trackEnded' handshake resolves the flush the same way the worklet
      // does in production.
      const advancing = engine.advance(item('/next.mpc'));
      harness.onWorkletMessage({ type: 'trackEnded', frames: 100, generation: 7 });
      await expect(advancing).resolves.toMatchObject({
        lengthSamples: 441000,
      });
    },
  );

  it('keeps only the newest standby when prepareNext calls overlap', async () => {
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: vi.fn(),
      error: vi.fn(),
      tick: vi.fn(),
    });
    const harness = engine as unknown as EngineHarness;
    const workers: AsyncWorker[] = [];
    harness.ctx = {} as AudioContext;
    harness.node = { port: { postMessage: vi.fn() } };
    harness.makeWorker = () => {
      const worker = new AsyncWorker();
      workers.push(worker);
      return {
        worker: worker as unknown as { postMessage: ReturnType<typeof vi.fn> },
        info: null,
        sourceInfo: null,
        eos: false,
        nextUrl: null,
        cancelOpen: null,
      };
    };

    const first = engine.prepareNext(item('/a.mpc'));
    await vi.waitFor(() => expect(workers).toHaveLength(1));
    const second = engine.prepareNext(item('/b.mpc')); // supersedes the first
    await vi.waitFor(() => expect(workers).toHaveLength(2));

    workers[1]?.emitInfo(0);
    await expect(second).resolves.toMatchObject({ rate: 44100, lengthSamples: 441000 });
    await expect(first).resolves.toBeNull(); // superseded standby never resolves with info

    // the loser's worker is closed and terminated; only the newest survives
    expect(workers[0]?.terminated).toBe(true);
    expect((harness.standby!.worker as unknown as AsyncWorker)).toBe(workers[1]);

    // a late info from the discarded standby must not promote it
    workers[0]?.emitInfo(0);
    expect((harness.standby!.worker as unknown as AsyncWorker)).toBe(workers[1]);
  });
});


describe('MusepackEngine crossfade (M8 Phase B)', () => {
  interface XHarness {
    generation: number;
    current: EngineHarness['current'] & { syntheticEosPending?: boolean; syntheticEosAt?: number };
    standby: (EngineHarness['current'] & { syntheticEosPending?: boolean; syntheticEosAt?: number }) | null;
    node: { port: { postMessage: (m: Record<string, unknown>) => void } } | null;
    onWorkletMessage(message: WorkletReport): void;
    onWorkerMessage(handle: EngineHarness['current'], message: Record<string, unknown>): void;
    /** The live crossfade attempt (null when none is running). */
    xfade: {
      token: number;
      phase: 'priming' | 'mixing';
      fadeFrames: number;
      needed: number;
      filled: number;
      q: Float32Array[];
      awaitingAccepted: boolean;
      decodeDone: boolean;
      endSent: boolean;
      readyResolve: ((ok: boolean) => void) | null;
      swapResolve: ((ok: boolean) => void) | null;
      swapFacts: { outgoingFrames: number; incomingFrames: number } | null;
    } | null;
  }

  /** Worker stub that answers the close handshake so closeWorker resolves
   *  promptly (the real worker acks `closed` after teardown). */
  class ClosingWorker {
    onmessage: ((event: MessageEvent) => void) | null = null;
    postMessage = vi.fn((message: Record<string, unknown>) => {
      if (message.type === 'close') {
        queueMicrotask(() => {
          this.onmessage?.({ data: { type: 'closed' } } as MessageEvent);
        });
      }
    });
    terminate = vi.fn();
  }

  function makeEngine(opts: { withStandby?: boolean } = {}): {
    engine: MusepackEngine;
    h: XHarness;
    eosSpy: ReturnType<typeof vi.fn>;
    workletMessages: Record<string, unknown>[];
  } {
    const eosSpy = vi.fn();
    const engine = new MusepackEngine({
      primed: vi.fn(),
      buffering: vi.fn(),
      eos: eosSpy,
      error: vi.fn(),
      tick: vi.fn(),
    });
    const h = engine as unknown as XHarness;
    const workletMessages: Record<string, unknown>[] = [];
    const info = { rate: 44100, channels: 2, version: 8, lengthSamples: 441000 };
    h.current = {
      worker: new ClosingWorker() as unknown as EngineHarness['current']['worker'],
      info,
      sourceInfo: info,
      eos: false,
      nextUrl: null,
      cancelOpen: null,
    };
    h.standby =
      opts.withStandby === false
        ? null
        : {
            worker: new ClosingWorker() as unknown as EngineHarness['current']['worker'],
            info,
            sourceInfo: info,
            eos: false,
            nextUrl: null,
            // The standby carries the item it was prepared for; tests drive
            // beginCrossfade(item('/next.mpc')), so this must match.
            item: item('/next.mpc'),
            cancelOpen: null,
          };
    h.node = {
      port: { postMessage: (m: Record<string, unknown>) => void workletMessages.push(m) },
    };
    return { engine, h, eosSpy, workletMessages };
  }

  /** Poll-until helper without vi.waitFor's timer interactions. */
  async function until(cond: () => boolean): Promise<void> {
    for (let i = 0; i < 200 && !cond(); i++) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(cond()).toBe(true);
  }

  /** Drives one beginCrossfade attempt to the swap point. Returns the
   *  still-running attempt WITHOUT awaiting it (an async fn returning the
   *  promise directly would deadlock against the caller's next step). */
  async function runToSwap(
    engine: MusepackEngine,
    h: XHarness,
  ): Promise<{ promise: Promise<CrossfadeResult | null> }> {
    const promise = engine.beginCrossfade(item('/next.mpc'), 8);
    await until(() => typeof h.xfade?.readyResolve === 'function');
    h.onWorkletMessage({ type: 'xfadeReady', frames: 0, available: 999, token: 1, generation: 0 });
    await until(() => typeof h.xfade?.swapResolve === 'function');
    return { promise };
  }

  it(
    'returns null immediately when there is no standby',
    { timeout: 15000 },
    async () => {
      const { engine } = makeEngine({ withStandby: false });
      await expect(engine.beginCrossfade(item('/next.mpc'), 8)).resolves.toBeNull();
    },
  );

  it(
    'arms the lane, pumps the standby into it, and swaps on xfaded',
    { timeout: 15000 },
    async () => {
      const { engine, h, workletMessages, eosSpy } = makeEngine();
      const incoming = h.standby!;
      const { promise: pending } = await runToSwap(engine, h);

      // The lane was armed and the standby worker told to play.
      expect(workletMessages[0]).toMatchObject({ type: 'xfade', token: 1 });
      expect(
        (incoming.worker.postMessage as ReturnType<typeof vi.fn>).mock.calls.some(
          ([m]) => (m as Record<string, unknown>).type === 'play',
        ),
      ).toBe(true);

      // Standby PCM routes to xsamples; the lane decode-eos stays silent.
      h.onWorkerMessage(incoming, { type: 'pcm', samples: Float32Array.of(0.5), generation: 0 });
      expect(workletMessages.some((m) => m.type === 'xsamples')).toBe(true);
      h.onWorkerMessage(incoming, { type: 'eos', generation: 0 });
      expect(eosSpy).not.toHaveBeenCalled();

      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 352800,
        incomingFrames: 352796,
        token: 1,
        generation: 0,
      });

      await expect(pending).resolves.toMatchObject({
        info: { rate: 44100 },
        overlapFrames: 352796,
      });
      // Promotion happened exactly like advance(): the standby handle is
      // now current and the standby slot is empty.
      expect(h.current).toBe(incoming);
      expect(h.standby).toBeNull();
    },
  );

  it(
    'suppresses a stale outgoing-lane eos during the fade',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy } = makeEngine();
      const { promise: pending } = await runToSwap(engine, h);

      // The OUTGOING track's decode-eos lands mid-fade: swallowed...
      h.onWorkerMessage(h.current, { type: 'eos', generation: 0 });
      expect(eosSpy).not.toHaveBeenCalled();

      h.onWorkletMessage({ type: 'xfaded', frames: 3, outgoingFrames: 4, incomingFrames: 4, token: 1, generation: 0 });
      await expect(pending).resolves.toBeTruthy();
      expect(eosSpy).not.toHaveBeenCalled(); // still nothing: new track just started
    },
  );

  it(
    're-emits the core eos when the incoming track decoded fully inside the lane',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy } = makeEngine();
      const incoming = h.standby!;
      const pendingPromise = engine.beginCrossfade(item('/next.mpc'), 8);
      await until(() => typeof h.xfade?.readyResolve === 'function');

      // Incoming decode completes in the lane BEFORE promotion...
      h.onWorkerMessage(incoming, { type: 'eos', generation: 0 });
      h.onWorkletMessage({ type: 'xfadeReady', frames: 0, available: 9, token: 1, generation: 0 });
      await until(() => typeof h.xfade?.swapResolve === 'function');
      h.onWorkletMessage({ type: 'xfaded', frames: 3, outgoingFrames: 4, incomingFrames: 4, token: 1, generation: 0 });

      await expect(pendingPromise).resolves.toBeTruthy();
      expect(incoming.syntheticEosPending).toBe(true);
      expect(eosSpy).not.toHaveBeenCalled(); // not yet — only at its audible end

      // The promoted handle's spent worker eos arrives later: re-emitted.
      h.onWorkerMessage(incoming, { type: 'eos', generation: 0 });
      expect(eosSpy).toHaveBeenCalledOnce();
    },
  );

  it(
    'falls back to null when the lane never becomes ready',
    { timeout: 15000 },
    async () => {
      vi.useFakeTimers();
      try {
        const { engine, h, workletMessages } = makeEngine();
        const pending = engine.beginCrossfade(item('/next.mpc'), 8);
        const assertion = expect(pending).resolves.toBeNull();
        await vi.advanceTimersByTimeAsync(31000);
        await assertion;
        expect(
          workletMessages.some((m) => (m as Record<string, unknown>).type === 'xfade-cancel'),
        ).toBe(true);
      } finally {
        vi.useRealTimers();
      }
    },
  );

describe('MusepackEngine crossfade lifecycle regressions', () => {
  it(
    'recovers the decode pump when an underrun follows a lost play (short-track stall)',
    { timeout: 15000 },
    async () => {
      const { engine, h } = makeEngine();
      // A 'play' is outstanding (credit held) when the ring starves — the
      // response was consumed by promotion/generation churn. The underrun
      // proves the outstanding request is gone; decode MUST resume.
      (engine as any).pullInFlight = true;
      (engine as any).pumpingRequested = true;
      const worker = (engine as any).current.worker;
      const before = worker.postMessage.mock.calls.length;
      h.onWorkletMessage({ type: 'underrun', frames: 0, generation: 0 });
      // The credit was released and IMMEDIATELY re-acquired by the fresh
      // recovery play (resumePumpingIfRequested posts and holds it).
      expect((engine as any).pullInFlight).toBe(true);
      const plays = (
        worker.postMessage.mock.calls.slice(before) as Array<[Record<string, unknown>]>
      ).filter(([m]) => m.type === 'play');      expect(plays.length).toBe(1);
    },
  );

  it(
    'clears the demand-pump credit when a crossfade promotes over an in-flight play',
    { timeout: 15000 },
    async () => {
      const { engine, h } = makeEngine();
      (engine as any).pullInFlight = true;
      const incoming = h.standby!;
      const { promise } = await runToSwap(engine, h);
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 400000,
        incomingFrames: 100000,
        token: 1,
        generation: 0,
      });
      await expect(promise).resolves.toBeTruthy();
      // Promotion changed the current handle: the old lane's outstanding
      // credit is meaningless and must not block the new track's pump.
      expect((engine as any).pullInFlight).toBe(false);
    },
  );

  it(
    'primes the whole fade window inside the sized lane ring (step 6)',
    { timeout: 15000 },
    async () => {
      const { engine, h } = makeEngine();
      // Give the standby a long track so the fade window (not the track
      // length) is what bounds the readiness threshold.
      const longInfo = { rate: 44100, channels: 2, version: 8, lengthSamples: 24 * 44100 };
      h.standby!.info = longInfo;
      const pending = engine.beginCrossfade(item('/next.mpc'), 12);
      await until(() => h.xfade !== null && typeof h.xfade.readyResolve === 'function');
      // 12 s fade at the default 44.1 kHz output rate: the readiness
      // threshold is fadeFrames + margin, and it must fit inside the lane
      // capacity (which grows beyond the main ring for long fades).
      const xfade = h.xfade!;
      const fadeFrames = Math.round(12 * 44100);
      expect(xfade.fadeFrames).toBe(fadeFrames);
      expect(xfade.needed).toBe(fadeFrames + 2048);
      const { laneCapacityFrames } = await import(
        '../../app/src/lib/playback/worklet-protocol'
      );
      expect(xfade.needed).toBeLessThan(laneCapacityFrames(44100, fadeFrames));
      // Settle the attempt so no timers leak into the next test.
      h.onWorkletMessage({ type: 'xfadeReady', frames: 0, available: 9, token: xfade.token, generation: 0 });
      await until(() => typeof h.xfade?.swapResolve === 'function');
      h.onWorkletMessage({ type: 'xfaded', frames: 3, outgoingFrames: 4, incomingFrames: 4, token: xfade.token, generation: 0 });
      await expect(pending).resolves.toBeTruthy();
    },
  );

  it(
    'delivers the NEXT boundary eos after a completed fade (BUG-1: no stale suppression)',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy } = makeEngine();
      const incoming = h.standby!;
      const { promise } = await runToSwap(engine, h);
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 400000,
        incomingFrames: 100000,
        token: 1,
        generation: 0,
      });
      await expect(promise).resolves.toBeTruthy();
      expect(h.current).toBe(incoming);

      // The promoted track plays to its natural decode end much later:
      // its eos MUST reach the core (today it is swallowed by the stale
      // suppression flag left behind by the successful fade).
      h.onWorkerMessage(incoming, { type: 'eos', generation: 0 });
      expect(eosSpy).toHaveBeenCalledTimes(1);
    },
  );

  it(
    'cleans up when superseded by a standby replacement mid-fade (BUG-3a)',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy, workletMessages } = makeEngine();
      const { promise } = await runToSwap(engine, h);

      // prepareNext replaced the standby slot while the fade was awaiting
      // its swap: the attempt is dead and must resolve null, not promote.
      h.standby = null;
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 4,
        incomingFrames: 4,
        token: 1,
        generation: 0,
      });

      await expect(promise).resolves.toBeNull();
      expect(workletMessages.some((m) => (m as Record<string, unknown>).type === 'xfade-cancel')).toBe(true);
      // The still-current track keeps its normal lifecycle:
      h.onWorkerMessage(h.current!, { type: 'eos', generation: 0 });
      expect(eosSpy).toHaveBeenCalledTimes(1);
    },
  );

  it(
    'cleans up when the generation moved during the swap wait (BUG-3b)',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy, workletMessages } = makeEngine();
      const { promise } = await runToSwap(engine, h);

      // A seek/open bumped the generation; the stale xfaded resolves the
      // await but the attempt must cancel cleanly.
      (h as unknown as { generation: number }).generation = 1;
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 4,
        incomingFrames: 4,
        token: 1,
        generation: 1,
      });

      await expect(promise).resolves.toBeNull();
      expect(workletMessages.some((m) => (m as Record<string, unknown>).type === 'xfade-cancel')).toBe(true);
      // No stale suppression: the current track's own later eos fires.
      h.onWorkerMessage(h.current!, { type: 'eos', generation: 1 });
      expect(eosSpy).toHaveBeenCalledTimes(1);
    },
  );

  it(
    'refuses a second attempt while one is already armed/mixing (BUG-4)',
    { timeout: 20000 },
    async () => {
      const { engine, h, workletMessages } = makeEngine();
      const { promise } = await runToSwap(engine, h);

      // A second trigger while the first is awaiting its swap must be
      // refused immediately (one arm message only) instead of arming a
      // competing attempt whose abort would kill the live fade. RED note:
      // today the second attempt arms a second lane message and hangs in
      // its readiness wait; only the arm-count assertion is checked here
      // before finishing the first attempt.
      engine.beginCrossfade(item('/other.mpc'), 8);
      expect(workletMessages.filter((m) => (m as Record<string, unknown>).type === 'xfade')).toHaveLength(1);

      // The first attempt completes unaffected.
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 4,
        incomingFrames: 4,
        token: 1,
        generation: 0,
      });
    },
  );

  it(
    'fires the synthetic eos at the compressed audible end, not the raw length (accounting)',
    { timeout: 15000 },
    async () => {
      const { engine, h, eosSpy } = makeEngine();
      const incoming = h.standby!;
      const pendingPromise = engine.beginCrossfade(item('/next.mpc'), 8);
      await until(() => typeof h.xfade?.readyResolve === 'function');

      // Incoming decode completes inside the lane before promotion.
      h.onWorkerMessage(incoming, { type: 'eos', generation: 0 });
      h.onWorkletMessage({ type: 'xfadeReady', frames: 0, available: 9, token: 1, generation: 0 });
      await until(() => typeof h.xfade?.swapResolve === 'function');
      h.onWorkletMessage({
        type: 'xfaded',
        frames: 3,
        outgoingFrames: 400000,
        incomingFrames: 100000,
        token: 1,
        generation: 0,
      });
      await expect(pendingPromise).resolves.toBeTruthy();
      expect(incoming.syntheticEosPending).toBe(true);

      // lengthSamples = 441000; the audible end sits at
      // 400000 + (441000 - 100000) = 741000 continuous frames. At 500000
      // the old raw-length check fired far too early (B had barely begun).
      h.onWorkletMessage({ type: 'rendered', frames: 500000, generation: 0 });
      expect(eosSpy).not.toHaveBeenCalled();

      h.onWorkletMessage({ type: 'rendered', frames: 741001, generation: 0 });
      expect(eosSpy).toHaveBeenCalledTimes(1);
    },
  );
});
});
