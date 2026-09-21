// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// End-to-end coverage for the waveform-backed seek control. Tests that the
// fixed reference fixtures (test-musicpack-album.mpack carries envelopes,
// test-flac-album.mpack does not) drive the correct UI in each case.

import { test, expect } from '@playwright/test';
import { signIn, playerState, waitFor } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('waveform canvas renders for a track with envelope; click seeks within the track', async ({ page }) => {
  // Long Player carries envelopes AND a long first track (~39 s decoded),
  // leaving decode/EOF headroom that the ~1 s TestComp tracks lack: on
  // those, an in-flight gapless handoff could move the cursor right after
  // pausing and break the same-track assertion.
  await page.getByText('Long Player').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();

  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // The waveform canvas must replace the legacy linear range on a
  // waveform-bearing track.
  await expect(page.locator('.playerbar canvas').first()).toBeVisible();

  // The hidden <input type=range> sibling remains the keyboard/a11y surface;
  // it is track-scoped, so its max reflects the per-track duration.
  const st = await playerState(page);
  expect(st.currentTrackDurationSeconds).toBeGreaterThan(0);
  const rangeMax = await page.locator('.playerbar input[type=range]').first().getAttribute('max');
  expect(parseFloat(rangeMax ?? '0')).toBeGreaterThan(0);

  // Pause first: the position must be stable to assert exact targeting.
  // Go through the controller, not the Pause button: the fixture album is
  // short and can reach 'ended' before we get here, at which point the
  // button would read "Play" again. Pausing also sets pauseIntent, so the
  // seek below will not resume playback.
  await page.evaluate(() => void window.__musicpack?.player.pause());
  await waitFor(page, async () => {
    const s = await playerState(page);
    return s.state === 'paused' || s.state === 'ended';
  }, { label: 'paused' });
  const paused = await playerState(page);
  expect(paused.currentTrackDurationSeconds).toBeGreaterThan(0);

  // Click on the canvas at ~25% -> must land ~25% into THIS track, not
  // 25% into the album. A click mapped against the album duration would
  // jump to another track entirely (the multi-file seek bug).
  const canvas = page.locator('.playerbar canvas').first();
  const box = await canvas.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.click(box!.x + box!.width * 0.25, box!.y + box!.height / 2);

  const target = paused.currentTrackStartSeconds + paused.currentTrackDurationSeconds * 0.25;
  await waitFor(
    page,
    async () => Math.abs((await playerState(page)).positionSeconds - target) < 1.0,
    { label: 'click seeks to ~25% of the current track' },
  );
  // The seek must not have crossed into another track.
  expect((await playerState(page)).currentTitle).toBe(paused.currentTitle);
});

test('focus on the hidden range draws the focus ring on the waveform container', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Focus the (hidden) range via the keyboard: the canvas container must
  // receive the :focus-within styling. We don't assert visual style; we
  // verify the focus + keyboard semantics by pressing ArrowRight and
  // observing position advance.
  const range = page.locator('.playerbar input[type=range]').first();
  await range.focus();
  const before = (await playerState(page)).positionSeconds;
  await page.keyboard.press('ArrowRight');
  await waitFor(page, async () => (await playerState(page)).positionSeconds > before,
              { label: 'ArrowRight advances position' });
});

test('no-waveform tracks fall back to the linear range', async ({ page }) => {
  // The Classical Compilation has no waveform envelopes -> the legacy
  // <input type=range> stays visible, no canvas.
  await page.getByText('Synthetic Classical Compilation').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();

  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // No canvas: the fallback range is the only seek control.
  await expect(page.locator('.playerbar canvas')).toHaveCount(0);
  await expect(page.locator('.playerbar input[type=range]').first()).toBeVisible();

  const dur = (await playerState(page)).durationSeconds;
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, Math.round(dur * 0.5));
  await waitFor(page, async () => (await playerState(page)).positionSeconds > dur * 0.3,
              { label: 'fallback range seeks' });
});