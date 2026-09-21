// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-4.1: targeted verification of Rust range-error propagation.
// A failed range fetch must surface as an explicit player error, never leave
// the controller stuck in `loading`, never fall back to legacy, clean up its
// generation, and allow a successful replay afterwards.

import { test, expect, type Page, type Route } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

/** Test-only controllable range-request failure injection. */
class AudioGate {
  calls = 0;
  failFirst = false;
  failFrom = Number.POSITIVE_INFINITY; // 1-based request index
  failAll = false;

  async handle(route: Route): Promise<void> {
    this.calls += 1;
    const fail = this.failAll || (this.failFirst && this.calls === 1) || this.calls >= this.failFrom;
    if (fail) await route.abort('failed').catch(() => undefined);
    else await route.continue().catch(() => undefined);
  }
}

async function setup(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const counts = { workersLive: 0, contextsLive: 0, nodesCreated: 0 };
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
    const RealNode = window.AudioWorkletNode;
    if (RealNode) {
      const N = function (this: unknown, ...a: unknown[]) {
        counts.nodesCreated += 1;
        return new (RealNode as unknown as new (...x: unknown[]) => AudioWorkletNode)(...a);
      } as unknown as typeof AudioWorkletNode;
      N.prototype = RealNode.prototype;
      window.AudioWorkletNode = N;
    }
  });
  await signIn(page);
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
}

async function resources(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => (window as unknown as { __res: Record<string, number> }).__res);
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

/** Asserts the failure produced an explicit error (not a stuck load), the
 *  backend identity is unchanged, and resources are bounded. */
async function assertErroredNotStuck(page: Page, label: string): Promise<void> {
  await waitFor(page, async () => (await playerState(page)).state === 'error', {
    label: `${label}: error state`,
    timeout: 30_000,
  });
  const s = await playerState(page);
  const kind = await page.evaluate(() => window.__musicpack?.player.getBackendKind());
  const res = await resources(page);
  // eslint-disable-next-line no-console
  console.log(`errverify[${label}] state=${s.state} kind=${kind} error=${s.error} workersLive=${res.workersLive}`);
  expect(s.state).not.toBe('loading');
  expect(s.error).toBeTruthy();
  expect(kind).toBe('rust'); // no silent legacy fallback
  expect(res.workersLive).toBeLessThanOrEqual(1);
}

const AUDIO = '**/audio*';

test.describe('Rust range-error propagation (14H-4.1)', () => {
  test.setTimeout(120_000);

  test('failed first range request during open: error + recovery', async ({ page }) => {
    await setup(page);
    const gate = new AudioGate();
    gate.failFirst = true;
    await page.route(AUDIO, (route) => gate.handle(route));

    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await assertErroredNotStuck(page, 'open-first');

    // Recovery: restore the network, dispose, replay; playback must advance.
    gate.failFirst = false;
    await page.evaluate(() => window.__musicpack!.player.teardown());
    await playAlbum(page, 'Long Player');
    await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.5, {
      label: 'recovered playback advances',
      timeout: 20_000,
    });
    expect((await playerState(page)).error).toBeUndefined();
    await page.unroute(AUDIO);
  });

  test('failed request during load/priming (after open): error', async ({ page }) => {
    await setup(page);
    const gate = new AudioGate();
    gate.failFrom = 3; // allow the header/first blocks, fail later reads
    await page.route(AUDIO, (route) => gate.handle(route));

    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await assertErroredNotStuck(page, 'priming');
    await page.unroute(AUDIO);
  });

  test('failed request during a seek: error + recovery', async ({ page }) => {
    await setup(page);
    const gate = new AudioGate();
    await page.route(AUDIO, (route) => gate.handle(route));
    await playAlbum(page, 'Long Player');
    const callsBefore = gate.calls;
    gate.failAll = true;
    // Seek far into the 48 s opener so the read crosses a not-yet-cached block.
    await page.evaluate(() => void window.__musicpack!.player.seek(45));
    await page.waitForTimeout(1500);
    // eslint-disable-next-line no-console
    console.log(`errverify[seek-probe] callsBefore=${callsBefore} callsAfter=${gate.calls} state=${(await playerState(page)).state}`);
    await assertErroredNotStuck(page, 'seek');
    gate.failAll = false;
    await page.evaluate(() => window.__musicpack!.player.teardown());
    await playAlbum(page, 'Long Player');
    expect((await playerState(page)).error).toBeUndefined();
    await page.unroute(AUDIO);
  });

  test('failed standby prepare does not leave the player stuck loading', async ({ page }) => {
    await setup(page);
    const gate = new AudioGate();
    await page.route(AUDIO, (route) => gate.handle(route));
    await playAlbum(page, 'Long Player');
    gate.failAll = true;
    await page.evaluate(() => void window.__musicpack!.player.next());
    await page.waitForTimeout(3000);
    const s = await playerState(page);
    // eslint-disable-next-line no-console
    console.log(`errverify[standby] state=${s.state} error=${s.error}`);
    // The essential invariant: never stuck in `loading`. A standby failure may
    // either be non-fatal (the sounding track continues) or surface an error;
    // it must not wedge.
    expect(s.state).not.toBe('loading');
    if (s.state === 'error') {
      gate.failAll = false;
      await page.evaluate(() => window.__musicpack!.player.teardown());
      await playAlbum(page, 'Long Player');
      expect((await playerState(page)).error).toBeUndefined();
    }
    await page.unroute(AUDIO);
  });

  test('failed requests for a whole open: error, bounded resources, no stale output', async ({ page }) => {
    await setup(page);
    const gate = new AudioGate();
    gate.failAll = true;
    await page.route(AUDIO, (route) => gate.handle(route));
    await openAlbum(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await assertErroredNotStuck(page, 'open-all');
    // Release and replay; the old generation must not resurface.
    gate.failAll = false;
    await page.evaluate(() => window.__musicpack!.player.teardown());
    await playAlbum(page, 'Long Player');
    const before = (await playerState(page)).positionSeconds;
    await waitFor(page, async () => (await playerState(page)).positionSeconds > before + 0.4, {
      label: 'fresh generation advances',
      timeout: 20_000,
    });
    expect((await playerState(page)).error).toBeUndefined();
    expect((await resources(page)).workersLive).toBeLessThanOrEqual(1);
    await page.unroute(AUDIO);
  });
});
