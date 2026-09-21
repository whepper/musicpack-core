// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it, vi } from 'vitest';
import { Player } from '../../player-core/src/player';
import { createQueueModel } from '../../player-core/src/queue';
import type { PlayerEvent } from '../../player-core/src/events';
import type { PlaybackItem } from '../../player-core/src/types';
function rng(): () => number {
  let s = 42;
  return () => {
    s = (s * 1103515245 + 12345) % 2147483648;
    return s / 2147483648;
  };
}


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

function silentPlayer(): { player: Player; queue: ReturnType<typeof createQueueModel> } {
  const queue = createQueueModel({ rng: rng() });
  const player = new Player(queue as never, {
    engineFactory: () => {
      throw new Error('no engine in this test');
    },
    resolveKind: () => 'musepack',
    storage: { get: () => null, set: () => undefined },
    now: () => 0,
    schedulePersist: () => 0,
  });
  return { player, queue };
}

describe('PlayerEvent surface (M7)', () => {
  it('emits policy events on setRepeat/setShuffle', () => {
    const { player } = silentPlayer();
    const events: PlayerEvent[] = [];
    player.on((e) => events.push(e));

    player.setRepeat('all');
    player.setShuffle(true);
    player.setRepeat('off');
    player.setShuffle(false);

    const policy = events.filter((e) => e.t === 'policy');
    expect(policy).toEqual([
      { t: 'policy', repeat: 'all', shuffle: false },
      { t: 'policy', repeat: 'all', shuffle: true },
      { t: 'policy', repeat: 'off', shuffle: true },
      { t: 'policy', repeat: 'off', shuffle: false },
    ]);
  });

  it('emits state and error events; unsubscribe stops delivery', async () => {
    const { player } = silentPlayer();
    const events: PlayerEvent[] = [];
    const unsub = player.on((e) => events.push(e));

    // drive a failure through the public surface (engineFactory throws).
    // NOTE: playItem resolves AFTER fail(); the await guarantees both events
    // landed synchronously before the assertions below.
    await player.playItem(item(1));
    const states = events.filter((e) => e.t === 'state').map((e) => (e as { state: string }).state);
    expect(states).toContain('loading');
    expect(states).toContain('error'); // fail() emits BOTH state and error
    expect(events.some((e) => e.t === 'error')).toBe(true);

    const count = events.length;
    unsub();
    player.setRepeat('one');
    expect(events.length).toBe(count); // no delivery after unsub
  });

  it('position events carry track-relative values for media-control consumers', async () => {
    // Use a scripted fake engine to drive tick() with real geometry.
    let handlers!: {
      primed(): void;
      buffering(): void;
      eos(): void;
      error(m: string): void;
      tick(): void;
    };
    let rendered = 0;
    const RATE = 44100;
    const LEN = RATE * 10;
    const queue = createQueueModel({ rng: rng() });
    const player = new Player(queue as never, {
      engineFactory: (_kind, h) => {
        handlers = h;
        return {
          capabilities: { preloadNext: true, sampleAccurateGapless: true, decodeGate: true, crossfade: false },
          init: async () => undefined,
          open: async () => ({ rate: RATE, channels: 2, version: 8, lengthSamples: LEN }),
          prepareNext: async () => null,
          advance: async () => null,
          play: async () => undefined,
          pause: async () => undefined,
          seekSample: async () => undefined,
          startPumping: () => undefined,
          pausePumping: () => undefined,
          setGain: () => undefined,
          renderedSamples: () => rendered,
          close: async () => undefined,
          on: () => () => undefined,
        };
      },
      resolveKind: () => 'musepack',
      storage: { get: () => null, set: () => undefined },
      now: () => 0,
      schedulePersist: () => 0,
      mediaControls: {
        bind: () => undefined,
        setMetadata: () => undefined,
        setPosition: vi.fn(),
      },
    });
    player.init();
    await player.playItem(item(1));
    handlers.primed();
    await Promise.resolve();

    const events: PlayerEvent[] = [];
    const media = (player as unknown as { ports: { mediaControls: { setPosition: ReturnType<typeof vi.fn> } } }).ports.mediaControls;
    const setPositionCalls = media.setPosition as ReturnType<typeof vi.fn>;
    void setPositionCalls;

    const positions: PlayerEvent[] = [];
    player.on((e) => {
      if (e.t === 'position') positions.push(e);
    });

    rendered = 3 * RATE; // 3 s into the (single, 10 s) track
    handlers.tick();

    expect(positions.length).toBe(1);
    const pos = positions[0] as Extract<PlayerEvent, { t: 'position' }>;
    expect(pos.positionSeconds).toBeCloseTo(3, 5); // within-track seconds
    expect(pos.trackStartSeconds).toBe(0);
    expect(pos.trackDurationSeconds).toBe(10);
  });

  it('track events fire on load and teardown(null)', async () => {
    let handlers!: { primed(): void; buffering(): void; eos(): void; error(m: string): void; tick(): void };
    const RATE = 44100;
    const queue = createQueueModel({ rng: rng() });
    const player = new Player(queue as never, {
      engineFactory: (_k, h) => {
        handlers = h;
        return {
          capabilities: { preloadNext: false, sampleAccurateGapless: false, decodeGate: false, crossfade: false },
          init: async () => undefined,
          open: async () => ({ rate: RATE, channels: 2, version: 0, lengthSamples: RATE * 5 }),
          play: async () => undefined,
          pause: async () => undefined,
          seekSample: async () => undefined,
          setGain: () => undefined,
          renderedSamples: () => 0,
          close: async () => undefined,
          on: () => () => undefined,
        };
      },
      resolveKind: () => 'musepack',
      storage: { get: () => null, set: () => undefined },
      now: () => 0,
      schedulePersist: () => 0,
    });
    player.init();

    const tracks: (PlaybackItem | null)[] = [];
    player.on((e) => {
      if (e.t === 'track') tracks.push(e.item);
    });

    await player.playItem(item(7));
    expect(tracks).toHaveLength(1);
    expect((tracks[0] as PlaybackItem).id).toBe('t7');

    await player.teardown();
    expect(tracks).toHaveLength(2);
    expect(tracks[1]).toBeNull();
  });
});
