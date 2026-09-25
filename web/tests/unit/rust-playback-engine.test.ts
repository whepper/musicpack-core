// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-1: adapter contract tests for RustPlaybackEngine, driven by a fake
// worker client and a fake AudioContext/AudioWorklet sink. These verify the
// async TS Engine lifecycle <-> worker protocol mapping and generation
// isolation without a browser or the wasm build. The real application seam is
// exercised by tests/e2e/specs/rust-playback.spec.ts.

import { describe, expect, it } from 'vitest';
import {
  RustPlaybackEngine,
  type PlaybackWorkerLike,
  type RustAudioSink,
} from '../../app/src/lib/playback/rust-playback-engine';
import { createWebEngine } from '../../app/src/lib/playback/controller';
import type { PlaybackItem } from '../../player-core/src/types';

const INFO = { rate: 44100, channels: 2, version: 8, lengthSamples: 2116800 };

function item(url = '/a.mpc', id = 't1'): PlaybackItem {
  return {
    id,
    trackId: 1,
    source: { kind: 'http-range', url, byteSize: 493369 },
    title: 'T',
    artist: 'A',
    albumTitle: 'AL',
    codec: 'musepack-sv8',
  } as unknown as PlaybackItem;
}

class FakeWorker implements PlaybackWorkerLike {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  sent: Array<Record<string, unknown>> = [];
  terminated = 0;
  autoRespond = true;
  xfadeStatus = 'declined';

  postMessage(message: unknown): void {
    const m = message as Record<string, unknown>;
    this.sent.push(m);
    if (!this.autoRespond) return;
    const generation = m.generation as number;
    switch (m.type) {
      case 'open':
        this.emit({ type: 'opened', generation, info: INFO });
        break;
      case 'seek':
        this.emit({ type: 'seeked', generation, samples: 0 });
        break;
      case 'prepareNext':
        this.emit({ type: 'prepared', generation, info: { ...INFO, lengthSamples: 22050 } });
        break;
      case 'advance':
        this.emit({ type: 'advanced', generation, info: INFO });
        break;
      case 'beginCrossfade':
        this.emit({
          type: 'crossfade',
          generation,
          status: this.xfadeStatus,
          result: { info: INFO, overlapFrames: 1000 },
        });
        break;
      case 'openContainer':
        this.emit({
          type: 'containerTracks',
          generation,
          album: {
            container: m.container,
            size: m.size,
            title: 'Wasm Album',
            artists: ['The Packer'],
            tracks: [
              {
                number: 1,
                title: 'One',
                member: 'audio/01.mpc',
                source: `${m.container}`.replace(/^/, 'mpak:') + '#audio/01.mpc',
                size: 28226,
                codec: 'musepack-sv8',
                mimeType: 'audio/musepack',
              },
            ],
          },
        });
        break;
      case 'close':
        this.emit({ type: 'closed', generation });
        break;
      default:
        break;
    }
  }

  emit(data: unknown): void {
    this.onmessage?.({ data });
  }

  terminate(): void {
    this.terminated += 1;
  }
}

class FakeSink implements RustAudioSink {
  sampleRate = 44100;
  attaches: Array<{ frames: number; channels: number }> = [];
  resumed = 0;
  suspended = 0;
  closed = 0;

  attach(_control: SharedArrayBuffer, _data: SharedArrayBuffer, frames: number, channels: number): void {
    this.attaches.push({ frames, channels });
  }
  async resume(): Promise<void> {
    this.resumed += 1;
  }
  async suspend(): Promise<void> {
    this.suspended += 1;
  }
  async close(): Promise<void> {
    this.closed += 1;
  }
}

function makeEngine() {
  const worker = new FakeWorker();
  const sink = new FakeSink();
  const events: string[] = [];
  const engine = new RustPlaybackEngine({
    handlers: {
      primed: () => events.push('primed'),
      buffering: () => events.push('buffering'),
      eos: () => events.push('eos'),
      error: (message) => events.push('error:' + message),
      tick: () => events.push('tick'),
    },
    workerFactory: () => worker,
    sinkFactory: async () => sink,
  });
  return { engine, worker, sink, events };
}

async function opened() {
  const ctx = makeEngine();
  await ctx.engine.init('tok');
  const info = await ctx.engine.open(item());
  return { ...ctx, info };
}

