// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-4.1: characterize Rust underruns. Distinguishes start-up/resume
// underruns from continuous-playback accumulation by running an uninterrupted
// session and a pause/resume session separately, sampling the engine's ring
// diagnostics over several minutes.

import { test, expect, type Page } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

async function setup(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const counts = { workersLive: 0, contextsLive: 0 };
    (window as unknown as { __res: typeof counts }).__res = counts;
    const RealWorker = window.Worker;
    const W = function (this: unknown, ...a: unknown[]) {
      const w = new (RealWorker as unknown as new (...x: unknown[]) => Worker)(...a);
      counts.workersLive += 1;
      const t = w.terminate.bind(w);
      w.terminate = () => {
        counts.workersLive -= 1;
        return t();
      };
      return w;
    } as unknown as typeof Worker;
    W.prototype = RealWorker.prototype;
    window.Worker = W;
    const RealContext = window.AudioContext;
    if (RealContext) {
      const C = function (this: unknown, ...a: unknown[]) {
        const c = new (RealContext as unknown as new (...x: unknown[]) => AudioContext)(...a);
        counts.contextsLive += 1;
        const close = c.close.bind(c);
        c.close = () => {
          counts.contextsLive -= 1;
          return close();
        };
        return c;
      } as unknown as typeof AudioContext;
      C.prototype = RealContext.prototype;
      window.AudioContext = C;
    }
  });
  await signIn(page);
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
}

async function resources(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => (window as unknown as { __res: Record<string, number> }).__res);
}

async function diag(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => {
    const eng = (window.__musicpack?.player as unknown as { core: { engine: { getDiagnostics: () => Record<string, number> } } }).core.engine;
    return eng.getDiagnostics();
  });
}

async function rendered(page: Page): Promise<number> {
  return page.evaluate(() => {
    const eng = (window.__musicpack?.player as unknown as { core: { engine: { renderedSamples: () => number } } }).core.engine;
    return eng.renderedSamples();
  });
}

async function ensureShelf(page: Page): Promise<void> {
  const shelf = page.getByRole('heading', { name: 'The shelf' });
  if (await shelf.isVisible().catch(() => false)) return;
  await page.getByRole('link', { name: 'Albums' }).click();
  await expect(shelf).toBeVisible({ timeout: 15_000 });
}

async function playAlbum(page: Page, title: string, timeout = 40_000): Promise<void> {
  await ensureShelf(page);
  const escaped = title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  await page.getByRole('link', { name: new RegExp(`^${escaped}`) }).first().click();
  await expect(page.getByRole('button', { name: 'Play album' })).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: `${title} playing`, timeout });
}

interface Row {
  t: number;
  underruns: number;
  read: number;
  write: number;
  minOcc: number;
  maxOcc: number;
  rendered: number;
  pos: number;
  state: string;
  workersLive: number;
  contextsLive: number;
  idx: number;
}

async function row(page: Page, t: number): Promise<Row> {
  const d = await diag(page);
  const s = await playerState(page);
  const r = await resources(page);
  const idx = await page.evaluate(() => window.__musicpack!.queue.get().index);
  return {
    t,
    underruns: d.underruns,
    read: d.read,
    write: d.write,
    minOcc: d.minOcc,
    maxOcc: d.maxOcc,
    rendered: await rendered(page),
    pos: s.positionSeconds,
    state: s.state,
    workersLive: r.workersLive,
    contextsLive: r.contextsLive,
    idx,
  };
}

const DURATION_MS = 180_000;
const STEP_MS = 3_000;

