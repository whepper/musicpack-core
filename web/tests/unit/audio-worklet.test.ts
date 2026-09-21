// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  RING_SECONDS,
  XLANE_MAX_SECONDS,
  laneCapacityFrames,
} from '../../app/src/lib/playback/worklet-protocol';
import type { WorkletReport } from '../../app/src/lib/playback/worklet-protocol';

class FakePort {
  onmessage: ((event: MessageEvent) => void) | null = null;
  messages: WorkletReport[] = [];

  postMessage(message: WorkletReport): void {
    this.messages.push(message);
  }

  send(data: unknown): void {
    this.onmessage?.({ data } as MessageEvent);
  }
}

class FakeAudioWorkletProcessor {
  readonly port = new FakePort() as unknown as MessagePort;
}

interface Processor {
  readonly port: MessagePort;
  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean;
}

let ProcessorCtor: new () => Processor;

beforeAll(async () => {
  vi.stubGlobal('AudioWorkletProcessor', FakeAudioWorkletProcessor);
  vi.stubGlobal('currentTime', 0);
  vi.stubGlobal('registerProcessor', (_name: string, ctor: new () => Processor) => {
    ProcessorCtor = ctor;
  });
  await import('../../app/src/lib/playback/audio-worklet');
});

beforeEach(() => {
  vi.stubGlobal('currentTime', 0);
});

function createProcessor(
  sourceRate = 10,
  sourceChannels = 1,
  generation = 1,
  outputRate = sourceRate,
  outputChannels = sourceChannels,
): { processor: Processor; port: FakePort } {
  const processor = new ProcessorCtor();
  const port = processor.port as unknown as FakePort;
  port.send({
    type: 'config',
    sourceRate,
    sourceChannels,
    outputRate,
    outputChannels,
    generation,
  });
  return { processor, port };
}

function samples(frames: number, start = 0): Float32Array {
  return Float32Array.from({ length: frames }, (_, i) => start + i);
}

function render(processor: Processor, frames: number): Float32Array {
  const output = new Float32Array(frames);
  processor.process([], [[output]]);
  return output;
}

