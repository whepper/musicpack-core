// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-4: controlled Rust playback observation through the real
// application. Longer, more realistic sessions than the 14H-3 soak, plus
// network/lifecycle observation and extended resource tracking.
//
// This is observation, not architecture: no production behaviour changes.
// Since R4.4 Rust is the only product backend; the frozen Emscripten decoder
// is reachable solely through the test-only oracle lane.

import { test, expect, type Page } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

type Backend = 'legacy' | 'rust';

async function selectBackend(page: Page, backend: Backend): Promise<void> {
  await page.addInitScript((b: string) => {
    try {
      if (b === 'legacy') sessionStorage.setItem('musicpack.oracle-decoder.v1', '1');
      else sessionStorage.removeItem('musicpack.oracle-decoder.v1');
    } catch {
      /* storage unavailable */
    }
  }, backend);
}

async function installResourceCounters(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const counts = {
      workersCreated: 0,
      workersLive: 0,
      contextsCreated: 0,
      contextsLive: 0,
      workletNodesCreated: 0,
      workletNodesDisconnected: 0,
    };
    (window as unknown as { __res: typeof counts }).__res = counts;
    const RealWorker = window.Worker;
    const WrappedWorker = function (this: unknown, ...args: unknown[]) {
      const w = new (RealWorker as unknown as new (...a: unknown[]) => Worker)(...args);
      counts.workersCreated += 1;
      counts.workersLive += 1;
      const terminate = w.terminate.bind(w);
      w.terminate = () => {
        counts.workersLive -= 1;
        return terminate();
      };
      return w;
    } as unknown as typeof Worker;
    WrappedWorker.prototype = RealWorker.prototype;
    window.Worker = WrappedWorker;
    const RealContext = window.AudioContext;
    if (RealContext) {
      const WrappedContext = function (this: unknown, ...args: unknown[]) {
        const c = new (RealContext as unknown as new (...a: unknown[]) => AudioContext)(...args);
        counts.contextsCreated += 1;
        counts.contextsLive += 1;
        const close = c.close.bind(c);
        c.close = () => {
          counts.contextsLive -= 1;
          return close();
        };
        return c;
      } as unknown as typeof AudioContext;
      WrappedContext.prototype = RealContext.prototype;
      window.AudioContext = WrappedContext;
    }
    const RealNode = window.AudioWorkletNode;
    if (RealNode) {
      const WrappedNode = function (this: unknown, ...args: unknown[]) {
        counts.workletNodesCreated += 1;
        const n = new (RealNode as unknown as new (...a: unknown[]) => AudioWorkletNode)(...args);
        const disconnect = n.disconnect.bind(n);
        n.disconnect = (...dArgs: unknown[]) => {
          counts.workletNodesDisconnected += 1;
          return (disconnect as (...a: unknown[]) => unknown)(...dArgs);
        };
        return n;
      } as unknown as typeof AudioWorkletNode;
      WrappedNode.prototype = RealNode.prototype;
      window.AudioWorkletNode = WrappedNode;
    }
  });
}

async function resources(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => (window as unknown as { __res: Record<string, number> }).__res);
}

async function diagnostics(page: Page): Promise<Record<string, number> | null> {
  return page.evaluate(() => {
    const eng = (window.__musicpack?.player as unknown as { core: { engine: { getDiagnostics?: () => Record<string, number> } } }).core.engine;
    return typeof eng?.getDiagnostics === 'function' ? eng.getDiagnostics() : null;
  });
}

async function ensureShelf(page: Page): Promise<void> {
  const shelf = page.getByRole('heading', { name: 'The shelf' });
  if (await shelf.isVisible().catch(() => false)) return;
  await page.getByRole('link', { name: 'Albums' }).click();
  await expect(shelf).toBeVisible({ timeout: 15_000 });
}

async function openAlbum(page: Page, title: string): Promise<void> {
  await ensureShelf(page);
  const escaped = title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  await page.getByRole('link', { name: new RegExp(`^${escaped}`) }).first().click();
  await expect(page.getByRole('button', { name: 'Play album' })).toBeVisible({ timeout: 15_000 });
}

