// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// R4.4: sustained soak / production-observation suite for the Rust playback
// backend, driven through the real PlayerController.
//
// Everything here is observation/regression, not architecture: repeated
// lifecycle, seeking, queue transitions, a bounded-resource leak check, a
// long-running monotonicity check, repeated crossfades, and deterministic
// blocked-range cancellation (Playwright routes intercept the worker's range
// fetches). Rust is the only product backend; the frozen decoder runs only as
// the test-only oracle lane.

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

/** Page-level construction/liveness counters (test-only instrumentation).
 *  Counts page-created Workers and AudioContexts; producer workers are created
 *  per Rust generation, so their live count must return to a small bound. */
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

/** Album-title clicks navigate to the album detail page; return to the shelf
 *  (an SPA navigation, so in-memory player/queue state and counters persist). */
async function ensureShelf(page: Page): Promise<void> {
  const shelf = page.getByRole('heading', { name: 'The shelf' });
  if (await shelf.isVisible().catch(() => false)) return;
  await page.getByRole('link', { name: 'Albums' }).click();
  await expect(shelf).toBeVisible({ timeout: 15_000 });
}

/** Opens an album's detail page from the shelf via its link role. */
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

async function pauseBtn(page: Page): Promise<void> {
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
}

async function resumeBtn(page: Page): Promise<void> {
  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
}

async function setup(page: Page, backend: Backend): Promise<void> {
  await selectBackend(page, backend);
  await installResourceCounters(page);
  await signIn(page);
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe(backend);
}