describe('MusicPackPcmProcessor backpressure', () => {
  it('pauses at high water and resumes after reads cross low water', () => {
    const { processor, port } = createProcessor(); // 80-frame ring
    const pcm = samples(100, 1);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });

    expect(port.messages.filter((message) => message.type === 'full')).toHaveLength(1);
    expect(port.messages.some((message) => message.type === 'accepted')).toBe(false);
    render(processor, 40); // pending PCM refills 20 of the consumed frames
    expect(port.messages.filter((message) => message.type === 'accepted')).toHaveLength(1);
    expect(port.messages.some((message) => message.type === 'need')).toBe(false);
    render(processor, 45); // 15 frames remain: below the 16-frame low water
    expect(port.messages.filter((message) => message.type === 'need')).toHaveLength(1);
  });

  it('retains a partial ring write and renders every PCM frame in order', () => {
    const { processor, port } = createProcessor();
    const pcm = samples(100, 1);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });

    const rendered = [
      ...render(processor, 40),
      ...render(processor, 45),
      ...render(processor, 20).slice(0, 15),
    ];
    expect(rendered).toEqual(Array.from(pcm));
  });

  it('reports an underrun when insufficient PCM makes it render silence', () => {
    const { processor, port } = createProcessor();
    const pcm = samples(5, 1);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });

    const output = render(processor, 8);
    expect(Array.from(output)).toEqual([1, 2, 3, 4, 5, 0, 0, 0]);
    expect(port.messages.filter((message) => message.type === 'underrun')).toEqual([
      expect.objectContaining({ type: 'underrun', frames: 5, available: 5 }),
    ]);
  });

  it('primes a short stream when EOS arrives below the normal watermark', () => {
    const { port } = createProcessor(); // 80-frame ring, 16-frame prime threshold
    const pcm = samples(5, 1);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });
    expect(port.messages.some((message) => message.type === 'primed')).toBe(false);

    port.send({ type: 'end', generation: 1 });
    expect(port.messages.filter((message) => message.type === 'primed')).toHaveLength(1);
    expect(port.messages.filter((message) => message.type === 'trackEnded')).toHaveLength(1);
  });

  it('ignores stale PCM and stale resets from an earlier generation', () => {
    const { processor, port } = createProcessor();
    const current = samples(4, 10);
    port.send({ type: 'reset', generation: 2 });
    port.send({ type: 'samples', buffer: samples(4, 100).buffer, generation: 1 });
    port.send({ type: 'reset', generation: 1 });
    port.send({ type: 'samples', buffer: current.buffer, generation: 2 });

    expect(Array.from(render(processor, 4))).toEqual(Array.from(current));
    expect(port.messages.filter((message) => message.type === 'accepted')).toEqual([
      expect.objectContaining({ type: 'accepted', generation: 2 }),
    ]);
  });

  it('renders 44.1 kHz source at the duration and pitch implied by a 48 kHz context', () => {
    const { processor, port } = createProcessor(44100, 1, 1, 48000, 1);
    const pcm = samples(441);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });
    port.send({ type: 'end', generation: 1 });

    const output = render(processor, 480);
    expect(output).toHaveLength(480);
    expect(output[160]).toBeCloseTo(147, 5);
    expect(output[320]).toBeCloseTo(294, 5);
    expect(output[479]).toBeCloseTo(440, 5);
    expect(port.messages).toContainEqual(expect.objectContaining({ type: 'trackEnded' }));
  });

  it('renders 48 kHz source at the duration implied by a 44.1 kHz context', () => {
    const { processor, port } = createProcessor(48000, 1, 1, 44100, 1);
    const pcm = samples(480);
    port.send({ type: 'samples', buffer: pcm.buffer, generation: 1 });
    port.send({ type: 'end', generation: 1 });

    const output = render(processor, 441);
    expect(output[147]).toBeCloseTo(160, 5);
    expect(output[294]).toBeCloseTo(320, 5);
    expect(output[440]).toBeCloseTo((440 * 48000) / 44100, 5);
    expect(port.messages).toContainEqual(expect.objectContaining({ type: 'trackEnded' }));
  });

  it('keeps normalized output queued across tracks with different rates and channels', () => {
    const { processor, port } = createProcessor(10, 1, 1, 20, 2);
    const mono = Float32Array.of(1, 2, 3);
    port.send({ type: 'samples', buffer: mono.buffer, generation: 1 });
    port.send({ type: 'end', generation: 1 });
    port.send({ type: 'track', sourceRate: 20, sourceChannels: 2, generation: 1 });
    const stereo = Float32Array.of(10, 20, 11, 21);
    port.send({ type: 'samples', buffer: stereo.buffer, generation: 1 });
    port.send({ type: 'end', generation: 1 });

    const left = new Float32Array(8);
    const right = new Float32Array(8);
    processor.process([], [[left, right]]);
    expect(Array.from(left)).toEqual([1, 1.5, 2, 2.5, 3, 3, 10, 11]);
    expect(Array.from(right)).toEqual([1, 1.5, 2, 2.5, 3, 3, 20, 21]);
  });
});

describe('lane ring sizing (M8 repair, step 6)', () => {
  it('keeps the main-ring capacity for small fades', () => {
    // 4 s fade at 48 kHz: wanted (5 s + margin) < base → base wins.
    expect(laneCapacityFrames(48000, 4 * 48000)).toBe(Math.round(48000 * RING_SECONDS));
  });

  it('grows to hold a long fade plus margin and headroom', () => {
    const cap = laneCapacityFrames(48000, 12 * 48000);
    // The readiness threshold (fade + margin) must fit inside the ring with
    // room to spare — that is the whole point of step 6.
    const needed = 12 * 48000 + 2048;
    expect(cap).toBeGreaterThan(needed);
    expect(cap).toBeLessThanOrEqual(Math.round(48000 * XLANE_MAX_SECONDS));
  });

  it('clamps at XLANE_MAX_SECONDS even for the maximum clamped fade', () => {
    const cap = laneCapacityFrames(48000, 15 * 48000);
    expect(cap).toBe(Math.round(48000 * XLANE_MAX_SECONDS));
    // Even at the cap the fade window itself fits.
    expect(15 * 48000).toBeLessThan(cap);
  });
});