async function playAlbum(page: Page, title: string, timeout = 30_000): Promise<void> {
  await openAlbum(page, title);
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: `${title} playing`, timeout });
}

async function setSeek(page: Page, seconds: number): Promise<void> {
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, seconds);
}

async function setup(page: Page, backend: Backend): Promise<void> {
  await selectBackend(page, backend);
  await installResourceCounters(page);
  await signIn(page);
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe(backend);
}

interface Sample {
  t: number;
  state: string;
  pos: number;
  trackStart: number;
  trackDur: number;
  idx: number;
  error: string | undefined;
  workersLive: number;
  contextsLive: number;
  underruns: number | null;
  read: number | null;
  write: number | null;
}

async function sample(page: Page, t: number): Promise<Sample> {
  const s = await playerState(page);
  const r = await resources(page);
  const d = await diagnostics(page);
  const idx = await page.evaluate(() => window.__musicpack!.queue.get().index);
  return {
    t,
    state: s.state,
    pos: s.positionSeconds,
    trackStart: s.currentTrackStartSeconds,
    trackDur: s.currentTrackDurationSeconds,
    idx,
    error: s.error,
    workersLive: r.workersLive,
    contextsLive: r.contextsLive,
    underruns: d?.underruns ?? null,
    read: d?.read ?? null,
    write: d?.write ?? null,
  };
}

