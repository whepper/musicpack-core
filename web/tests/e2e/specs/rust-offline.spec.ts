// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// R4.4: offline (OPFS) playback through the Rust/WASM engine.
//
// Proves the frozen Emscripten decoder is no longer on the offline path: the
// committed OPFS bytes are served to the *same* Rust decoder through its
// synchronous range callback, and the engine kind is `rust` — never
// `musepack`. Network is severed for the playback assertions, so working
// playback can only come from OPFS.
//
// Flow: install → go offline → play → assert rust + OPFS bytes → seek →
// queue across tracks → reload → play restored session.

import { test, expect, type Page } from '@playwright/test';
import { playerState, signIn, waitFor } from './helpers';

test.describe.configure({ mode: 'serial' });

const OFFLINE_PREFIX = /^\/api\/v1\/tracks\/\d+\/audio$/;

async function queueItems(page: Page) {
  return page.evaluate(() =>
    window.__musicpack!.queue.get().items.map((i) => ({
      id: i.id,
      kind: i.source.kind,
      url: i.source.url,
      codec: i.codec,
    })),
  );
}

async function backendKind(page: Page): Promise<string | null> {
  return page.evaluate(() => window.__musicpack?.player.getBackendKind() ?? null);
}

async function openAlbum(page: Page, title: string): Promise<void> {
  await page.getByText(title).first().click();
  await expect(page.getByRole('heading', { name: title })).toBeVisible({ timeout: 15_000 });
}

async function installViaUi(page: Page, title: string): Promise<void> {
  await page.getByRole('button', { name: new RegExp(`^Download ${title} for offline`) }).click();
  await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });
}

async function setSeek(page: Page, seconds: number): Promise<void> {
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, seconds);
}

