// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Accessibility smoke tests (R2 §13): the keyboard paths and overlay
// behavior that the baseline depends on stay operable. These are
// behavioral checks, not a full audit — the structural baseline is
// documented in web/README.md (semantic controls, focus-visible tokens,
// reduced-motion) and enforced by review + Svelte's a11y lints.

import { expect, test, type Page } from '@playwright/test';
import { env } from './helpers';

/** Signs in only when the auth gate is showing, waiting for whichever
 *  renders first (gate or shelf). The app boots asynchronously, so an
 *  instant `isVisible()` check races the gate under parallel workers. */
async function signInIfGateShown(page: Page): Promise<void> {
  const token = page.getByLabel('Server token');
  const card = page.locator('a[href^="/albums/"]').first();
  await token.or(card).first().waitFor({ state: 'visible', timeout: 20_000 });
  if (await token.isVisible()) {
    await token.fill(env.token);
    await page.getByRole('button', { name: 'Sign in' }).click();
  }
  await expect(card).toBeVisible({ timeout: 20_000 });
}

test('sign-in form is fully keyboard operable', async ({ page }) => {
  await page.goto('/');
  const token = page.getByLabel('Server token');
  await token.focus();
  await page.keyboard.type(env.token);
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible({
    timeout: 20_000,
  });
});

test('album cards are focusable and activate with Enter', async ({ page }) => {
  await page.goto('/');
  await signInIfGateShown(page);
  const card = page.locator('a[href^="/albums/"]').first();
  await card.focus();
  await expect(card).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/albums\//);
});

test('primary playback button activates with Enter', async ({ page }) => {
  await page.goto('/');
  await signInIfGateShown(page);
  const card = page.locator('a[href^="/albums/"]').first();
  await card.click();
  const play = page.getByRole('button', { name: 'Play album' });
  await expect(play).toBeVisible();
  await play.focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('.playerbar')).toBeVisible({ timeout: 20_000 });
});

test('artwork dialog is modal-labelled and closes on Escape', async ({ page }) => {
  await page.goto('/');
  await signInIfGateShown(page);
  const card = page.locator('a[href^="/albums/"]').first();
  await card.click();
  await page.getByRole('button', { name: 'View artwork', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Artwork viewer' });
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAttribute('aria-modal', 'true');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
});
