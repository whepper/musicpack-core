// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-4.2: output-rate compatibility matrix for the Rust backend.
//
// The AudioContext rate is forced (test-only) so the same scenarios run at
// 44100/48000/96000 Hz. The invariant: PlayerController time/duration/position
// math must use the actual Rust output rate — no 44100 fallback.

import { test, expect, type Page, type Route } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

async function setUp(page: Page, rate: number): Promise<void> {
  await page.addInitScript((r: number) => {
    const Real = window.AudioContext;
    if (Real) {
      const Wrapped = function (this: unknown, ...args: unknown[]) {
        const opts = args[0] && typeof args[0] === 'object' ? (args[0] as AudioContextOptions) : {};
        return new (Real as unknown as new (o?: AudioContextOptions) => AudioContext)({ ...opts, sampleRate: r });
      } as unknown as typeof AudioContext;
      Wrapped.prototype = Real.prototype;
      window.AudioContext = Wrapped;
    }
  }, rate);
  await signIn(page);
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
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

async function playAlbum(page: Page, title: string, timeout = 40_000): Promise<void> {
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

interface MatrixResult {
  engineRate: number | null;
  trackDuration: number;
  albumDuration: number;
  seekPos: number;
  seekTarget: number;
  pausedFroze: boolean;
  resumedPos: number;
  nextTitle: string | null;
  error: string | undefined;
}

async function runMatrix(page: Page): Promise<MatrixResult> {
  await playAlbum(page, 'Long Player');
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.8, { label: 'advances', timeout: 20_000 });
  const engineRate = await page.evaluate(() => {
    const eng = (window.__musicpack?.player as unknown as { core: { engine: { rate?: number } } }).core.engine;
    return typeof eng?.rate === 'number' ? eng.rate : null;
  });
  const playing = await playerState(page);

  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
  const p1 = (await playerState(page)).positionSeconds;
  await page.waitForTimeout(400);
  const p2 = (await playerState(page)).positionSeconds;
  const pausedFroze = Math.abs(p2 - p1) < 0.3;

  const target = playing.currentTrackDurationSeconds * 0.5;
  await setSeek(page, target);
  await waitFor(page, async () => Math.abs((await playerState(page)).positionSeconds - target) < 1.5, {
    label: 'seek 50%',
    timeout: 15_000,
  });
  const seekPos = (await playerState(page)).positionSeconds;

  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
  await waitFor(page, async () => (await playerState(page)).positionSeconds > seekPos + 0.2, {
    label: 'resume advances',
    timeout: 15_000,
  });
  const resumedPos = (await playerState(page)).positionSeconds;

  const before = (await playerState(page)).currentTitle;
  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(page, async () => (await playerState(page)).currentTitle !== before, { label: 'next', timeout: 20_000 });
  const nextTitle = (await playerState(page)).currentTitle;

  await page.evaluate(() => window.__musicpack!.player.stop());
  await waitFor(page, async () => (await playerState(page)).state === 'idle', { label: 'idle' });

  return {
    engineRate,
    trackDuration: playing.currentTrackDurationSeconds,
    albumDuration: playing.durationSeconds,
    seekPos,
    seekTarget: target,
    pausedFroze,
    resumedPos,
    nextTitle,
    error: playing.error,
  };
}

for (const rate of [44_100, 48_000, 96_000]) {
  test(`output rate ${rate} Hz: rate/duration/position/seek/pause/resume/next`, async ({ page }) => {
    test.setTimeout(120_000);
    await setUp(page, rate);
    const r = await runMatrix(page);
    // eslint-disable-next-line no-console
    console.log(`rate[${rate}] ${JSON.stringify(r)}`);
    expect(r.engineRate).toBe(rate);
    expect(r.error).toBeUndefined();
    // ~48 s opener, ~51 s album, at every output rate.
    expect(Math.abs(r.trackDuration - 48)).toBeLessThan(0.7);
    expect(Math.abs(r.albumDuration - 51)).toBeLessThan(1.5);
    expect(Math.abs(r.seekPos - r.seekTarget)).toBeLessThan(1.5);
    expect(r.pausedFroze).toBe(true);
    expect(r.resumedPos).toBeGreaterThan(r.seekPos);
    expect(r.nextTitle).toBeTruthy();
  });
}

test('48 kHz regression: 48 s track, 50% seek -> 24.0 s (not ~52.24 s duration)', async ({ page }) => {
  test.setTimeout(120_000);
  await setUp(page, 48_000);
  const r = await runMatrix(page);
  // eslint-disable-next-line no-console
  console.log(`rate[48000-regression] ${JSON.stringify(r)}`);
  expect(r.engineRate).toBe(48_000);
  // The 14H-4.1 defect inflated duration to ~52.24 s and seek to ~26 s.
  expect(Math.abs(r.trackDuration - 48)).toBeLessThan(0.3);
  expect(Math.abs(r.albumDuration - 51)).toBeLessThan(1);
  expect(Math.abs(r.seekPos - 24)).toBeLessThan(1);
  expect(Math.abs(r.seekTarget - 24)).toBeLessThan(0.3);
});

/** Test-only range failure injection with request accounting. */
class Gate {
  calls = 0;
  failAll = false;
  async handle(route: Route): Promise<void> {
    this.calls += 1;
    if (this.failAll) await route.abort('failed').catch(() => undefined);
    else await route.continue().catch(() => undefined);
  }
}

for (const rate of [44_100, 48_000]) {
  test(`failed seek at ${rate} Hz: bounded requests, prompt error, clean recovery`, async ({ page }) => {
    test.setTimeout(120_000);
    await setUp(page, rate);
    const gate = new Gate();
    await page.route('**/audio*', (route) => gate.handle(route));
    await playAlbum(page, 'Long Player');
    const callsBefore = gate.calls;
    gate.failAll = true;
    await page.evaluate(() => void window.__musicpack!.player.seek(45)); // far, uncached block
    await waitFor(page, async () => (await playerState(page)).state === 'error', { label: 'error', timeout: 20_000 });
    const callsDuring = gate.calls - callsBefore;
    const s = await playerState(page);
    // eslint-disable-next-line no-console
    console.log(`rate[failseek ${rate}] callsDuring=${callsDuring} state=${s.state} error=${s.error}`);
    // The 14H-4.1 storm issued thousands; the fix must keep it tiny.
    expect(callsDuring).toBeLessThan(10);
    expect(s.error).toBeTruthy();
    expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe('rust');

    // Clean recovery on a fresh generation.
    gate.failAll = false;
    await page.evaluate(() => window.__musicpack!.player.teardown());
    await playAlbum(page, 'Long Player');
    await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.5, { label: 'recovered', timeout: 20_000 });
    expect((await playerState(page)).error).toBeUndefined();
    await page.unroute('**/audio*');
  });
}

