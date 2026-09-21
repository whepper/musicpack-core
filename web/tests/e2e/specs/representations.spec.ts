// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 4: representation selection, observed end-to-end. The "Shapeshifter"
// fixture album's first track carries a real 48 s FLAC alternate; the rest
// are Musepack-only. Assertions read queue items and backend state through
// the debug hook — behaviour, not internals.

import { test, expect } from '@playwright/test';
import { playerState, selectBackend, signIn, waitFor } from './helpers';

// Representation→engine mapping is asserted against the legacy engines, which
// are now the explicit escape hatch; Rust representation coverage lives in
// rust-default.spec.ts.
test.beforeEach(async ({ page }) => {
  await selectBackend(page, 'legacy');
  await signIn(page);
});

interface QueueItemView {
  id: string;
  url: string;
  codec: string | undefined;
}

async function queueItemViews(page: import('@playwright/test').Page): Promise<QueueItemView[]> {
  return page.evaluate(() =>
    window.__musicpack!.queue.get().items.map((i) => ({
      id: i.id,
      url: i.source.url,
      codec: i.codec,
    })),
  );
}

async function openShapeshifter(page: import('@playwright/test').Page): Promise<void> {
  await page.getByText('Shapeshifter').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });
}

test('default preference keeps represented tracks on their primary Musepack source', async ({
  page,
}) => {
  await openShapeshifter(page);

  const items = await queueItemViews(page);
  expect(items[0]?.codec).toBe('musepack-sv8');
  expect(items[0]?.url).not.toContain('/representations/');
  expect(items[0]?.url).toMatch(/\/audio$/);
  expect(items[0]?.id).toMatch(/^t\d+$/); // plain t{trackId}, no representation suffix
  // The demand-driven MUSEPACK engine is the thing serving bytes.
  const state = await playerState(page);
  expect(state.servedBytes).toBeGreaterThan(0);
});

test('lossless preference routes the represented track through the native backend and persists across reloads', async ({
  page,
}) => {
  // The preference now has a real UI: Settings → Playback quality.
  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.getByRole('radio', { name: /Prefer lossless/ }).check();
  expect(
    await page.evaluate(() => window.__musicpack?.audioPreference.get() ?? null),
  ).toEqual({ mode: 'lossless' });

  await page.reload(); // cookie session survives; so must the preference
  expect(
    await page.evaluate(() => window.__musicpack?.audioPreference.get() ?? null),
  ).toEqual({ mode: 'lossless' });
  // Still authenticated via the session cookie — back on the (reloaded)
  // Settings page; navigate to the shelf for the playback assertions.
  await page.getByRole('link', { name: 'Albums' }).click();
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible({
    timeout: 20_000,
  });

  await openShapeshifter(page);

  const items = await queueItemViews(page);
  expect(items[0]?.id).toMatch(/r\d+$/); // representation-aware identity
  expect(items[0]?.url).toContain('/representations/');
  expect(items[0]?.codec).toBe('flac');

  // The 8 s alternate plays through the native engine...
  await waitFor(
    page,
    async () =>
      (await page.evaluate(() => window.__musicpack?.player.getBackendKind())) === 'native',
    { label: 'native backend engaged' },
  );
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.5, {
    label: 'flac position advances',
  });
  expect((await playerState(page)).state).not.toBe('error');

  // ...and the boundary into the following MUSEPACK track recovers through
  // the core's fresh-load path (the browser cannot preload .mpc natively —
  // a failed standby must never kill the sounding track).
  await waitFor(
    page,
    async () => (await page.evaluate(() => window.__musicpack?.queue.get().index ?? -1)) >= 1,
    { label: 'boundary into musepack track 2', timeout: 30_000 },
  );
  await waitFor(
    page,
    async () =>
      (await page.evaluate(() => window.__musicpack?.player.getBackendKind())) === 'musepack',
    { label: 'engine switched for musepack track' },
  );
  // Priming the freshly built engine takes a moment; wait for audible
  // playback rather than sampling the transient buffering state.
  await waitFor(
    page,
    async () => {
      const s = await playerState(page);
      return s.state === 'playing' && s.error === undefined;
    },
    { label: 'musepack track playing after recovery', timeout: 20_000 },
  );
  expect((await playerState(page)).error).toBeUndefined();

  // Musepack-only tracks in the same library are untouched by the preference.
  await page.getByRole('link', { name: 'MusicPack home' }).click(); // back to the shelf
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'musepack playing',
  });
  const musepackItems = await queueItemViews(page);
  expect(musepackItems.every((i) => !i.id.includes('r'))).toBe(true);
  expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe('musepack');
});

test('changing the preference mid-playback never restarts the current item but applies to later construction', async ({
  page,
}) => {
  await openShapeshifter(page);

  const before = await queueItemViews(page);
  expect(before[0]?.id).not.toContain('r'); // built under the default pref

  await page.evaluate(() =>
    window.__musicpack?.audioPreference.set({ mode: 'lossless' }),
  );

  // Already-built items (including whatever is playing) are NOT rebuilt.
  const after = await queueItemViews(page);
  expect(after.slice(0, before.length)).toEqual(before);
  expect((await playerState(page)).state).not.toBe('error');

  // NEWLY built queue entries resolve under the new preference.
  await page.getByRole('button', { name: 'Add album to queue' }).click();
  const appended = (await queueItemViews(page)).slice(before.length);
  expect(appended.length).toBeGreaterThan(0);
  expect(appended[0]?.id).toMatch(/r\d+$/);
  expect(appended[0]?.url).toContain('/representations/');
});