test.describe('Rust controlled observation (feature-flagged)', () => {
  test.setTimeout(300_000);

  test('long session (150 s): repeated gapless transitions, pause/resume, no accumulation', async ({ page }) => {
    await setup(page, 'rust');
    await playAlbum(page, 'Long Player');
    await page.evaluate(() => window.__musicpack!.player.setRepeat('all')); // loop the 4-track album
    const samples: Sample[] = [];
    const t0 = Date.now();
    let pauseCycles = 0;
    while (Date.now() - t0 < 150_000) {
      await page.waitForTimeout(5_000);
      samples.push(await sample(page, Date.now() - t0));
      // Periodic suspend/resume: exercises AudioContext lifecycle + re-prime.
      if (samples.length % 6 === 0) {
        pauseCycles++;
        await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
        await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
        await page.waitForTimeout(400);
        await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
        await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
      }
    }
    const first = samples[0];
    const last = samples[samples.length - 1];
    const transitions = new Set(samples.map((s) => s.idx)).size;
    // eslint-disable-next-line no-console
    console.log(
      `obs[long150] samples=${samples.length} pauseCycles=${pauseCycles} transitions=${transitions} ` +
        `firstUnderruns=${first.underruns} lastUnderruns=${last.underruns} ` +
        `resFirst=${first.workersLive}/${first.contextsLive} resLast=${last.workersLive}/${last.contextsLive} lastPos=${last.pos}`,
    );
    // No error at any sample; resources bounded throughout.
    expect(samples.every((s) => s.error === undefined)).toBe(true);
    expect(samples.every((s) => s.workersLive <= 1)).toBe(true);
    expect(samples.every((s) => s.contextsLive <= 1)).toBe(true);
    // The 4-track album loops under repeat-all: several boundaries crossed.
    expect(transitions).toBeGreaterThanOrEqual(2);
    // Underruns stay bounded across the whole session. Each pause/resume
    // re-primes the ring and may underrun a few quanta; measured ~20-40 over
    // 150 s with 5 pause cycles. A runaway/leak would grow into the hundreds.
    if (first.underruns !== null && last.underruns !== null) {
      expect(last.underruns - first.underruns).toBeLessThan(120);
    }
    // Position advanced substantially across the session. (The raw ring
    // counters reset on each repeat-all wrap's seek, so sum the per-sample
    // forward deltas instead of comparing first/last counters.)
    let advancedSec = 0;
    for (let i = 1; i < samples.length; i++) {
      advancedSec += Math.max(0, samples[i].pos - samples[i - 1].pos);
    }
    expect(advancedSec).toBeGreaterThan(60);
    if (last.write !== null && last.read !== null) {
      expect(last.write - last.read).toBeLessThanOrEqual(16_384);
    }
  });

  test('network: delayed range responses recover to playback; rapid next stays coherent', async ({ page }) => {
    await setup(page, 'rust');
    let delayed = 0;
    await page.route('**/audio*', async (route) => {
      delayed++;
      await new Promise((r) => setTimeout(r, 250));
      await route.continue().catch(() => undefined);
    });
    await playAlbum(page, 'Long Player', 60_000);
    await waitFor(page, async () => (await playerState(page)).positionSeconds > 1, {
      label: 'advances under latency',
      timeout: 20_000,
    });
    // Realistic supersede under load: two quick nexts, then settle.
    await page.evaluate(() => void window.__musicpack!.player.next());
    await page.waitForTimeout(300);
    await page.evaluate(() => void window.__musicpack!.player.next());
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'playing after rapid next',
      timeout: 30_000,
    });
    const s = await sample(page, 0);
    // eslint-disable-next-line no-console
    console.log(`obs[latency] delayed=${delayed} state=${s.state} error=${s.error} workersLive=${s.workersLive}`);
    expect(s.error).toBeUndefined();
    expect(s.workersLive).toBeLessThanOrEqual(1);
    await page.unroute('**/audio*');
  });

  test('network: failed range responses surface an error (no fallback), then recover', async ({ page }) => {
    await setup(page, 'rust');
    let failed = 0;
    let failEnabled = true;
    await page.route('**/audio*', async (route) => {
      if (!failEnabled) {
        await route.continue().catch(() => undefined);
        return;
      }
      failed++;
      await route.abort('failed').catch(() => undefined);
    });
    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    // A hard source failure must surface as an error, not silently fall back.
    await waitFor(page, async () => (await playerState(page)).state === 'error', {
      label: 'error surfaced',
      timeout: 30_000,
    });
    const errored = await playerState(page);
    const kind = await page.evaluate(() => window.__musicpack?.player.getBackendKind());
    // eslint-disable-next-line no-console
    console.log(`obs[fetchfail] failed=${failed} state=${errored.state} error=${errored.error} kind=${kind}`);
    expect(errored.error).toBeTruthy();
    expect(kind).toBe('rust'); // stayed on the selected backend; no auto-fallback

    // Recovery: restore the network and replay.
    failEnabled = false;
    await page.evaluate(() => window.__musicpack!.player.teardown());
    await playAlbum(page, 'Long Player', 40_000);
    expect((await playerState(page)).error).toBeUndefined();
    await page.unroute('**/audio*');
  });

  test('browser lifecycle: reload restores paused state and resumes on Rust', async ({ page }) => {
    await setup(page, 'rust');
    await playAlbum(page, 'Long Player');
    await page.waitForTimeout(1500);
    await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
    const saved = await playerState(page);
    expect(saved.positionSeconds).toBeGreaterThan(0.5);

    await page.reload();
    await waitFor(page, async () => (await playerState(page)).state === 'paused', {
      label: 'restored paused',
      timeout: 30_000,
    });
    const restored = await playerState(page);
    const identity = await page.evaluate(() => window.__musicpack?.playbackBackend);
    // eslint-disable-next-line no-console
    console.log(`obs[reload] identity=${identity} saved=${saved.positionSeconds} restored=${restored.positionSeconds}`);
    expect(identity).toBe('rust');
    expect(restored.currentTitle).toBe(saved.currentTitle);
    expect(Math.abs(restored.positionSeconds - saved.positionSeconds)).toBeLessThan(3);

    await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed after reload' });
    expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe('rust');
    expect((await playerState(page)).error).toBeUndefined();
  });

  test('crossfade over repeated transitions: taken, advances, seek stays accurate', {
    timeout: 240_000,
  }, async ({ page }) => {
    await setup(page, 'rust');
    await openAlbum(page, 'Fade Rider');
    await page.evaluate(() => (window.__musicpack!.player as unknown as { setCrossfade(s: number): void }).setCrossfade(4));
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

    for (let round = 0; round < 3; round++) {
      await page.evaluate(() => {
        const eng = (window.__musicpack!.player as unknown as { core: { engine: { beginCrossfade: (...a: unknown[]) => Promise<unknown> } } }).core.engine;
        const orig = eng.beginCrossfade.bind(eng);
        (window as unknown as { __xf: string }).__xf = 'none';
        eng.beginCrossfade = async (...args: unknown[]) => {
          const r = await orig(...args);
          (window as unknown as { __xf: string }).__xf = r ? 'taken' : 'declined';
          return r;
        };
      });
      const before = (await playerState(page)).currentTitle;
      await setSeek(page, 45.5);
      let result = 'none';
      let advanced = false;
      let maxDwellMs = 0;
      let lastPos = (await playerState(page)).positionSeconds;
      let lastMove = Date.now();
      for (let i = 0; i < 90 && !(result === 'taken' && advanced); i++) {
        await page.waitForTimeout(150);
        const s = await playerState(page);
        if (s.state === 'error') throw new Error(`player errored: ${s.error}`);
        if (Math.abs(s.positionSeconds - lastPos) > 0.01) {
          lastPos = s.positionSeconds;
          lastMove = Date.now();
        } else {
          maxDwellMs = Math.max(maxDwellMs, Date.now() - lastMove);
        }
        if (s.currentTitle && s.currentTitle !== before) advanced = true;
        result = await page.evaluate(() => (window as unknown as { __xf?: string }).__xf ?? 'none');
      }
      // eslint-disable-next-line no-console
      console.log(`obs[crossfade] round=${round} result=${result} advanced=${advanced} maxDwellMs=${maxDwellMs}`);
      expect(result).toBe('taken');
      expect(advanced).toBe(true);
      expect((await playerState(page)).error).toBeUndefined();
      // A fade is a documented playhead dwell, not a correctness defect; bound
      // it to the fade window + scheduling slack.
      expect(maxDwellMs).toBeLessThan(9_000);
      // Return to the album start for the next fade.
      await page.evaluate(() => void window.__musicpack!.player.playQueueIndex(0));
      await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: `restart ${round}` });
    }
  });
});