describe('RustPlaybackEngine (production adapter seam)', () => {
  it('exposes honest capabilities and initializes', async () => {
    const { engine } = makeEngine();
    expect(engine.capabilities).toEqual({
      preloadNext: true,
      sampleAccurateGapless: true,
      decodeGate: true,
      crossfade: true,
    });
    await engine.init(null);
  });

  it('exposes the output rate for player-core seconds math', async () => {
    const worker = new FakeWorker();
    const sink = new FakeSink();
    sink.sampleRate = 48_000;
    const engine = new RustPlaybackEngine({
      handlers: {
        primed: () => undefined,
        buffering: () => undefined,
        eos: () => undefined,
        error: () => undefined,
        tick: () => undefined,
      },
      workerFactory: () => worker,
      sinkFactory: async () => sink,
    });
    await engine.init(null);
    // player-core divides output-rate sample counts by this; a 44100 fallback
    // at a 48 kHz output would inflate every duration/position by ~8.8%.
    expect(engine.rate).toBe(48_000);
  });

  it('opens through the worker, attaches a sink generation and returns StreamInfo', async () => {
    const { engine, worker, sink } = await opened();
    expect(worker.sent[0]).toMatchObject({
      type: 'open',
      url: '/a.mpc',
      size: 493369,
      token: 'tok',
      generation: 1,
    });
    expect(JSON.parse(worker.sent[0]!.itemJson as string).codec).toBe('musepack-sv8');
    expect(sink.attaches).toEqual([{ frames: 16384, channels: 2 }]);
    expect(engine.renderedSamples()).toBe(0);
  });

  it('play/pause drive the AudioContext, not the pump', async () => {
    const { engine, sink } = await opened();
    await engine.play();
    expect(sink.resumed).toBe(1);
    await engine.pause();
    expect(sink.suspended).toBe(1);
  });

  it('startPumping/pausePumping (and the DecodeGate aliases) reach the worker', async () => {
    const { engine, worker } = await opened();
    engine.startPumping();
    expect(engine.isPumping()).toBe(true);
    engine.pausePumping();
    expect(engine.isPumping()).toBe(false);
    engine.start();
    engine.stop();
    const types = worker.sent.map((m) => m.type);
    expect(types).toEqual(['open', 'startPumping', 'pausePumping', 'startPumping', 'pausePumping']);
  });

  it('seek bumps the generation, resets position and flushes the old generation', async () => {
    const { engine, worker, events } = await opened();
    worker.emit({ type: 'rendered', generation: 1, samples: 5000 });
    expect(engine.renderedSamples()).toBe(5000);
    await engine.seekSample(1234);
    expect(engine.renderedSamples()).toBe(0);
    const seek = worker.sent.find((m) => m.type === 'seek');
    expect(seek).toMatchObject({ samples: 1234, generation: 2 });
    // A late event from the dead generation is ignored.
    worker.emit({ type: 'rendered', generation: 1, samples: 9999 });
    expect(engine.renderedSamples()).toBe(0);
    expect(events.filter((e) => e === 'tick')).toHaveLength(1);
  });

  it('rendered/primed/buffering/eos map to Engine events once per transition', async () => {
    const { engine, worker, events } = await opened();
    engine.startPumping();
    worker.emit({ type: 'rendered', generation: 1, samples: 1024 });
    worker.emit({ type: 'primed', generation: 1 });
    worker.emit({ type: 'buffering', generation: 1 });
    worker.emit({ type: 'eos', generation: 1 });
    expect(events).toEqual(['tick', 'primed', 'buffering', 'eos']);
    expect(engine.renderedSamples()).toBe(1024);
    expect(engine.isOutputDrained()).toBe(true);
  });

  it('forwards gain to the Rust engine', async () => {
    const { engine, worker } = await opened();
    engine.setGain(0.25);
    expect(worker.sent.find((m) => m.type === 'setGain')).toMatchObject({ linear: 0.25 });
  });

  it('prepareNext and advance resolve with worker stream facts', async () => {
    const { engine } = await opened();
    expect((await engine.prepareNext(item('/b.mpc', 't2')))?.lengthSamples).toBe(22050);
    expect((await engine.advance(item('/b.mpc', 't2')))?.rate).toBe(44100);
  });

  it('reported position stays monotonic across a gapless advance', async () => {
    const { engine, worker } = await opened();
    engine.startPumping();
    worker.emit({ type: 'rendered', generation: 1, samples: 1_000_000 });
    expect(engine.renderedSamples()).toBe(1_000_000);
    // Rust restarts the promoted session's playhead; the adapter must carry
    // the retired session's frames so the album clock never regresses.
    await engine.advance(item('/b.mpc', 't2'));
    expect(engine.renderedSamples()).toBe(1_000_000);
    worker.emit({ type: 'rendered', generation: 1, samples: 512 });
    expect(engine.renderedSamples()).toBe(1_000_512);
    // A seek is a real reset (the core re-bases its own offset).
    await engine.seekSample(2048);
    expect(engine.renderedSamples()).toBe(0);
    worker.emit({ type: 'rendered', generation: 2, samples: 256 });
    expect(engine.renderedSamples()).toBe(256);
  });

  it('crossfade: declined resolves null', async () => {
    const { engine, worker } = await opened();
    worker.xfadeStatus = 'declined';
    expect(await engine.beginCrossfade(item('/b.mpc', 't2'), 4)).toBeNull();
  });

  it('crossfade: completed resolves with the overlap result', async () => {
    const { engine, worker } = await opened();
    worker.xfadeStatus = 'completed';
    const result = await engine.beginCrossfade(item('/b.mpc', 't2'), 4);
    expect(result?.overlapFrames).toBe(1000);
    expect(result?.info.rate).toBe(44100);
  });

  it('crossfade: pending resolves when the pump later completes it', async () => {
    const { engine, worker } = await opened();
    worker.autoRespond = false;
    const promise = engine.beginCrossfade(item('/b.mpc', 't2'), 4);
    worker.emit({ type: 'crossfade', generation: 1, status: 'pending' });
    worker.emit({
      type: 'crossfade',
      generation: 1,
      status: 'completed',
      result: { info: INFO, overlapFrames: 777 },
    });
    expect((await promise)?.overlapFrames).toBe(777);
  });

  it('worker error during playback tears the generation down and surfaces the error', async () => {
    const { engine, worker, events } = await opened();
    worker.emit({ type: 'error', generation: 1, message: 'decode failed' });
    expect(events).toContain('error:decode failed');
    await new Promise((r) => setTimeout(r, 0));
    expect(worker.terminated).toBe(1);
    expect(engine.isOpen()).toBe(false);
  });

  it('worker startup failure rejects open()', async () => {
    const { engine, worker } = makeEngine();
    worker.autoRespond = false;
    await engine.init(null);
    const promise = engine.open(item());
    await new Promise((r) => setTimeout(r, 0)); // let open() arm its pending slot
    worker.onerror?.({ message: 'worker boom' });
    await expect(promise).rejects.toThrow('worker boom');
  });

  it('close terminates the worker and closes the sink; reopen works without leaks', async () => {
    const { engine, worker, sink } = await opened();
    await engine.close();
    expect(worker.terminated).toBe(1);
    expect(sink.closed).toBe(1);
    expect(engine.isClosed()).toBe(true);
    // Reopen allocates a fresh generation and a fresh sink.
    const info = await engine.open(item('/c.mpc', 't3'));
    expect(info.rate).toBe(44100);
    expect(sink.attaches).toHaveLength(2);
    await engine.close();
  });

  it('reopening replaces the previous generation cleanly', async () => {
    const { engine, worker } = await opened();
    const first = engine;
    const info = await first.open(item('/b.mpc', 't2'));
    expect(info.rate).toBe(44100);
    expect(worker.terminated).toBe(1);
    const opens = worker.sent.filter((m) => m.type === 'open');
    expect(opens).toHaveLength(2);
    expect(opens[1]!.generation).toBe(2);
  });
});

