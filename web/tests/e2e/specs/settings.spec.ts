// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Settings page: playback-quality preference UI + downloads/storage panel.
// PWA-lite: the manifest the shell service worker precaches is served.

import { test, expect } from '@playwright/test';
import { signIn } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('settings exposes the audio preference and persists it', async ({ page }) => {
  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();

  // Automatic is the default.
  await expect(page.getByRole('radio', { name: /Automatic/ })).toBeChecked();

  // Choose lossless; the underlying persisted store must reflect it.
  await page.getByRole('radio', { name: /Prefer lossless/ }).check();
  expect(await page.evaluate(() => window.__musicpack?.audioPreference.get())).toEqual({
    mode: 'lossless',
  });

  // And it survives a reload (localStorage-backed).
  await page.reload();
  await expect(page.getByRole('radio', { name: /Prefer lossless/ })).toBeChecked();
});

test('storage panel lists downloaded albums with sizes and remove action', async ({ page }) => {
  // Download Long Player via its album page first.
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: /^Download Long Player for offline/ }).click();
  await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

  await page.getByRole('link', { name: 'Settings' }).click();
  const row = page.getByRole('listitem').filter({ hasText: 'Long Player' });
  await expect(row).toBeVisible();
  await expect(row).toContainText(/\d+/); // size figure present

  // Remove from settings; row disappears.
  await row.getByRole('button', { name: /Remove Long Player download/ }).click();
  await expect(row).toBeHidden();
});

test('PWA-lite: manifest and icons are served as the shell expects', async ({ request }) => {
  const manifest = await request.get('/manifest.json');
  expect(manifest.ok()).toBeTruthy();
  expect(manifest.headers()['content-type']).toContain('application/json');
  const body = await manifest.json();
  expect(body.name).toBe('MusicPack');
  expect(body.icons.length).toBeGreaterThan(0);

  for (const icon of body.icons) {
    const res = await request.get(icon.src);
    expect(res.ok()).toBeTruthy();
    expect(res.headers()['content-type']).toContain('image/png');
  }
});
