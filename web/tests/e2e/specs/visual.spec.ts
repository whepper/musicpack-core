// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Visual regression baselines for the migrated web player (R2 §14).
//
// A small, representative set of screens — not every state: the shelf,
// album page, track page, search results, settings, queue, the signed-out
// auth gate, and the mobile shelf. Playwright disables animations and
// caret blinking for screenshots; the captured screens contain no
// wall-clock-dependent text.
//
// Baselines are PER-PLATFORM (font rendering differs across OSes). The
// committed set lives in visual.spec.ts-snapshots/ with the platform
// suffix Playwright appends (e.g. *-darwin.png). On a platform without
// committed baselines the suite SKIPS itself instead of failing —
// regenerate after intentional visual changes with:
//
//   npx playwright test visual --update-snapshots
//
// and review the image diffs in the PR (see web/README.md).

import { existsSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { env, signIn } from './helpers';

const here = path.dirname(fileURLToPath(import.meta.url));
const snapshotDir = path.join(here, 'visual.spec.ts-snapshots');
const platformSuffix = { darwin: 'darwin', linux: 'linux', win32: 'win32' }[process.platform] ?? process.platform;

const hasBaselinesForPlatform =
  existsSync(snapshotDir) &&
  readdirSync(snapshotDir).some((f) => f.endsWith(`-${platformSuffix}.png`));

// Locally a missing baseline is written on first run (Playwright's
// default `updateSnapshots: 'missing'`), which is how the first set is
// produced. Under CI a missing baseline would fail, so platforms without
// a committed set skip instead (see web/README.md for the re-cut flow).
test.skip(
  !hasBaselinesForPlatform && Boolean(process.env.CI),
  `no visual baselines committed for ${platformSuffix}`,
);

async function openFirstAlbum(page: import('@playwright/test').Page): Promise<string> {
  await page.goto('/');
  await page.locator('a[href^="/albums/"]').first().click();
  await page.waitForURL(/\/albums\//);
  return page.url();
}

test('shelf renders the collected albums', async ({ page }) => {
  await signIn(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible();
  await expect(page.locator('a[href^="/albums/"]').first()).toBeVisible();
  await expect(page).toHaveScreenshot('shelf.png', { fullPage: true });
});

test('album page (overview section)', async ({ page }) => {
  await signIn(page);
  await openFirstAlbum(page);
  await expect(page.getByRole('button', { name: 'Play album' })).toBeVisible();
  await expect(page).toHaveScreenshot('album.png', { fullPage: true });
});

test('track page', async ({ page }) => {
  await signIn(page);
  await openFirstAlbum(page);
  await page.getByRole('link', { name: /Tracks|tracks/ }).first().click();
  const trackLink = page.locator('a[href^="/tracks/"]').first();
  await trackLink.click();
  await page.waitForURL(/\/tracks\//);
  await expect(page.getByRole('button', { name: 'Play track' })).toBeVisible();
  await expect(page).toHaveScreenshot('track.png', { fullPage: true });
});

test('search results', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=long');
  await expect(page).toHaveURL(/\/search\?q=long/);
  await expect(page).toHaveScreenshot('search.png', { fullPage: true });
});

test('settings page', async ({ page }) => {
  await signIn(page);
  await page.goto('/settings');
  await expect(page.getByRole('heading', { name: 'Downloads & storage' })).toBeVisible();
  await expect(page).toHaveScreenshot('settings.png', {
    fullPage: true,
    // The storage estimate reports the browser's disk-dependent quota,
    // which changes between Chrome launches — mask the line.
    mask: [page.locator('p.muted', { hasText: / of .* used/ })],
  });
});

test('queue page', async ({ page }) => {
  await signIn(page);
  await openFirstAlbum(page);
  await page.getByRole('button', { name: 'Play album' }).click();
  await page.goto('/queue');
  await expect(page).toHaveScreenshot('queue.png', { fullPage: true });
});

test('signed-out auth gate', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveScreenshot('auth-gate.png', { fullPage: true });
});

test('mobile shelf navigation', async ({ page }) => {
  await signIn(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible();
  await expect(page.locator('a[href^="/albums/"]').first()).toBeVisible();
  await expect(page).toHaveScreenshot('mobile-shelf.png', { fullPage: false });
  void env; // server env document kept for future visual fixtures
});
