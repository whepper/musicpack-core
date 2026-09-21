// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Stage 1 vertical-slice coverage (Phase D): genres, album actions, codec
// badge, now-playing indication, multi-disc grouping, mobile layout. These
// tests validate library behavior, not the M8 crossfade — every test
// disables crossfade up front.

import { test, expect, type Page } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
  // Deterministic library tests: M8 crossfade must be off.
  await page.evaluate(() => {
    window.__musicpack?.player.setCrossfade(0);
  });
});

async function queueSnapshot(page: Page) {
  return page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    return {
      index: q?.index ?? -1,
      length: q?.items.length ?? -1,
      titles: q?.items.map((i) => i.track.title) ?? [],
      current: window.__musicpack?.player.model.get().current?.track.title ?? null,
    };
  });
}

test('Play Album starts playback and stays on the album page', async ({ page }) => {
  await page.getByText('Long Player').click();
  await expect(page.getByRole('heading', { name: 'Long Player' })).toBeVisible();

  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });

  expect(page.url()).toContain('/albums/');
  expect(page.url()).not.toContain('/queue');
  await expect(page.getByRole('heading', { name: 'Long Player' })).toBeVisible();
});

test('Shuffle Album starts on a valid album track without navigating', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Shuffle' }).click();

  // The queue index is set synchronously by playAlbum, so read it before
  // the ~1 s fixture tracks can advance.
  const snap = await queueSnapshot(page);
  expect(snap.length).toBe(4);
  expect(snap.index).toBeGreaterThanOrEqual(0);
  expect(snap.index).toBeLessThan(4);

  expect(page.url()).toContain('/albums/');
  expect(page.url()).not.toContain('/queue');
});

test('Add Album to Queue appends without replacing current playback', async ({ page }) => {
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', {
    label: 'paused',
  });
  const before = await queueSnapshot(page);
  expect(before.current).toBeTruthy();

  // Browse to another album and append it. The feedback assertion uses a
  // structural locator on purpose: a name-based getByRole would stop
  // matching while the confirmation label is active.
  await page.getByRole('link', { name: 'Albums' }).click();
  await page.getByText('Synthetic Test Compilation').click();
  await expect(page.getByRole('heading', { name: 'Synthetic Test Compilation' })).toBeVisible();

  const add = page.locator('button.album-add');
  await add.click();
  await expect(add).toHaveAttribute('aria-label', 'Album added to queue');
  await expect(add).toContainText('✓ Added');
  await expect(add).toHaveAttribute('aria-label', 'Add album to queue');

  const after = await queueSnapshot(page);
  expect(after.length).toBe(before.length + 4);
  expect(after.titles.slice(before.length)).toEqual([
    'Alphaville - Big in Japan',
    'Bleachers - The Van',
    'Synthwave - Night Drive',
    'Test Artist - Fourth Track',
  ]);
  expect(after.index).toBe(before.index);
  expect(after.current).toBe(before.current);
});

test('per-track Add to Queue appends that track and confirms transiently', async ({ page }) => {
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', {
    label: 'paused',
  });
  const before = await queueSnapshot(page);

  const secondRowAdd = page.locator('.queue-add').nth(1);
  const label = await secondRowAdd.getAttribute('aria-label');
  expect(label).toMatch(/^Add .+ to queue$/);
  const trackTitle = (label ?? '').replace(/^Add /, '').replace(/ to queue$/, '');

  await secondRowAdd.click();
  // The button flips to "✓" while the addition is fresh…
  await expect(secondRowAdd).toHaveText('✓');
  // …and reverts once the confirmation window passes.
  await waitFor(
    page,
    async () => (await secondRowAdd.textContent()) === '+',
    { label: 'add button reverted', timeout: 4000 },
  );

  const after = await queueSnapshot(page);
  expect(after.length).toBe(before.length + 1);
  expect(after.titles.at(-1)).toBe(trackTitle);
  expect(after.index).toBe(before.index);
  expect(after.current).toBe(before.current);
});

test('codec badge renders and the playing track is semantically marked', async ({ page }) => {
  await page.getByText('Long Player').click();

  const badges = page.locator('.track .codec');
  await expect(badges.first()).toHaveText('MPC');

  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(
    page,
    async () => !!(await page.evaluate(() => window.__musicpack?.player.model.get().current)),
    { label: 'current item set' },
  );

  const marked = page.locator('.tracklist .track[aria-current="true"]');
  await expect(marked).toHaveCount(1);
  // The indication is not color-only: aria-current carries the state, and
  // the row also exposes the now-playing accent class for styling.
  await expect(marked.first()).toHaveClass(/now-playing/);
});

