// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Synchronized-lyrics end-to-end (R3.4, playback tier — real audio): the
// track page's lyrics panel follows the playing position (spec §13), a
// seek recomputes the active line, and a lyric-fetch failure never stops
// playback (§7 playback isolation). Mirrors playback.spec.ts conventions
// (legacy engine, the deterministic escape-hatch path the suite tests).

import { test, expect } from '@playwright/test';
import { selectBackend, signIn, playerState, waitFor } from './helpers';

test.beforeEach(async ({ page }) => {
  await selectBackend(page, 'legacy');
  await signIn(page);
});

/** Sets a range slider value, firing the same input+change the UI listens to. */
async function setSeek(page: import('@playwright/test').Page, seconds: number): Promise<void> {
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, seconds);
}

/** Opens the track-1 detail page of the Lyric Shelf album. */
async function openLyricTrack(page: import('@playwright/test').Page): Promise<void> {
  await page.getByText('Lyric Shelf').first().click();
  await expect(page.getByRole('heading', { name: 'Lyric Shelf' })).toBeVisible();
  const row = page.locator('.tracklist .track').first();
  await row.locator('.track-detail').click();
  await expect(page.getByRole('heading', { name: /Big in Japan/ })).toBeVisible();
}

test('synced lyrics highlight the active line and recompute on seek', async ({ page }) => {
  await openLyricTrack(page);
  await page.getByRole('button', { name: 'Play track' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });

  const panel = page.locator('.lyrics-section');
  // Before the first timestamp (1 s): no active line (§9).
  await expect(panel.locator('.lyric-line[aria-current="step"]')).toHaveCount(0);

  // Past the first timestamp the first line becomes active and is exposed
  // via aria-current="step" (§12 accessibility contract).
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 1.4, {
    label: 'position passes the first timestamp',
  });
  const active = panel.locator('.lyric-line[aria-current="step"]');
  await expect(active).toHaveText('First line appears here');

  // Seek deep into the document: the active line recomputes (no
  // hysteresis) and follow tracks the new line.
  await setSeek(page, 21);
  await expect(panel.locator('.lyric-line[aria-current="step"]')).toHaveText(
    'Fourth line holds to the end',
    { timeout: 15_000 },
  );
});

test('a failed lyric request never breaks playback', async ({ page }) => {
  // Only the lyric bytes are blocked; audio streams from /tracks/{id}/audio.
  await page.route('**/api/v1/assets/**', (route) => route.abort());
  await openLyricTrack(page);
  await page.getByRole('button', { name: 'Play track' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
  });

  // The panel shows the degrade state…
  const panel = page.locator('.lyrics-section');
  await expect(panel.getByRole('status')).toContainText('Lyrics could not be loaded');
  // …and playback is completely unaffected: position advances.
  const before = await playerState(page);
  await waitFor(page, async () => (await playerState(page)).positionSeconds > before.positionSeconds + 0.5, {
    label: 'audio keeps advancing',
  });
  expect((await playerState(page)).state).toBe('playing');
});