test.describe('Rust soak (feature-flagged)', () => {
  test.setTimeout(180_000);

  test('repeated lifecycle (open/play/pause/resume/stop) keeps resources bounded', async ({ page }) => {
    await setup(page, 'rust');
    const cycles = 6;
    for (let i = 0; i < cycles; i++) {
      await playAlbum(page, 'Long Player');
      await pauseBtn(page);
      await resumeBtn(page);
      await page.evaluate(() => window.__musicpack!.player.stop());
      await waitFor(page, async () => (await playerState(page)).state === 'idle', { label: `idle ${i}` });
      expect((await playerState(page)).error).toBeUndefined();
    }
    // Repeated teardown/reopen: the engine (AudioContext + worker) is disposed
    // and rebuilt each time; no accumulation.
    const before = await resources(page);
    for (let i = 0; i < 4; i++) {
      await playAlbum(page, 'Long Player');
      await page.evaluate(() => window.__musicpack!.player.teardown());
      await waitFor(page, async () => (await playerState(page)).state === 'idle', { label: `teardown ${i}` });
    }
    const after = await resources(page);
    // eslint-disable-next-line no-console
    console.log(`soak[resources] before=${JSON.stringify(before)} after=${JSON.stringify(after)} cycles=${cycles}`);
    expect(after.workersLive).toBeLessThanOrEqual(before.workersLive + 1);
    expect(after.contextsLive).toBeLessThanOrEqual(before.contextsLive + 1);
    expect(after.workersCreated).toBeGreaterThan(before.workersCreated); // it really did rebuild
  });

  test('seek soak: sweep while playing and paused, then rapid seeks', async ({ page }) => {
    await setup(page, 'rust');
    await playAlbum(page, 'Long Player');
    const targets = [2, 12, 24, 36, 45, 6];
    for (const t of targets) {
      await page.evaluate((v) => window.__musicpack!.player.seek(v), t);
      await waitFor(page, async () => {
        const s = await playerState(page);
        return !s.error && s.state !== 'error' && Math.abs(s.positionSeconds - t) < 2.5;
      }, { label: `seek ${t}`, timeout: 15_000 });
      expect((await playerState(page)).state).not.toBe('error');
    }
    // Paused seeks: position must settle and stay frozen.
    await pauseBtn(page);
    for (const t of [20, 40, 10]) {
      await page.evaluate((v) => window.__musicpack!.player.seek(v), t);
      await waitFor(page, async () => Math.abs((await playerState(page)).positionSeconds - t) < 2.5, {
        label: `paused seek ${t}`,
        timeout: 15_000,
      });
    }
    const frozen = (await playerState(page)).positionSeconds;
    await page.waitForTimeout(400);
    expect(Math.abs((await playerState(page)).positionSeconds - frozen)).toBeLessThan(0.3);
    await resumeBtn(page);

    // Rapid-fire seeks must converge on the last target with no stale state.
    void page.evaluate(() => {
      const p = window.__musicpack!.player;
      void p.seek(5);
      void p.seek(30);
      void p.seek(15);
      void p.seek(42);
    });
    await waitFor(page, async () => {
      const s = await playerState(page);
      return s.state === 'playing' && Math.abs(s.positionSeconds - 42) < 3;
    }, { label: 'rapid seeks converge', timeout: 20_000 });
    expect((await playerState(page)).error).toBeUndefined();
  });

  test('queue soak: repeated album EOS, next/previous bursts stay coherent', async ({ page }) => {
    await setup(page, 'rust');
    for (let round = 0; round < 2; round++) {
      await playAlbum(page, 'Synthetic Test Compilation', 40_000);
      await waitFor(page, async () => (await playerState(page)).state === 'ended', {
        label: `ended round ${round}`,
        timeout: 60_000,
      });
      const s = await playerState(page);
      const q = await page.evaluate(() => window.__musicpack!.queue.get());
      expect(s.error).toBeUndefined();
      expect(q.index).toBe(q.items.length - 1);
    }
    // next/previous bursts on Long Player (48 s opener gives room).
    await playAlbum(page, 'Long Player');
    for (let i = 0; i < 3; i++) {
      await page.evaluate(() => void window.__musicpack!.player.next());
      await page.waitForTimeout(300);
    }
    for (let i = 0; i < 2; i++) {
      await page.evaluate(() => void window.__musicpack!.player.previous());
      await page.waitForTimeout(300);
    }
    await page.evaluate(() => void window.__musicpack!.player.next());
    await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing after bursts' });
    expect((await playerState(page)).error).toBeUndefined();
    const q = await page.evaluate(() => window.__musicpack!.queue.get());
    expect(q.index).toBeGreaterThanOrEqual(0);
    expect(q.index).toBeLessThan(q.items.length);
  });

  test('long-running playback: monotonic position, bounded resources/underruns', async ({ page }) => {
    await setup(page, 'rust');
    await playAlbum(page, 'Long Player');
    const samples: number[] = [];
    const startRes = await resources(page);
    const t0 = Date.now();
    while (Date.now() - t0 < 15_000) {
      const s = await playerState(page);
      if (s.error) throw new Error(`player errored: ${s.error}`);
      samples.push(s.positionSeconds);
      await page.waitForTimeout(400);
    }
    for (let i = 1; i < samples.length; i++) {
      expect(samples[i]).toBeGreaterThanOrEqual(samples[i - 1] - 0.05);
    }
    expect(samples[samples.length - 1]).toBeGreaterThan(5);
    const endRes = await resources(page);
    const diag = await diagnostics(page);
    // eslint-disable-next-line no-console
    console.log(`soak[longrun] samples=${samples.length} last=${samples[samples.length - 1]} res=${JSON.stringify(endRes)} diag=${JSON.stringify(diag)}`);
    expect(endRes.workersLive).toBeLessThanOrEqual(startRes.workersLive);
    expect(endRes.contextsLive).toBeLessThanOrEqual(startRes.contextsLive);
  });

  test('repeated crossfade boundaries: taken, advances, monotonic', {
    timeout: 180_000,
  }, async ({ page }) => {
    await setup(page, 'rust');
    for (let round = 0; round < 2; round++) {
      await playAlbum(page, 'Fade Rider');
      await page.evaluate(() => (window.__musicpack!.player as unknown as { setCrossfade(s: number): void }).setCrossfade(4));
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
      const first = (await playerState(page)).currentTitle;
      await page.evaluate(() => (window.__musicpack!.player as unknown as { seek(s: number): void }).seek(45.5));
      let result = 'none';
      let advanced = false;
      for (let i = 0; i < 80 && !(result === 'taken' && advanced); i++) {
        await page.waitForTimeout(250);
        const s = await playerState(page);
        if (s.state === 'error') throw new Error(`player errored: ${s.error}`);
        if (s.currentTitle && s.currentTitle !== first) advanced = true;
        result = await page.evaluate(() => (window as unknown as { __xf?: string }).__xf ?? 'none');
      }
      // eslint-disable-next-line no-console
      console.log(`soak[crossfade] round=${round} result=${result} advanced=${advanced}`);
      expect(result).toBe('taken');
      expect(advanced).toBe(true);
      expect((await playerState(page)).error).toBeUndefined();
    }
  });

  test('deterministic cancellation: blocked range fetch superseded by next', async ({ page }) => {
    await setup(page, 'rust');
    const gate = { blocked: false, held: [] as Array<() => void>, intercepted: 0 };
    await page.route('**/audio*', async (route) => {
      gate.intercepted++;
      if (!gate.blocked) {
        await route.continue().catch(() => undefined);
        return;
      }
      await new Promise<void>((resolve) => gate.held.push(resolve));
      await route.continue().catch(() => undefined);
    });

    gate.blocked = true;
    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await page.waitForTimeout(600); // the open is now blocked in the worker
    expect(gate.intercepted).toBeGreaterThan(0);

    // Supersede the blocked generation, then release everything.
    await page.evaluate(() => void window.__musicpack!.player.next());
    await page.waitForTimeout(600);
    gate.blocked = false;
    gate.held.splice(0).forEach((r) => r());

    await waitFor(page, async () => {
      const s = await playerState(page);
      return s.state === 'playing';
    }, { label: 'superseding generation plays', timeout: 40_000 });
    const s = await playerState(page);
    // eslint-disable-next-line no-console
    console.log(`soak[cancel-supersede] state=${s.state} title=${s.currentTitle} intercepted=${gate.intercepted}`);
    expect(s.error).toBeUndefined();
    const res = await resources(page);
    expect(res.workersLive).toBeLessThanOrEqual(1);
  });

  test('deterministic cancellation: blocked open then stop has no stale effect', async ({ page }) => {
    await setup(page, 'rust');
    const gate = { blocked: false, held: [] as Array<() => void>, intercepted: 0 };
    await page.route('**/audio*', async (route) => {
      gate.intercepted++;
      if (!gate.blocked) {
        await route.continue().catch(() => undefined);
        return;
      }
      await new Promise<void>((resolve) => gate.held.push(resolve));
      await route.continue().catch(() => undefined);
    });

    gate.blocked = true;
    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await page.waitForTimeout(600);
    expect(gate.intercepted).toBeGreaterThan(0);
    void page.evaluate(() => window.__musicpack!.player.stop());
    await page.waitForTimeout(300);
    gate.blocked = false;
    gate.held.splice(0).forEach((r) => r());

    await waitFor(page, async () => (await playerState(page)).state === 'idle', {
      label: 'idle after blocked stop',
      timeout: 20_000,
    });
    await page.waitForTimeout(500);
    const s = await playerState(page);
    // eslint-disable-next-line no-console
    console.log(`soak[cancel-stop] state=${s.state} pos=${s.positionSeconds} intercepted=${gate.intercepted}`);
    expect(s.state).toBe('idle');
    expect(s.error).toBeUndefined();
  });
});