test.describe('offline playback on the Rust engine', () => {
  test('OPFS Musepack plays through Rust, seeks and advances with the network severed', async ({
    page,
    context,
  }) => {
    await signIn(page);
    await openAlbum(page, 'Long Player');
    await installViaUi(page, 'Long Player');

    // ---- network severed: only OPFS can serve the bytes -----------------
    await context.setOffline(true);
    const audioRequests: string[] = [];
    page.on('request', (r) => {
      const path = new URL(r.url()).pathname;
      if (OFFLINE_PREFIX.test(path)) audioRequests.push(path);
    });

    await page.getByRole('button', { name: 'Play album' }).click();
    const playStarted = Date.now();
    let last: Record<string, unknown> | null = null;
    try {
      await waitFor(
        page,
        async () => {
          last = await page.evaluate(() => {
            const m = window.__musicpack?.player?.model?.get();
            const items = window.__musicpack?.queue?.get?.().items ?? [];
            return {
              state: m?.state,
              error: m?.error,
              kind: window.__musicpack?.player?.getBackendKind() ?? null,
              source: m?.current?.source?.kind ?? null,
              url: m?.current?.source?.url ?? null,
              index: window.__musicpack?.queue?.get?.().index,
              sources: items.map((i) => i.source?.kind),
            };
          });
          return last?.state === 'playing';
        },
        { label: 'offline rust playback', timeout: 30_000 },
      );
    } catch {
      throw new Error(`offline rust playback | state: ${JSON.stringify(last)}`);
    }
    // eslint-disable-next-line no-console
    console.log(`offline-rust[play] timeToPlayingMs=${Date.now() - playStarted}`);
    expect(Date.now() - playStarted).toBeLessThan(15_000);
    // The decoder is Rust; the source is the committed OPFS file.
    expect(await backendKind(page)).toBe('rust');
    expect((await queueItems(page))[0]?.kind).toBe('local-file');

    await waitFor(page, async () => (await playerState(page)).positionSeconds > 1, {
      label: 'offline position advances',
      timeout: 20_000,
    });
    // Bytes were consumed from the local source (counted by the worker's
    // range accounting), and no audio was fetched over the network.
    expect(await page.evaluate(() => window.__musicpack!.player.getServedBytes())).toBeGreaterThan(0);
    expect(audioRequests).toEqual([]);
    expect((await playerState(page)).error).toBeUndefined();

    // ---- seek over OPFS bytes (random access) ---------------------------
    const seekStarted = Date.now();
    await setSeek(page, 20);
    await waitFor(
      page,
      async () => {
        const s = await playerState(page);
        return s.state === 'playing' && Math.abs(s.positionSeconds - 20) < 2;
      },
      { label: 'offline seek lands', timeout: 20_000 },
    );
    // eslint-disable-next-line no-console
    console.log(`offline-rust[seek] latencyMs=${Date.now() - seekStarted}`);
    expect(await backendKind(page)).toBe('rust');
    expect((await playerState(page)).error).toBeUndefined();

    // ---- queue across tracks --------------------------------------------
    const before = (await playerState(page)).currentTitle;
    await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
    await waitFor(page, async () => (await playerState(page)).currentTitle !== before, {
      label: 'offline next track',
      timeout: 20_000,
    });
    expect(await backendKind(page)).toBe('rust');
    expect((await queueItems(page))[1]?.kind).toBe('local-file');
    expect((await playerState(page)).error).toBeUndefined();
  });

  test('OPFS persistence: reload keeps the installed session playable offline', async ({
    page,
    context,
  }) => {
    await signIn(page);
    await openAlbum(page, 'Long Player');
    await installViaUi(page, 'Long Player');
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'online local playback',
    });
    await context.setOffline(true);
    await page.reload();
    // Offline shell: no sign-in, but installed content is playable.
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeHidden({ timeout: 20_000 });
    await page.locator('.playerbar').getByRole('button', { name: 'Play', exact: true }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'offline playback after reload',
      timeout: 30_000,
    });
    expect(await backendKind(page)).toBe('rust');
    expect((await queueItems(page)).find((i) => i.kind === 'local-file')).toBeTruthy();
    await waitFor(page, async () => (await playerState(page)).positionSeconds > 1, {
      label: 'restored offline position advances',
    });
    expect((await playerState(page)).error).toBeUndefined();
  });

  test('full vertical: a Rust-authored package installs and plays offline through Rust', async ({
    page,
    context,
  }) => {
    // The "Rust Authored" fixture is built by the e2e harness with the Rust
    // authoring pipeline (musicpack-author) and served by the Rust server, so
    // this exercises Author -> Server -> offline planner -> OPFS -> Rust/WASM.
    await signIn(page);
    await openAlbum(page, 'Rust Authored');
    await installViaUi(page, 'Rust Authored');
    await context.setOffline(true);

    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'rust-authored offline playback',
      timeout: 30_000,
    });
    expect(await backendKind(page)).toBe('rust');
    expect((await queueItems(page))[0]?.kind).toBe('local-file');
    expect((await playerState(page)).currentTitle).toBe('Rust One');
    expect((await playerState(page)).error).toBeUndefined();
  });

  test('offline crossfade: an overlapped boundary advances through the Rust mixer', {
    timeout: 150_000,
  }, async ({ page, context }) => {
    await signIn(page);
    await openAlbum(page, 'Fade Rider');
    await installViaUi(page, 'Fade Rider');
    await context.setOffline(true);

    await page.evaluate(() =>
      (window.__musicpack!.player as unknown as { setCrossfade(s: number): void }).setCrossfade(12),
    );
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'offline fade rider playing',
      timeout: 30_000,
    });
    expect(await backendKind(page)).toBe('rust');

    // Spy the live engine's crossfade attempt (same technique as the online
    // crossfade tests).
    await page.evaluate(() => {
      const eng = (
        window.__musicpack!.player as unknown as {
          core: { engine: { beginCrossfade: (...a: unknown[]) => Promise<unknown> } };
        }
      ).core.engine;
      const orig = eng.beginCrossfade.bind(eng);
      (window as unknown as { __xf: string }).__xf = 'none';
      eng.beginCrossfade = async (...args: unknown[]) => {
        const r = await orig(...args);
        (window as unknown as { __xf: string }).__xf = r ? 'taken' : 'declined';
        return r;
      };
    });

    const firstTitle = (await playerState(page)).currentTitle;
    await page.evaluate(() =>
      (window.__musicpack!.player as unknown as { seek(s: number): void }).seek(40.5),
    );

    let fadeResult = 'none';
    let advanced = false;
    for (let i = 0; i < 120 && !(fadeResult === 'taken' && advanced); i++) {
      await page.waitForTimeout(250);
      const s = await playerState(page);
      if (s.state === 'error') throw new Error(`offline crossfade errored: ${s.error}`);
      if (s.currentTitle && s.currentTitle !== firstTitle) advanced = true;
      fadeResult = await page.evaluate(() => (window as unknown as { __xf?: string }).__xf ?? 'none');
    }
    expect(fadeResult).toBe('taken');
    expect(advanced).toBe(true);
    expect(await backendKind(page)).toBe('rust');
  });
});