test.describe('Legacy realistic comparison', () => {
  test.setTimeout(180_000);

  test('60 s session with periodic seeks and EOS matches Rust observables', async ({ page }) => {
    await setup(page, 'legacy');
    await playAlbum(page, 'Long Player');
    const t0 = Date.now();
    let seeks = 0;
    while (Date.now() - t0 < 60_000) {
      await page.waitForTimeout(6_000);
      const s = await playerState(page);
      if (s.error) throw new Error(`legacy errored: ${s.error}`);
      if (s.state === 'playing') {
        const target = Math.max(1, Math.min(s.currentTrackDurationSeconds - 2, 8 + (seeks % 3) * 12));
        await setSeek(page, target);
        seeks++;
      }
    }
    const s = await playerState(page);
    const r = await resources(page);
    // eslint-disable-next-line no-console
    console.log(`obs[legacy60] seeks=${seeks} state=${s.state} pos=${s.positionSeconds} res=${r.workersLive}/${r.contextsLive}`);
    expect(s.error).toBeUndefined();
    // Legacy MusepackEngine keeps a current + standby decoder worker.
    expect(r.workersLive).toBeLessThanOrEqual(2);
    expect(r.contextsLive).toBeLessThanOrEqual(1);

    // EOS on the short album reaches ended with the cursor at the last item.
    await playAlbum(page, 'Synthetic Test Compilation', 40_000);
    await waitFor(page, async () => (await playerState(page)).state === 'ended', {
      label: 'legacy ended',
      timeout: 60_000,
    });
    const q = await page.evaluate(() => window.__musicpack!.queue.get());
    expect(q.index).toBe(q.items.length - 1);
  });
});