test.describe('Legacy comparison soak', () => {
  test.setTimeout(180_000);

  test('repeated lifecycle + seek + EOS reach the same observable states', async ({ page }) => {
    await setup(page, 'legacy');
    // lifecycle
    for (let i = 0; i < 4; i++) {
      await playAlbum(page, 'Long Player');
      await pauseBtn(page);
      await resumeBtn(page);
      await page.evaluate(() => window.__musicpack!.player.stop());
      await waitFor(page, async () => (await playerState(page)).state === 'idle', { label: `idle ${i}` });
    }
    // seek sweep
    await playAlbum(page, 'Long Player');
    for (const t of [2, 24, 45, 10]) {
      await page.evaluate((v) => window.__musicpack!.player.seek(v), t);
      await waitFor(page, async () => Math.abs((await playerState(page)).positionSeconds - t) < 2.5, {
        label: `legacy seek ${t}`,
        timeout: 15_000,
      });
    }
    // EOS
    await playAlbum(page, 'Synthetic Test Compilation', 40_000);
    await waitFor(page, async () => (await playerState(page)).state === 'ended', {
      label: 'legacy ended',
      timeout: 60_000,
    });
    const s = await playerState(page);
    const q = await page.evaluate(() => window.__musicpack!.queue.get());
    expect(s.error).toBeUndefined();
    expect(q.index).toBe(q.items.length - 1);
  });
});
