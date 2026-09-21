// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Lyrics UI end-to-end (R3.4, ui tier — no audio timing): the track page
// renders the lyrics panel from track detail's `lyrics[]`, bytes flow
// through the existing /api/v1/assets/{id} endpoint, selection follows the
// server's first-entry order, absence leaves no residue, and a failed
// fetch degrades to a retryable error state without breaking the page.
// The synchronized-follow behavior lives in lyrics-playback.spec.ts.

import { test, expect, type Page } from '@playwright/test';
import { signIn } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

/** Opens the Lyric Shelf album and clicks through to the detail page of
 *  the track at `rowIndex` (the album overview list order). */
async function openTrack(page: Page, rowIndex: number): Promise<string> {
  await page.getByText('Lyric Shelf').first().click();
  await expect(page.getByRole('heading', { name: 'Lyric Shelf' })).toBeVisible();
  const row = page.locator('.tracklist .track').nth(rowIndex);
  const title = (await row.locator('.tt').innerText()).trim();
  await row.locator('.track-detail').click();
  await expect(page.getByRole('heading', { name: title })).toBeVisible();
  return title;
}

test('a lyric-bearing track renders the panel from track detail bytes', async ({ page }) => {
  // The lyric document must arrive through the existing asset endpoint.
  const assetRequest = page.waitForRequest((r) =>
    /\/api\/v1\/assets\/\d+$/.test(new URL(r.url()).pathname),
  );
  await openTrack(page, 0); // track 1 — synced LRC (lang en)
  await assetRequest;

  const panel = page.locator('.lyrics-section');
  await expect(panel.getByRole('heading', { name: /Lyrics/ })).toBeVisible();
  // Parsed through the wasm core: metadata tags ([ti:], [ar:]) never
  // render as content; the four timed lines do.
  await expect(panel.locator('.lyric-line')).toHaveCount(4);
  await expect(panel.getByText('First line appears here')).toBeVisible();
  await expect(panel.getByText('Big in Japan')).toHaveCount(0); // metadata, not a line
  // Lang badge from the selected reference (server-supplied metadata).
  await expect(panel.locator('.lyrics-lang')).toHaveText('en');
  // No playback: the document shows without an active line (§9 — before
  // the first timestamp / no position feed).
  await expect(panel.locator('.lyric-line[aria-current="step"]')).toHaveCount(0);
});

test('selection follows the server order: first reference wins', async ({ page }) => {
  await openTrack(page, 1); // track 2 — refs [en, fr]
  const panel = page.locator('.lyrics-section');
  await expect(panel.locator('.lyric-line')).toHaveText([
    'Words without timing one',
    'Words without timing two',
  ]);
  await expect(panel.locator('.lyrics-lang')).toHaveText('en');
  // The fr document (second entry) is never fetched for rendering.
  const frCalls = [];
  page.on('request', (r) => {
    if (r.url().includes('02-the-van.fr')) frCalls.push(r.url());
  });
  expect(frCalls).toEqual([]);
});

test('a track without lyrics shows no lyrics panel', async ({ page }) => {
  await openTrack(page, 2); // track 3 — no lyric references
  await expect(page.locator('.lyrics-section')).toHaveCount(0);
  // The page is otherwise intact (hero + sections render).
  await expect(page.getByRole('button', { name: 'Play track' })).toBeVisible();
});

test('navigating from a lyric track to a plain track leaves no residue', async ({ page }) => {
  await openTrack(page, 0);
  await expect(page.locator('.lyrics-section .lyric-line').first()).toBeVisible();
  // Back to the album, then into the lyric-less track.
  await page.locator('.album-heading .eyebrow a').click();
  await expect(page.getByRole('heading', { name: 'Lyric Shelf' })).toBeVisible();
  const row = page.locator('.tracklist .track').nth(2);
  const title = (await row.locator('.tt').innerText()).trim();
  await row.locator('.track-detail').click();
  await expect(page.getByRole('heading', { name: title })).toBeVisible();
  await expect(page.locator('.lyrics-section')).toHaveCount(0);
});

test('a failed lyric fetch degrades to a retryable error, page intact', async ({ page }) => {
  await page.route('**/api/v1/assets/**', (route) => route.abort());
  await openTrack(page, 0);
  const panel = page.locator('.lyrics-section');
  await expect(panel.getByRole('status')).toContainText('Lyrics could not be loaded');
  // The rest of the page is unaffected (auxiliary content, §7).
  await expect(page.getByRole('button', { name: 'Play track' })).toBeVisible();

  // Retry with the network restored renders the document — no reload.
  await page.unroute('**/api/v1/assets/**');
  await panel.getByRole('button', { name: 'Retry' }).click();
  await expect(panel.locator('.lyric-line')).toHaveCount(4);
});