test('genre pills: capped with +N on cards, full list on detail, absent when empty', async ({
  page,
}) => {
  // Card: max three pills plus an overflow indicator.
  const longCard = page.locator('.album-card', { hasText: 'Long Player' });
  await expect(longCard.locator('.genre-pill')).toHaveCount(4);
  await expect(longCard.locator('.genre-pill').nth(0)).toHaveText('Rock');
  await expect(longCard.locator('.genre-pill').nth(2)).toHaveText('Synthwave');
  await expect(longCard.locator('.genre-pill').nth(3)).toHaveText('+1');

  // Detail: every genre is listed below the artist line.
  await longCard.click();
  await expect(page.getByRole('heading', { name: 'Long Player' })).toBeVisible();
  const detailPills = page.locator('.album-heading .genre-pill');
  await expect(detailPills).toHaveCount(4);
  await expect(detailPills.nth(3)).toHaveText('Pop');
  // Pills are passive metadata: plain spans, not links or buttons.
  const pillTag = await detailPills.first().evaluate((el) => el.tagName.toLowerCase());
  expect(pillTag).toBe('span');

  // Albums without genres render no genre UI at all.
  await page.getByRole('link', { name: 'Albums' }).click();
  const plainCard = page.locator('.album-card', { hasText: 'Fade Rider' });
  await expect(plainCard.locator('.genre-pills')).toHaveCount(0);
  await plainCard.click();
  await expect(page.getByRole('heading', { name: 'Fade Rider' })).toBeVisible();
  await expect(page.locator('.album-heading .genre-pills')).toHaveCount(0);
});

test('multi-disc albums group tracks per disc in display and semantics', async ({ page }) => {
  await page.getByText('Two Disc Extravaganza').click();
  await expect(page.getByRole('heading', { name: 'Disc 1' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Disc 2' })).toBeVisible();

  const lists = page.locator('.disc-tracks');
  await expect(lists).toHaveCount(2);
  await expect(lists.nth(0)).toHaveAccessibleName('Disc 1 tracks');
  await expect(lists.nth(1)).toHaveAccessibleName('Disc 2 tracks');
  await expect(lists.nth(0).locator('.track')).toHaveCount(4);
  await expect(lists.nth(1).locator('.track')).toHaveCount(2);
});

test('mobile viewport: hero actions wrap instead of overflowing', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByText('Long Player').click();
  await expect(page.getByRole('heading', { name: 'Long Player' })).toBeVisible();

  const hero = page.locator('.album-heading .hero-actions');
  await expect(hero).toBeVisible();
  const layout = await hero.evaluate((el) => ({
    wrap: getComputedStyle(el).flexWrap,
    overflowX: el.scrollWidth - el.clientWidth,
  }));
  expect(layout.wrap).toBe('wrap');
  expect(layout.overflowX).toBeLessThanOrEqual(1);

  // Pills wrap within the heading rather than pushing the page wide.
  const pills = page.locator('.album-heading .genre-pills');
  const pillOverflow = await pills.evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(pillOverflow).toBeLessThanOrEqual(1);

  // Every track row keeps its queue button reachable at this width.
  const adds = page.locator('.queue-add');
  const count = await adds.count();
  expect(count).toBeGreaterThan(0);
  for (let i = 0; i < count; i++) {
    await expect(adds.nth(i)).toBeVisible();
  }
});

test('offline affordances: ⤓ badge on installed albums and "Available offline" shelf filter', async ({
  page,
}) => {
  // Install Long Player through the UI, then verify both affordances.
  await page.getByText('Long Player').click();
  await expect(page.locator('.dl-control')).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: /^Download Long Player for offline/ }).click();
  await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

  // Back to the shelf: the installed album shows the offline badge...
  await page.getByRole('link', { name: 'MusicPack home' }).click();
  const card = page.getByRole('listitem').filter({ hasText: 'Long Player' });
  await expect(card.locator('.offline-badge')).toBeVisible();

  // ...and the filter narrows the grid to exactly that album.
  await page.getByRole('button', { name: 'Available offline' }).click();
  await expect(page.locator('.shelf-grid [role="listitem"]')).toHaveCount(1);
  await expect(page.locator('.shelf-grid').getByText('Long Player')).toBeVisible();
});
