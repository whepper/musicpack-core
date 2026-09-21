// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import type { Engine, EngineEventName } from '../../player-core/src/engine';
import { MusepackEngine } from '../../app/src/lib/playback/musepack-engine';
import { NativeBackend } from '../../app/src/lib/playback/native-backend';

/** M4 contract: both concrete web engines ARE Engine implementations, with
 *  honest capability flags; the on() API fans out alongside the legacy
 *  wiring. */

function musepackHandlers() {
  return {
    primed: vi.fn(),
    buffering: vi.fn(),
    eos: vi.fn(),
    error: vi.fn(),
    tick: vi.fn(),
  };
}

describe('engine port conformance', () => {
  it('musepack engine satisfies Engine and reports exact-gapless capabilities', () => {
    const e: Engine = new MusepackEngine(musepackHandlers());
    expect(e.capabilities).toEqual({
      preloadNext: true,
      sampleAccurateGapless: true,
      decodeGate: true,
      crossfade: true, // Phase B: worklet overlap-add mixing
    });
  });

  it('native backend satisfies Engine and declares NO sample-accurate gapless', () => {
    const e: Engine = new NativeBackend();
    expect(e.capabilities).toEqual({
      preloadNext: true,
      sampleAccurateGapless: false, // honesty rule: element swap is approximate
      decodeGate: false,
      crossfade: true, // Phase A: element-based overlap implemented
    });
  });

  it('on() listeners fire for engine events without disturbing legacy wiring', () => {
    const h = musepackHandlers();
    const legacy = new MusepackEngine(h);
    const primed = vi.fn();
    const eos = vi.fn();
    legacy.on('primed' as EngineEventName, primed);
    const unsubEos = legacy.on('eos' as EngineEventName, eos);

    // Drive the internal emit paths through the worker-message handler.
    const harness = legacy as unknown as {
      current: { eos: boolean } | null;
      onWorkerMessage(h2: unknown, m: unknown): void;
    };
    harness.current = { eos: false };
    harness.onWorkerMessage(harness.current, { type: 'eos', generation: 0 });

    expect(h.primed).not.toHaveBeenCalled(); // no primed message was delivered
    expect(h.eos).toHaveBeenCalledOnce(); // legacy path intact
    expect(eos).toHaveBeenCalledOnce(); // port listener fired too
    expect(primed).not.toHaveBeenCalled();

    unsubEos();
    harness.current.eos = false; // reset the engine's eos latch
    harness.onWorkerMessage(harness.current, { type: 'eos', generation: 0 });
    expect(h.eos).toHaveBeenCalledTimes(2); // legacy keeps firing
    expect(eos).toHaveBeenCalledOnce(); // port listener did NOT re-fire
  });
});