describe('MusicPackPcmProcessor crossfade lane (M8 Phase B)', () => {
  function armFade(
    fadeFrames = 4,
    sourceRate = 10,
    generation = 1,
    token = 7,
  ): { processor: Processor; port: FakePort; go: () => void } {
    const { processor, port } = createProcessor(sourceRate, 1, generation);
    port.send({ type: 'xfade', sourceRate, sourceChannels: 1, fadeFrames, token, generation });
    return {
      processor,
      port,
      go: (): void => port.send({ type: 'xfade-go', token, generation }),
    };
  }

  /** Sends lane PCM/end stamped with the given attempt token. */
  function sendX(
    port: FakePort,
    type: 'xsamples' | 'xend',
    data: Float32Array | null,
    token: number,
    generation = 1,
  ): void {
    if (type === 'xsamples' && data) {
      port.send({ type, buffer: data.buffer, token, generation });
    } else if (type === 'xend') {
      port.send({ type, token, generation });
    }
  }

  it('mixes an equal-power overlap and swaps rings at the end of the window', () => {
    // Outgoing: a constant 1.0 stream; incoming: a constant 0.5 stream.
    const { processor, port, go } = armFade(4);
    sendX(port, 'xsamples', Float32Array.of(0.5, 0.5, 0.5), 7);
    port.send({ type: 'samples', buffer: Float32Array.of(1, 1, 1, 1, 1, 1).buffer, generation: 1 });
    sendX(port, 'xend', null, 7);
    go();

    // The whole track fits in one callback: mix 4 frames, then swap. The
    // lane holds exactly 3 frames (the track's full output), so the 4th
    // fade frame has no incoming audio (silence at full sine gain — the
    // engine only starts a fade when the lane holds the whole fade window,
    // so this cannot happen in production; the test pins the defensive
    // behavior). The 0.5-frames were consumed by the mix, so after the swap
    // the new ring is empty: frame 4 renders silence.
    const out = render(processor, 5);
    expect(out[0]).toBeCloseTo(1, 5); // cos(0)=1, sin(0)=0
    expect(out[1]).toBeCloseTo(Math.cos((1 / 4) * Math.PI / 2) + 0.5 * Math.sin((1 / 4) * Math.PI / 2), 5);
    expect(out[2]).toBeCloseTo(Math.cos((2 / 4) * Math.PI / 2) + 0.5 * Math.sin((2 / 4) * Math.PI / 2), 5);
    expect(out[3]).toBeCloseTo(Math.cos((3 / 4) * Math.PI / 2), 5);
    expect(out[4]).toBe(0); // post-swap ring drained

    const xfaded = port.messages.find((m) => m.type === 'xfaded');
    expect(xfaded).toMatchObject({ outgoingFrames: 4, incomingFrames: 3, token: 7 });
    expect(xfaded?.available).toBe(0); // ring swapped to the drained lane
  });

  it('rebases the promoted ring to the swap BOUNDARY, not the raw outgoing count (BUG-2, redesigned)', () => {
    // This processor is created fresh and armed immediately, so mixing
    // begins at outgoing.renderedFrames === 0 — the boundary itself is 0
    // here (no pre-roll before the fade). Outgoing goes on to supply all 4
    // fade frames (no underrun on that side); the lane only had 3 xsamples
    // queued, so it runs dry on the 4th fade frame (a defensive edge case
    // per the next test's comment, not a production path).
    //
    // Earlier revision of this test asserted 4 here ("continue counting
    // from the outgoing ring's raw swap-time count"), which kept the raw
    // engine counter monotonic across the swap but is what let the exposed
    // album position silently run ahead of the declared (overlap-shrunk)
    // offsets by one fade window on EVERY crossfade — the root cause behind
    // multi-track skips right after a fade (see player.ts's
    // beginCrossfadeTransition, which shrinks the outgoing track's declared
    // length by the reported overlap). Rebasing to the boundary instead
    // means the promoted ring reports exactly what the declared offsets
    // model expects once the incoming track becomes current: here, that is
    // the boundary itself (0), plus however far the incoming lane's own
    // reads have progressed (3) — NOT the outgoing side's unrelated raw
    // count. The one-time backward step this produces is the
    // necessary, self-correcting price of that consistency: two tracks
    // briefly share the timeline during the overlap, so no single
    // continuous low-level counter can describe both without a snap at the
    // instant one of them stops being "current".
    const { processor, port, go } = armFade(4);
    sendX(port, 'xsamples', Float32Array.of(0.5, 0.5, 0.5), 7);
    port.send({ type: 'samples', buffer: Float32Array.of(1, 1, 1, 1, 1, 1).buffer, generation: 1 });
    sendX(port, 'xend', null, 7);
    go();
    // 4 frames mix; the swap itself runs at the top of the NEXT callback's
    // mixing loop, so drive one more render tick past the window.
    render(processor, 5);

    const ring = (
      processor as unknown as { ring: { renderedFrames: number; availableFrames: number; capacity: number } }
    ).ring;
    expect(ring.renderedFrames).toBe(3);
  });

  it('reports the TRUE overlap (bounded by what the outgoing side actually supplied), not the lane count', () => {
    // Same defensive scenario as above (lane underruns on the 4th fade
    // frame), but this pins the 'xfaded' message's new overlapFrames field
    // directly: the outgoing side truly supplied all 4 fade frames, so
    // overlapFrames must be 4, even though the lane's OWN count only
    // reached 3. Before this fix, callers read `incomingFrames` (3) as the
    // overlap — silently wrong whenever the two lanes' counts diverge, of
    // which an underrunning lane is the most severe case.
    const { processor, port, go } = armFade(4);
    sendX(port, 'xsamples', Float32Array.of(0.5, 0.5, 0.5), 7);
    port.send({ type: 'samples', buffer: Float32Array.of(1, 1, 1, 1, 1, 1).buffer, generation: 1 });
    sendX(port, 'xend', null, 7);
    go();
    render(processor, 5);

    const xfaded = port.messages.find((m) => m.type === 'xfaded');
    expect(xfaded).toMatchObject({
      outgoingFrames: 4,
      incomingFrames: 3,
      swapBaseFrames: 0,
      overlapFrames: 4,
    });
  });

  it('recovers correctly when the outgoing ring underruns mid-fade (fix b: short remaining audio at trigger time)', () => {
    // The scenario this session's biggest coverage gap was missing: the
    // engine only arms a fade once trackRemainingSamples() looks like it
    // covers the whole fade window, but that estimate can be stale (late
    // length correction, a seek just before the trigger, resampler
    // rounding) — so the outgoing ring can still run dry PARTWAY through
    // an active mix. Six frames of real outgoing content already played
    // BEFORE the fade arms (the boundary is 6, not 0, unlike the tests
    // above), then only 2 more frames of true content exist when mixing
    // starts — less than the 4-frame fade window requested. The incoming
    // lane is fully supplied (4/4), isolating the outgoing side's underrun.
    const { processor, port } = createProcessor(10, 1, 1);
    port.send({
      type: 'samples',
      buffer: Float32Array.of(9, 9, 9, 9, 9, 9, 9, 9).buffer,
      generation: 1,
    });
    render(processor, 6); // consumes 6 of the 8 queued frames — boundary = 6

    port.send({ type: 'xfade', sourceRate: 10, sourceChannels: 1, fadeFrames: 4, token: 7, generation: 1 });
    sendX(port, 'xsamples', Float32Array.of(0.5, 0.5, 0.5, 0.5), 7); // lane: full 4, no lane underrun
    sendX(port, 'xend', null, 7);
    port.send({ type: 'xfade-go', token: 7, generation: 1 });
    render(processor, 5); // mixes 4 frames (2 real + 2 silent on the outgoing side), then swaps

    const xfaded = port.messages.find((m) => m.type === 'xfaded');
    // The TRUE overlap is bounded by what the outgoing ring actually had
    // left (2), never the nominal fade window (4) it was asked for.
    expect(xfaded).toMatchObject({
      outgoingFrames: 8, // boundary (6) + the 2 real frames it had left
      incomingFrames: 4,
      swapBaseFrames: 6,
      overlapFrames: 2,
    });
    const ring = (
      processor as unknown as { ring: { renderedFrames: number } }
    ).ring;
    // Because the lane's own natural count (4) stayed within the boundary
    // (6), the rebase fully corrects for the underrun: the album clock
    // lands EXACTLY on the boundary post-swap, with no drift carried
    // forward — despite the outgoing side having delivered only half of
    // the requested overlap.
    expect(ring.renderedFrames).toBe(6);
  });

  it('reports xfadeReady only when the whole incoming track is queued', () => {
    const { port } = armFade(2);
    expect(port.messages.some((m) => m.type === 'xfadeReady')).toBe(false);

    sendX(port, 'xsamples', Float32Array.of(9, 9), 7);
    expect(port.messages.some((m) => m.type === 'xfadeReady')).toBe(false);

    sendX(port, 'xend', null, 7);
    const ready = port.messages.find((m) => m.type === 'xfadeReady');
    expect(ready).toMatchObject({ available: 2, token: 7 });
  });

  it('keeps rendering the outgoing track while the armed lane fills up', () => {
    const { processor, port } = armFade(4);
    port.send({ type: 'samples', buffer: Float32Array.of(3, 3, 3).buffer, generation: 1 });
    sendX(port, 'xsamples', Float32Array.of(7, 7, 7), 7);

    const out = render(processor, 3);
    expect(Array.from(out)).toEqual([3, 3, 3]); // no go yet: no mixing
    expect(port.messages.some((m) => m.type === 'xfaded')).toBe(false);
  });

  it('ignores stale xsamples from a cancelled attempt and honours a re-armed lane', () => {
    const { processor, port } = armFade(2, 10, 1, 7);
    sendX(port, 'xsamples', Float32Array.of(1), 7);
    sendX(port, 'xend', null, 7);
    port.send({ type: 'xfade-cancel', generation: 1 }); // token bumped, lane dropped

    // Stale-token stragglers from the cancelled attempt are dropped.
    sendX(port, 'xsamples', Float32Array.of(5), 7);
    expect(port.messages.filter((m) => m.type === 'error')).toHaveLength(0);
    expect((processor as unknown as { ring: { availableFrames: number } }).ring.availableFrames).toBe(0);

    port.send({ type: 'xfade', sourceRate: 10, sourceChannels: 1, fadeFrames: 2, token: 8, generation: 1 });
    sendX(port, 'xsamples', Float32Array.of(5), 8);
    sendX(port, 'xend', null, 8);
    expect(
      port.messages.some((m) => m.type === 'xfadeReady' && m.token === 8),
    ).toBe(true);
  });

  it('absorbs late xsamples/xend into the normal path after the swap', () => {
    const { processor, port, go } = armFade(2);
    port.send({ type: 'samples', buffer: Float32Array.of(1, 1).buffer, generation: 1 });
    sendX(port, 'xsamples', Float32Array.of(4), 7);
    sendX(port, 'xend', null, 7);
    go();

    const first = render(processor, 3); // mixes 2 frames + swaps mid-callback
    // The lane's single 4-frame is consumed at fade-frame 0 where sin(0)=0,
    // so it is inaudible; frame 1 has only the fading-outgoing contribution.
    expect(first[0]).toBeCloseTo(1, 5);
    expect(first[1]).toBeCloseTo(Math.cos(Math.PI / 4), 5);
    expect(first[2]).toBe(0); // post-swap: rings drained

    // A straggler lane chunk (same token as the swap) flows through the
    // normal credit path; a WRONG-token chunk would be dropped.
    sendX(port, 'xsamples', Float32Array.of(6, 6), 7);
    const second = render(processor, 2);
    expect(Array.from(second)).toEqual([6, 6]);

    sendX(port, 'xsamples', Float32Array.of(9, 9), 99); // stale token
    const third = render(processor, 2);
    expect(Array.from(third)).toEqual([0, 0]);
  });

  it('defers the producer credit when a chunk only partially flushes', () => {
    const { processor, port } = armFade(2);
    // Step 6: the lane ring is sized by laneCapacityFrames (at the harness'
    // tiny output rate that is XLANE-clamped, not the old fixed 8 s base).
    const laneRing = (
      processor as unknown as { xlane: { ring: { capacity: number }; creditOwed: boolean; pending: unknown } | null }
    ).xlane!;
    expect(laneRing.ring.capacity).toBe(laneCapacityFrames(10, 2));
    // Send more lane audio than fits the sized ring: the chunk must NOT be
    // credited immediately (the old behavior double-credited and corrupted
    // the producer's accounting).
    sendX(port, 'xsamples', Float32Array.from({ length: laneRing.ring.capacity + 40 }, () => 1), 7);
    expect(
      port.messages.filter((m) => m.type === 'accepted' && m.lane === 2),
    ).toHaveLength(0);
    const lane = (
      processor as unknown as { xlane: { creditOwed: boolean; pending: unknown } | null }
    ).xlane;
    expect(lane?.creditOwed).toBe(true);
    expect(lane?.pending).not.toBeNull();
  });

  it('drops crossfade state on reset and config', () => {
    const { processor, port } = armFade(2);
    port.send({ type: 'reset', generation: 1 });
    sendX(port, 'xsamples', Float32Array.of(1), 7);
    expect(port.messages.filter((m) => m.type === 'accepted')).toHaveLength(0);

    port.send({ type: 'xfade', sourceRate: 10, sourceChannels: 1, fadeFrames: 2, token: 9, generation: 1 });
    port.send({ type: 'reset', generation: 2 });
    sendX(port, 'xsamples', Float32Array.of(1), 9);
    expect(port.messages.filter((m) => m.type === 'accepted')).toHaveLength(0);
    void processor;
  });
});