describe('RustPlaybackEngine.openContainer (mpak MANF discovery)', () => {
  it('reads a container MANF and returns the canonical sources', async () => {
    const { engine, worker, sink } = makeEngine();
    await engine.init('tok');
    const album = await engine.openContainer('https://library.test/album.mpak', 1365);

    // The worker protocol: the container URL/key, its length and the session
    // token. The length is required — the container's tail framing is at the
    // end of the file.
    expect(worker.sent[0]).toMatchObject({
      type: 'openContainer',
      container: 'https://library.test/album.mpak',
      size: 1365,
      kind: 'http-range',
      token: 'tok',
    });

    // The reply is plain data with core's canonical member keys.
    expect(album.title).toBe('Wasm Album');
    expect(album.tracks).toHaveLength(1);
    expect(album.tracks[0]!.source).toBe(
      'mpak:https://library.test/album.mpak#audio/01.mpc',
    );
    // Discovery only reads bytes: nothing was decoded and no audio ring was
    // attached, so the container is never mistaken for playable audio.
    expect(sink.attaches).toHaveLength(0);
    expect(worker.sent.some((m) => m.type === 'open')).toBe(false);
  });

  it('spawns a worker on demand so no playback session is required', async () => {
    const { engine, worker } = makeEngine();
    // No `init`, no `open`: the capability brings up its own transport.
    const album = await engine.openContainer('/local.album.mpak', 4096, 'local-file');
    expect(album.tracks[0]!.member).toBe('audio/01.mpc');
    expect(worker.sent[0]).toMatchObject({ kind: 'local-file', container: '/local.album.mpak' });
  });

  it('does not disturb a live playback session', async () => {
    const { engine, worker, sink } = await opened();
    expect(worker.sent.filter((m) => m.type === 'open')).toHaveLength(1);

    // Reading a container while a track plays must not close the session: the
    // worker registers the container's transport alongside the live one.
    await engine.openContainer('https://library.test/other.mpak', 2048);

    expect(worker.terminated).toBe(0);
    expect(worker.sent.filter((m) => m.type === 'close')).toHaveLength(0);
    // The session's sink is still the one that was attached.
    expect(sink.attaches).toHaveLength(1);
  });

  it('isolates generations and rejects an in-flight read on replacement', async () => {
    const { engine, worker } = makeEngine();
    await engine.init('tok');
    worker.autoRespond = false;
    const pending = engine.openContainer('/a.mpak', 100);
    const generation = worker.sent[0]!.generation as number;

    // A reply carrying another generation's identity must not resolve this
    // request with someone else's tracks.
    worker.emit({
      type: 'containerTracks',
      generation: generation - 1,
      album: { container: '/stale.mpak', size: 1, title: 'Stale', artists: [], tracks: [] },
    });
    let settled = false;
    void pending.then(
      () => (settled = true),
      () => (settled = true),
    );
    await Promise.resolve();
    expect(settled).toBe(false);

    // Replacing the generation tears the worker down, so the read fails rather
    // than hanging forever.
    worker.autoRespond = true;
    await engine.open(item('/b.mpc'));
    await expect(pending).rejects.toThrow(/generation replaced/);
  });

  it('rejects when the worker fails', async () => {
    const { engine, worker } = makeEngine();
    await engine.init('tok');
    worker.autoRespond = false;
    const pending = engine.openContainer('/a.mpak', 100);
    worker.onerror?.({ message: 'boom' });
    await expect(pending).rejects.toThrow();
  });

  it('a failed read rejects only that request, and leaves playback alone', async () => {
    // A container that cannot be read reports through the worker's generic
    // error path, which is also the player's error path — so the adapter must
    // route it to the reader and not tear the session down.
    const { engine, worker, events } = await opened();
    worker.autoRespond = false;
    const pending = engine.openContainer('/broken.mpak', 100);
    const generation = worker.sent[worker.sent.length - 1]!.generation as number;
    worker.emit({
      type: 'error',
      generation,
      message: "cannot open container '/broken.mpak': no container header",
    });

    await expect(pending).rejects.toThrow(/cannot open container/);
    // No player error was raised and the session was not torn down.
    expect(events.filter((e) => e.startsWith('error:'))).toHaveLength(0);
    expect(worker.terminated).toBe(0);
  });
});

describe('createWebEngine seam', () => {
  it('keeps the legacy musepack construction unchanged', () => {
    const engine = createWebEngine('musepack', {
      primed: () => undefined,
      buffering: () => undefined,
      eos: () => undefined,
      error: () => undefined,
      tick: () => undefined,
    });
    expect(engine.capabilities.preloadNext).toBe(true);
    expect('open' in engine).toBe(true);
  });

  it('constructs the Rust adapter under the explicit rust kind', () => {
    const engine = createWebEngine('rust', {
      primed: () => undefined,
      buffering: () => undefined,
      eos: () => undefined,
      error: () => undefined,
      tick: () => undefined,
    });
    expect(engine).toBeInstanceOf(RustPlaybackEngine);
  });
});