test('48 kHz crossfade: bounded dwell, duration unchanged, no EOS error', {
  timeout: 120_000,
}, async ({ page }) => {
  test.setTimeout(150_000);
  await setUp(page, 48_000);
  await openAlbum(page, 'Fade Rider');
  await page.evaluate(() => (window.__musicpack!.player as unknown as { setCrossfade(s: number): void }).setCrossfade(4));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  const duration = (await playerState(page)).durationSeconds;
  // eslint-disable-next-line no-console
  console.log(`rate[crossfade48] albumDuration=${duration}`);
  // Two 48 s tracks, no overlap yet: ~96 s.
  expect(Math.abs(duration - 96)).toBeLessThan(2);

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
    if (s.currentTitle && s.currentTitle !== first) advanced = true;
    result = await page.evaluate(() => (window as unknown as { __xf?: string }).__xf ?? 'none');
  }
  // eslint-disable-next-line no-console
  console.log(`rate[crossfade48] result=${result} advanced=${advanced} maxDwellMs=${maxDwellMs}`);
  expect(result).toBe('taken');
  expect(advanced).toBe(true);
  // Dwell stays bounded and does not stack across the transition.
  expect(maxDwellMs).toBeLessThan(9_000);
  expect((await playerState(page)).error).toBeUndefined();
});
