// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Collection search journeys through the USER-FACING flow: typing on
// /search must run the query and render grouped results without a
// remount (the URL→state effect path), clearing must return to the
// prompt, and a no-hit term must announce itself. The top-bar field must
// submit into the same page. (The shipped bug: `term` was $derived over a
// plain `let`, so typing updated the input but never the results region.)

import { test, expect } from '@playwright/test';
import { signIn, waitFor } from './helpers';

async function pageText(page: import('@playwright/test').Page) {
  return page.evaluate(() => document.body.innerText);
}

test.describe('collection search', () => {
  test('typing on the search page runs the query live', async ({ page }) => {
    await signIn(page);
    await page.getByRole('link', { name: 'Search' }).click();
    await expect(page.getByRole('heading', { name: 'Search' })).toBeVisible();

    const field = page.getByLabel('Search the collection');
    await field.fill('long');
    await expect(page).toHaveURL(/\/search\?q=long/);
    await waitFor(
      page,
      async () => {
        const t = await pageText(page);
        return /Long Player/.test(t) && !/Type to search/.test(t);
      },
      { label: 'typed query renders results' },
    );

    // No-hit terms announce themselves rather than reverting to the prompt.
    await field.fill('qqzz');
    await waitFor(
      page,
      async () => /Nothing in the collection matches/.test(await pageText(page)),
      { label: 'no-hit term announced' },
    );

    // Clearing returns to the prompt state.
    await field.fill('');
    await waitFor(
      page,
      async () => /Type to search/.test(await pageText(page)),
      { label: 'clear returns to prompt' },
    );
  });

  test('top-bar search submits into the search page', async ({ page }) => {
    await signIn(page);
    await page.getByLabel('Search MusicPack').fill('long');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/\/search\?q=long/);
    await waitFor(
      page,
      async () => /Long Player/.test(await pageText(page)),
      { label: 'top-bar search results' },
    );
    await expect(page.getByLabel('Search the collection')).toHaveValue('long');
  });
});