test.describe('Rust underrun characterization (14H-4.1)', () => {
  test.setTimeout(300_000);

  test('A: uninterrupted 180 s — underruns bounded, position monotonic, resources flat', async ({ page }) => {
    await setup(page);
    await playAlbum(page, 'Long Player');
    await page.evaluate(() => window.__musicpack!.player.setRepeat('all'));
    const rows: Row[] = [];
    const t0 = Date.now();
    while (Date.now() - t0 < DURATION_MS) {
      await page.waitForTimeout(STEP_MS);
      const r = await row(page, Date.now() - t0);
      if (await page.evaluate(() => window.__musicpack!.player.model.get().error)) {
        throw new Error(`player errored: ${await page.evaluate(() => window.__musicpack!.player.model.get().error)}`);
      }
      rows.push(r);
    }
    const first = rows[0];
    const last = rows[rows.length - 1];
    // Reset-aware underrun growth per 60 s window. `UNDERRUNS` returns to 0
    // whenever a repeat-all wrap seeks (ring reset), so a raw first/last delta
    // is meaningless; count forward growth and re-count from 0 on a reset.
    const windows = [0, 0, 0];
    let lastU = rows[0].underruns;
    for (let i = 1; i < rows.length; i++) {
      const w = Math.min(2, Math.floor((i * 3) / rows.length));
      const cur = rows[i].underruns;
      windows[w] += cur >= lastU ? cur - lastU : cur;
      lastU = cur;
    }
    const totalUnderruns = windows.reduce((a, b) => a + b, 0);
    let advancedSec = 0;
    for (let i = 1; i < rows.length; i++) advancedSec += Math.max(0, rows[i].pos - rows[i - 1].pos);
    const states = [...new Set(rows.map((r) => r.state))];
    // eslint-disable-next-line no-console
    console.log(
      `underrun[A] samples=${rows.length} totalUnderruns=${totalUnderruns} windows=${JSON.stringify(windows)} ` +
        `advancedSec=${advancedSec.toFixed(1)} res=${last.workersLive}/${last.contextsLive} states=${states.join(',')}`,
    );
    expect(rows.every((r) => r.workersLive <= 1 && r.contextsLive <= 1)).toBe(true);
    expect(advancedSec).toBeGreaterThan(120);
    // The property under test is "no sustained degradation": no playback error
    // or stuck state, no runaway underruns, and no acceleration into the final
    // third. Individual underruns are scheduling-dependent and allowed.
    expect(states).not.toContain('error');
    expect(states).not.toContain('loading');
    expect(totalUnderruns).toBeLessThan(200);
    expect(windows[2]).toBeLessThanOrEqual(windows[0] + 40);
    // Ring stayed bounded; min/max occupancy sane.
    expect(last.write - last.read).toBeLessThanOrEqual(16_384);
    expect(last.maxOcc).toBeLessThanOrEqual(16_384);
  });

  test('B: pause/resume 180 s — underruns attributable to each resume, bounded', async ({ page }) => {
    await setup(page);
    await playAlbum(page, 'Long Player');
    await page.evaluate(() => window.__musicpack!.player.setRepeat('all'));
    const resumeDeltas: number[] = [];
    const t0 = Date.now();
    let cycles = 0;
    let lastUnderruns = (await diag(page)).underruns;
    while (Date.now() - t0 < DURATION_MS) {
      await page.waitForTimeout(18_000);
      const before = (await diag(page)).underruns;
      await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
      await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
      await page.waitForTimeout(350);
      await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
      await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
      await page.waitForTimeout(2_500); // let the re-prime settle
      const after = (await diag(page)).underruns;
      resumeDeltas.push(after - before);
      lastUnderruns = after;
      cycles++;
      if (await page.evaluate(() => window.__musicpack!.player.model.get().error)) {
        throw new Error('player errored during pause/resume');
      }
    }
    const r = await resources(page);
    // eslint-disable-next-line no-console
    console.log(
      `underrun[B] cycles=${cycles} totalUnderruns=${lastUnderruns} perResume=${JSON.stringify(resumeDeltas)} res=${r.workersLive}/${r.contextsLive}`,
    );
    expect(cycles).toBeGreaterThanOrEqual(8);
    // Each resume may underrun briefly; the cost must be bounded and must not
    // grow cycle over cycle.
    expect(Math.max(...resumeDeltas)).toBeLessThan(25);
    expect(resumeDeltas[resumeDeltas.length - 1]).toBeLessThanOrEqual(Math.max(...resumeDeltas.slice(0, 3)) + 5);
    expect(r.workersLive).toBeLessThanOrEqual(1);
    expect(r.contextsLive).toBeLessThanOrEqual(1);
  });
});
