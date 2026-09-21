// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// R4.4: Rust playback is the only product backend for the codecs it decodes,
// online and offline (OPFS). The frozen Emscripten decoder is retained only as
// a differential oracle, selected through the test-only session key. There is
// no product flag, URL parameter or automatic fallback.

import { test, expect, type Page } from '@playwright/test';
import { playerState, selectBackend, signIn, waitFor } from './helpers';

async function openAlbum(page: Page, title: string): Promise<void> {
  const shelf = page.getByRole('heading', { name: 'The shelf' });
  if (!(await shelf.isVisible().catch(() => false))) {
    await page.getByRole('link', { name: 'Albums' }).click();
    await expect(shelf).toBeVisible({ timeout: 15_000 });
  }
  const escaped = title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  await page.getByRole('link', { name: new RegExp(`^${escaped}`) }).first().click();
  await expect(page.getByRole('button', { name: 'Play album' })).toBeVisible({ timeout: 15_000 });
}

async function playAlbum(page: Page, title: string): Promise<void> {
  await openAlbum(page, title);
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: `${title} playing`, timeout: 40_000 });
}

async function lifecycle(page: Page, expectedKind: string): Promise<void> {
  await playAlbum(page, 'Long Player');
  expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe(expectedKind);
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.8, { label: 'advances', timeout: 20_000 });

  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
  const frozen = (await playerState(page)).positionSeconds;
  await page.waitForTimeout(400);
  expect(Math.abs((await playerState(page)).positionSeconds - frozen)).toBeLessThan(0.3);

  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
  await waitFor(page, async () => (await playerState(page)).positionSeconds > frozen + 0.2, { label: 'resume advances', timeout: 15_000 });

  await page.evaluate(() => window.__musicpack!.player.seek(12));
  await waitFor(page, async () => Math.abs((await playerState(page)).positionSeconds - 12) < 2, { label: 'seek', timeout: 15_000 });

  const before = (await playerState(page)).currentTitle;
  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(page, async () => (await playerState(page)).currentTitle !== before, { label: 'next', timeout: 20_000 });
  expect((await playerState(page)).error).toBeUndefined();
}

test.describe('R4.4 decoder lanes', () => {
  test.setTimeout(120_000);

  test('no override: Rust is the only product backend and drives PlayerController', async ({ page }) => {
    await signIn(page);
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
    await lifecycle(page, 'rust');
  });

  test('legacy decoder flags are inert: no product flag or URL override remains', async ({ page }) => {
    await page.goto('/?legacyPlayback=1&rustPlayback=0');
    await page.waitForFunction(() => Boolean(window.__musicpack));
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
  });

  test('the oracle lane drives the frozen decoder for differential runs', async ({ page }) => {
    // The test-only session key selects the frozen Emscripten decoder; it is
    // not a product setting (no UI, no URL parameter).
    await selectBackend(page, 'legacy');
    await signIn(page);
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('legacy');
    await lifecycle(page, 'musepack');
  });

  test('the oracle key is session-scoped: survives a reload, clears when removed', async ({ page }) => {
    await signIn(page);
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');

    await page.evaluate(() => sessionStorage.setItem('musicpack.oracle-decoder.v1', '1'));
    await page.reload();
    await page.waitForFunction(() => Boolean(window.__musicpack));
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('legacy');

    await page.evaluate(() => sessionStorage.removeItem('musicpack.oracle-decoder.v1'));
    await page.reload();
    await page.waitForFunction(() => Boolean(window.__musicpack));
    expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
  });

  test('Rust default: representation selection unchanged; FLAC alternate uses Rust', async ({ page }) => {
    await signIn(page);
    const items = await (async () => {
      await page.getByText('Shapeshifter').click();
      await page.getByRole('button', { name: 'Play album' }).click();
      await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
      return page.evaluate(() =>
        window.__musicpack!.queue.get().items.map((i) => ({ id: i.id, url: i.source.url, codec: i.codec })),
      );
    })();
    // Primary Musepack source, and Rust decodes it.
    expect(items[0]?.codec).toBe('musepack-sv8');
    expect(items[0]?.id).not.toContain('r');
    expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe('rust');

    // Lossless selects the FLAC representation; selection is unchanged and
    // Rust (which decodes FLAC) is the engine.
    await page.evaluate(() => window.__musicpack?.audioPreference.set({ mode: 'lossless' }));
    await page.getByRole('button', { name: 'Add album to queue' }).click();
    const appended = (
      await page.evaluate(() =>
        window.__musicpack!.queue.get().items.map((i) => ({ id: i.id, url: i.source.url, codec: i.codec })),
      )
    ).slice(items.length);
    expect(appended[0]?.id).toMatch(/r\d+$/);
    expect(appended[0]?.url).toContain('/representations/');
    expect(appended[0]?.codec).toBe('flac');
    await page.evaluate((i) => void window.__musicpack!.player.playQueueIndex(i), items.length);
    await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'flac playing', timeout: 25_000 });
    expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe('rust');
    expect((await playerState(page)).error).toBeUndefined();
  });
});
