// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// End-to-end coverage for the P4 track-detail surface (/tracks/:id): direct
// navigation, hero + editorial inspector facts, playback, waveform, and the
// cross-links back to the owning album/edition/package sections.

import { test, expect, type Page } from '@playwright/test';
import { signIn, waitFor, playerState } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

// Read the raw (pre-CSS-transform) title of the first track of an album and
// navigate to its detail page via the Overview list's detail affordance.
async function openFirstTrack(
  page: Page,
  album: string,
): Promise<{ title: string }> {
  await page.getByText(album).first().click();
  await expect(page.getByRole('heading', { name: album })).toBeVisible();
  const row = page.locator('.tracklist .track').first();
  const title = (await row.locator('.tt').innerText()).trim();
  await row.locator('.track-detail').click();
  return { title };
}

test('track detail opens from the album and renders the editorial hero', async ({ page }) => {
  const { title } = await openFirstTrack(page, 'Synthetic Test Compilation');

  // Deep-linked directly to /tracks/:id with the track title as the heading.
  await expect(page.getByRole('heading', { name: title })).toBeVisible();
  expect(page.url()).toContain('/tracks/');

  // Album context is present in the hero eyebrow (the album link).
  await expect(page.locator('.album-heading .eyebrow a')).toHaveText(/Synthetic Test Compilation/);

  // Playback actions are present.
  await expect(page.getByRole('button', { name: 'Play track' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Shuffle album' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add track to queue' })).toBeVisible();
});

test('direct navigation to /tracks/:id works and links back to the album', async ({ page }) => {
  const { title } = await openFirstTrack(page, 'Synthetic Test Compilation');
  const id = page.url().split('/tracks/')[1]?.split(/[?#]/)[0];
  await page.goto(`/tracks/${id}`);
  await expect(page.getByRole('heading', { name: title })).toBeVisible();

  // The hero album link returns to the album (edition preserved).
  await page.locator('.album-heading .eyebrow a').click();
  await expect(page.getByRole('heading', { name: 'Synthetic Test Compilation' })).toBeVisible();
  expect(page.url()).toContain('/albums/');
});

test('track detail exposes real technical facts and no invented values', async ({ page }) => {
  const { title } = await openFirstTrack(page, 'Synthetic Test Compilation');
  await expect(page.getByRole('heading', { name: title })).toBeVisible();

  // Real codec facts (never bitrate/bit-depth). Small-caps labels read back
  // uppercased, so compare case-insensitively.
  const body = await page.locator('main').innerText();
  expect(body).toMatch(/sample rate/i);
  expect(body).toMatch(/stream version/i);

  // Loudness is real BS.1770 data.
  expect(body).toMatch(/LUFS/);
  expect(body).toMatch(/dBTP/);

  // Representation and SHA-256 references are present.
  await expect(page.getByText('Representations', { exact: true })).toBeVisible();
  await expect(page.locator('main')).toContainText(/SHA-256/i);

  // Nothing invented: no bitrate/bit-depth/LRA anywhere on the page.
  expect(body).not.toMatch(/kbps|Mbps|LRA|bit depth|16-bit|24-bit/i);
});

test('playback starts from the track page', async ({ page }) => {
  // Long Player's first track is ~48 s — long enough that the assertion runs
  // before the gapless handoff advances to the next ~1 s fixture track.
  const { title } = await openFirstTrack(page, 'Long Player');
  await page.getByRole('button', { name: 'Play track' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  expect((await playerState(page)).currentTitle).toBe(title);
});

test('the audio / package sections deep-link from the track inspector', async ({ page }) => {
  const { title } = await openFirstTrack(page, 'Synthetic Test Compilation');
  await expect(page.getByRole('heading', { name: title })).toBeVisible();

  // The "full Package section" affordance jumps to the album ?section=package.
  await page.getByRole('button', { name: 'full Package section' }).click();
  await expect(page.getByRole('heading', { name: 'Synthetic Test Compilation' })).toBeVisible();
  expect(page.url()).toContain('section=package');
});
