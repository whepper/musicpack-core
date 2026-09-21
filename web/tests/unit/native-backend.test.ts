// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import { NativeBackend } from '../../app/src/lib/playback/native-backend';
import type { PlaybackItem } from '../../player-core/src/types';

function nativeItem(id: string): PlaybackItem {
  return {
    id,
    trackId: Number(id.replace(/\D/g, '')) || 1,
    source: { kind: 'stream', url: `/${id}` },
    title: id,
    artist: 'a',
    albumTitle: 'al',
  };
}

interface FakeElement {
  currentTime: number;
  duration: number;
  paused: boolean;
  src: string;
  play: ReturnType<typeof vi.fn>;
  pause: ReturnType<typeof vi.fn>;
}

interface NativeHarness {
  current: { el: FakeElement; src: unknown; item?: PlaybackItem | null; url: string } | null;
  standby: { el: FakeElement; src: unknown; item?: PlaybackItem | null; url: string } | null;
  ctx: { state: string; resume: ReturnType<typeof vi.fn> } | null;
}

function element(paused = true): FakeElement {
  return {
    currentTime: 0,
    duration: 10,
    paused,
    src: '/track',
    play: vi.fn().mockResolvedValue(undefined),
    pause: vi.fn(),
  };
}

describe('NativeBackend transport ownership', () => {
  it('seeks a paused element without starting playback', async () => {
    const backend = new NativeBackend();
    const harness = backend as unknown as NativeHarness;
    const el = element(true);
    const onPrimed = vi.fn();
    harness.current = { el, src: null, url: '/track' };
    backend.onPrimed = onPrimed;

    await backend.seek(44100);
    expect(el.currentTime).toBe(1);
    expect(el.play).not.toHaveBeenCalled();
    expect(onPrimed).toHaveBeenCalledOnce();
  });

  it('promotes standby without playing until the controller requests it', async () => {
    const backend = new NativeBackend();
    const harness = backend as unknown as NativeHarness;
    const current = element(false);
    const standby = element(true);
    harness.current = { el: current, src: null, url: '/current' };
    harness.standby = {
      el: standby,
      src: null,
      url: '/standby',
      item: nativeItem('t2'),
    };

    // Standby/policy agreement: promotion requires the expected item.
    await expect(backend.advance(nativeItem('t9'))).resolves.toBeNull();
    expect(standby.play).not.toHaveBeenCalled();
    await expect(backend.advance(null)).resolves.toBeNull(); // nothing may follow
    await expect(backend.advance(nativeItem('t2'))).resolves.toMatchObject({ lengthSamples: 441000 });
    expect(current.pause).toHaveBeenCalledOnce();
    expect(standby.play).not.toHaveBeenCalled();

    backend.startPumping();
    expect(standby.play).not.toHaveBeenCalled();
    await backend.play();
    expect(standby.play).toHaveBeenCalledOnce();
  });

  it('cancels a play that is waiting for the AudioContext when pause wins the race', async () => {
    const backend = new NativeBackend();
    const harness = backend as unknown as NativeHarness;
    const el = element(true);
    let finishResume = () => {};
    harness.current = { el, src: null, url: '/track' };
    harness.ctx = {
      state: 'suspended',
      resume: vi.fn(() => new Promise<void>((resolve) => {
        finishResume = resolve;
      })),
    };

    const playing = backend.play();
    await backend.pause();
    finishResume();
    await playing;
    expect(el.play).not.toHaveBeenCalled();
    expect(el.pause).toHaveBeenCalledOnce();
  });

  it('does not let overlapping valid play requests pause each other', async () => {
    const backend = new NativeBackend();
    const harness = backend as unknown as NativeHarness;
    const el = element(true);
    let finishFirstPlay = () => {};
    el.play
      .mockImplementationOnce(() => new Promise<void>((resolve) => {
        finishFirstPlay = resolve;
      }))
      .mockResolvedValueOnce(undefined);
    harness.current = { el, src: null, url: '/track' };

    const first = backend.play();
    const second = backend.play();
    await second;
    finishFirstPlay();
    await first;
    expect(el.play).toHaveBeenCalledTimes(2);
    expect(el.pause).not.toHaveBeenCalled();
  });
});
