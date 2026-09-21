import { test, expect } from '@playwright/test';
import { selectBackend, signIn, playerState, waitFor } from './helpers';

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('plays a native-codec (FLAC) track through the legacy browser backend', async ({ page }) => {
  // Native (HTMLMediaElement) playback is now reached via the explicit legacy
  // escape hatch; Rust is the default and decodes FLAC itself.
  await selectBackend(page, 'legacy');
  await page.reload();
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible({ timeout: 20_000 });
  await page.getByText('Synthetic Classical Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 0.5, { label: 'position advances' });

  const kind = await page.evaluate(() => window.__musicpack?.player.getBackendKind());
  expect(kind).toBe('native');
  const state = await playerState(page);
  expect(state.state).not.toBe('error');
});

test('exposes Media Session metadata and a track-relative position state', async ({ page }) => {
  // Capture setPositionState calls (write-only API -> wrap it before app boot).
  await page.addInitScript(() => {
    const w = window as unknown as Record<string, unknown>;
    w.__posStates = [];
    const ms = navigator.mediaSession;
    if (!ms) return;
    const orig = ms.setPositionState?.bind(ms);
    Object.defineProperty(ms, 'setPositionState', {
      configurable: true,
      value: (s: MediaPositionState) => {
        (w.__posStates as MediaPositionState[]).push(s);
        orig?.(s);
      },
    });
  });
  // Init scripts apply to the NEXT document; boot a wrapped one.
  await page.reload();
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible({ timeout: 20_000 });
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  const meta = await page.evaluate(() => {
    const m = navigator.mediaSession?.metadata;
    return m ? { title: m.title, artist: m.artist, album: m.album } : null;
  });
  expect(meta?.title).toBeTruthy();
  expect(meta?.album).toBe('Synthetic Test Compilation');
  expect(meta?.artist).toBe('Alphaville');

  // The reported position state must describe the CURRENT TRACK, not an
  // album-spanning timeline: OS scrubbers derive their seekTo times from
  // it, and album-scale values send media-key seeks to the wrong place.
  const last = await page.evaluate(() => {
    const arr = (window as unknown as { __posStates?: MediaPositionState[] }).__posStates ?? [];
    return arr[arr.length - 1] ?? null;
  });
  expect(last).not.toBeNull();
  const st = await playerState(page);
  expect(last!.duration).toBeGreaterThan(0);
  expect(last!.duration).toBeLessThan(st.durationSeconds); // track < album
  expect(last!.position).toBeGreaterThanOrEqual(0);
  expect(last!.position).toBeLessThanOrEqual(last!.duration + 0.25);
});

test('nav-bar navigation keeps playback alive (SPA, no full reload)', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Marker that a full document reload would wipe.
  await page.evaluate(() => {
    (window as unknown as Record<string, unknown>).__mpAlive = 1;
  });

  await page.getByRole('link', { name: 'Artists' }).click();
  await waitFor(page, async () => page.url().includes('/artists'), { label: '/artists reached' });

  await page.getByRole('link', { name: 'Artists' }).click();
  await waitFor(page, async () => page.url().includes('/artists'), { label: '/artists reached' });

  expect(await page.evaluate(() => (window as unknown as Record<string, unknown>).__mpAlive)).toBe(1);
  const n = await page.evaluate(() => window.__musicpack?.queue.get().items.length ?? 0);
  expect(n).toBeGreaterThan(0);
  expect((await playerState(page)).currentTitle).toBeTruthy();

  // The nav underline must follow the route (it used to freeze on Albums).
  // Scope to the nav landmark: artist-page links like "Alphaville 3 albums"
  // substring-match the role name 'Albums'.
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Artists' })).toHaveAttribute('aria-current', 'page');
  await expect(nav.getByRole('link', { name: 'Albums' })).not.toHaveAttribute('aria-current', 'page');
  await nav.getByRole('link', { name: 'Now Playing' }).click();
  await waitFor(page, async () => page.url().includes('/queue'), { label: '/queue reached' });
  await expect(page.getByRole('link', { name: 'Now Playing' })).toHaveAttribute('aria-current', 'page');
});

test('an invalid session returns to the sign-in screen (re-auth)', async ({ page }) => {
  // Poison the HttpOnly cookie and load the app — the boot session probe 401s
  // and the UI falls back to the sign-in screen (no raw error strings).
  await page.context().addCookies([
    { name: 'musicpack_session', value: 'definitely-not-a-session', domain: '127.0.0.1', path: '/' },
  ]);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible({ timeout: 20_000 });
  await expect(page.getByLabel('Server token')).toBeVisible();
});

test('a missing album shows a friendly error, not an internal string', async ({ page }) => {
  // beforeEach signs in; now an unknown album id must surface a friendly error.
  await page.goto('/albums/999999');
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeHidden();
  await expect(page.getByText('no longer in the collection', { exact: false }).first()).toBeVisible({ timeout: 20_000 });
});

test.describe('mobile player layout (<=680px)', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('transport buttons share one row in the mobile player', async ({ page }) => {
    // The global beforeEach already signed in at this viewport.
    await page.getByText('Long Player').first().click();
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

    // .mobile-player .row children: thumb, title button, prev, play, next.
    // The Next button must sit on the SAME row as Play (it used to wrap
    // onto an implicit second grid row).
    const boxes = await page.evaluate(() => {
      const btns = [...document.querySelectorAll('.mobile-player .row button')];
      const pick = (el: Element | undefined) => {
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.x, y: r.y + r.height / 2 };
      };
      return {
        prev: pick(btns[1]),
        play: pick(btns[2]),
        next: pick(btns[3]),
        count: btns.length,
      };
    });
    expect(boxes.count).toBe(4);
    expect(boxes.prev).not.toBeNull();
    expect(boxes.play).not.toBeNull();
    expect(boxes.next).not.toBeNull();
    expect(Math.abs(boxes.next!.y - boxes.play!.y)).toBeLessThanOrEqual(2);
    expect(boxes.next!.x).toBeGreaterThan(boxes.play!.x);
    expect(Math.abs(boxes.prev!.y - boxes.play!.y)).toBeLessThanOrEqual(2);
  });
});
