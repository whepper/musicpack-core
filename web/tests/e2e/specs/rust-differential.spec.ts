// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// R4.4: the Rust playback path, driven by the real PlayerController, compared
// against the frozen Emscripten decoder on the same scenarios. Rust is the
// only product backend; the frozen decoder is selected solely through the
// test-only oracle lane (session key), applied via `addInitScript` so the
// existing `signIn()` (which navigates to `/`) keeps the selection.

import { test, expect, devices, type Browser, type Page } from '@playwright/test';
import { env, signIn, playerState, waitFor } from './helpers';

const BACKENDS = ['legacy', 'rust'] as const;
type Backend = (typeof BACKENDS)[number];

async function selectBackend(page: Page, backend: Backend): Promise<void> {
  await page.addInitScript((b: string) => {
    try {
      if (b === 'legacy') sessionStorage.setItem('musicpack.oracle-decoder.v1', '1');
      else sessionStorage.removeItem('musicpack.oracle-decoder.v1');
    } catch {
      /* storage unavailable */
    }
  }, backend);
}

/** Sets a range slider value, firing the same input+change the UI listens to. */
async function setSeek(page: Page, seconds: number): Promise<void> {
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, seconds);
}

interface BasicResult {
  backend: string;
  kind: string | null;
  albumDurationSeconds: number;
  trackDurationSeconds: number;
  currentTitle: string | null;
  playingPosition: number;
  pausedPosition: number;
  pausedPosition2: number;
  seekedPosition: number;
  resumedPosition: number;
  nextTitle: string | null;
  error: string | undefined;
  timeToPlayingMs: number;
}

/** open -> play -> pause -> seek (paused) -> resume -> next; PlayerController only. */
async function basicScenario(page: Page): Promise<BasicResult> {
  const t0 = Date.now();
  await page.getByText('Long Player').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', {
    label: 'playing',
    timeout: 30_000,
  });
  const timeToPlayingMs = Date.now() - t0;
  await page.waitForTimeout(1200);
  const playing = await playerState(page);

  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
  const paused = await playerState(page);
  await page.waitForTimeout(300);
  const paused2 = await playerState(page);

  const td = paused.currentTrackDurationSeconds;
  await setSeek(page, Math.round(td * 0.5));
  await waitFor(
    page,
    async () => {
      const s = await playerState(page);
      return Math.abs(s.positionSeconds - s.currentTrackStartSeconds - td * 0.5) < 1.5;
    },
    { label: 'seek while paused', timeout: 15_000 },
  );
  const seeked = await playerState(page);

  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
  await waitFor(page, async () => (await playerState(page)).positionSeconds > seeked.positionSeconds + 0.2, {
    label: 'advances after resume',
    timeout: 15_000,
  });
  const resumed = await playerState(page);

  // next through the real controller (preload/advance path).
  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(
    page,
    async () => {
      const s = await playerState(page);
      return s.currentTitle !== null && s.currentTitle !== resumed.currentTitle;
    },
    { label: 'next track', timeout: 20_000 },
  );
  const afterNext = await playerState(page);

  return {
    backend: await page.evaluate(() => window.__musicpack?.playbackBackend ?? 'unknown'),
    kind: await page.evaluate(() => window.__musicpack?.player.getBackendKind() ?? null),
    albumDurationSeconds: playing.durationSeconds,
    trackDurationSeconds: playing.currentTrackDurationSeconds,
    currentTitle: playing.currentTitle,
    playingPosition: playing.positionSeconds,
    pausedPosition: paused.positionSeconds,
    pausedPosition2: paused2.positionSeconds,
    seekedPosition: seeked.positionSeconds,
    resumedPosition: resumed.positionSeconds,
    nextTitle: afterNext.currentTitle,
    error: playing.error ?? paused.error ?? seeked.error ?? afterNext.error,
    timeToPlayingMs,
  };
}

for (const backend of BACKENDS) {
  test.describe(`backend=${backend}`, () => {
    test.beforeEach(async ({ page }) => {
      await selectBackend(page, backend);
      await signIn(page);
      const identity = await page.evaluate(() => window.__musicpack?.playbackBackend);
      expect(identity).toBe(backend);
    });

    test('open/play/pause/resume/seek/next through PlayerController', async ({ page }) => {
      const r = await basicScenario(page);
      // eslint-disable-next-line no-console
      console.log(`differential[${backend}] ${JSON.stringify(r)}`);
      expect(r.backend).toBe(backend);
      expect(r.kind).toBe(backend === 'rust' ? 'rust' : 'musepack');
      expect(r.error).toBeUndefined();
      // Pause freezes the position.
      expect(Math.abs(r.pausedPosition2 - r.pausedPosition)).toBeLessThan(0.3);
      // Seek while paused lands near 50% of the track (track 1 start = 0) and
      // resume advances past it.
      expect(Math.abs(r.seekedPosition - r.trackDurationSeconds * 0.5)).toBeLessThan(1.5);
      expect(r.resumedPosition).toBeGreaterThan(r.seekedPosition);
      // next crossed to a different track.
      expect(r.nextTitle).not.toBe(r.currentTitle);
    });

    test('EOS, gapless transitions and queue advancement reach ended', async ({ page }) => {
      await page.getByText('Synthetic Test Compilation').click();
      await page.getByRole('button', { name: 'Play album' }).click();
      await waitFor(page, async () => (await playerState(page)).state === 'playing', {
        label: 'playing',
        timeout: 40_000,
      });
      await waitFor(page, async () => (await playerState(page)).state === 'ended', {
        label: 'ended',
        timeout: 60_000,
      });
      const s = await playerState(page);
      const q = await page.evaluate(() => window.__musicpack?.queue.get());
      expect(s.state).toBe('ended');
      expect(s.error).toBeUndefined();
      expect(q?.index).toBe((q?.items.length ?? 0) - 1);
    });

    test('representation selection is identical; Rust plays the chosen FLAC alternate', async ({
      page,
    }) => {
      await page.getByText('Shapeshifter').click();
      await page.getByRole('button', { name: 'Play album' }).click();
      await waitFor(page, async () => (await playerState(page)).state === 'playing', {
        label: 'playing',
      });
      const items = await page.evaluate(() =>
        window.__musicpack!.queue.get().items.map((i) => ({
          id: i.id,
          url: i.source.url,
          codec: i.codec,
        })),
      );
      // Default preference: the primary Musepack source, representation-free.
      expect(items[0]?.codec).toBe('musepack-sv8');
      expect(items[0]?.id).not.toContain('r');
      expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe(
        backend === 'rust' ? 'rust' : 'musepack',
      );

      // Lossless preference: the represented FLAC alternate is selected for
      // NEWLY built entries (selection is host policy, backend-independent).
      await page.evaluate(() => window.__musicpack?.audioPreference.set({ mode: 'lossless' }));
      await page.getByRole('button', { name: 'Add album to queue' }).click();
      const appended = (
        await page.evaluate(() =>
          window.__musicpack!.queue.get().items.map((i) => ({
            id: i.id,
            url: i.source.url,
            codec: i.codec,
          })),
        )
      ).slice(items.length);
      expect(appended[0]?.id).toMatch(/r\d+$/);
      expect(appended[0]?.url).toContain('/representations/');
      expect(appended[0]?.codec).toBe('flac');

      // Play the chosen FLAC representation; the backend kind is the only
      // difference between the two runs.
      const index = items.length;
      await page.evaluate((i) => void window.__musicpack!.player.playQueueIndex(i), index);
      await waitFor(page, async () => (await playerState(page)).state === 'playing', {
        label: 'flac alternate playing',
        timeout: 25_000,
      });
      expect(await page.evaluate(() => window.__musicpack?.player.getBackendKind())).toBe(
        backend === 'rust' ? 'rust' : 'native',
      );
      expect((await playerState(page)).error).toBeUndefined();
    });

    test('cancellation: next/stop during the initial fetch stays coherent', async ({ page }) => {
      await page.getByText('Long Player').first().click();
      await page.getByRole('button', { name: 'Play album' }).click();
      // Cancel the in-flight open with a next, then stop. The controller must
      // supersede cleanly rather than wedge or double-play.
      await page.evaluate(() => void window.__musicpack!.player.next());
      await page.waitForTimeout(400);
      await page.evaluate(() => window.__musicpack!.player.stop());
      await waitFor(page, async () => (await playerState(page)).state === 'idle', {
        label: 'idle after stop',
        timeout: 20_000,
      });
      expect((await playerState(page)).error).toBeUndefined();
    });
  });
}

test('rust: crossfade/Sweet Fade advances through the boundary without error', {
  timeout: 120_000,
}, async ({ page }) => {
  test.setTimeout(150_000);
  await selectBackend(page, 'rust');
  await signIn(page);
  await page.getByText('Fade Rider').first().click();
  await page.evaluate(() => (window.__musicpack!.player as unknown as { setCrossfade(s: number): void }).setCrossfade(12));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Spy the live engine's crossfade attempt (same technique as the legacy
  // musepack crossfade test).
  await page.evaluate(() => {
    const eng = (window.__musicpack!.player as unknown as { core: { engine: { beginCrossfade: (...a: unknown[]) => Promise<unknown> } } }).core.engine;
    const orig = eng.beginCrossfade.bind(eng);
    (window as unknown as { __xf: string }).__xf = 'none';
    eng.beginCrossfade = async (...args: unknown[]) => {
      const r = await orig(...args);
      (window as unknown as { __xf: string }).__xf = r ? 'taken' : 'declined';
      return r;
    };
  });

  const firstTitle = (await playerState(page)).currentTitle;
  await page.evaluate(() => (window.__musicpack!.player as unknown as { seek(s: number): void }).seek(40.5));

  let fadeResult = 'none';
  let advanced = false;
  for (let i = 0; i < 100 && !(fadeResult === 'taken' && advanced); i++) {
    await page.waitForTimeout(250);
    const s = await playerState(page);
    if (s.state === 'error') throw new Error(`player errored: ${s.error}`);
    if (s.currentTitle && s.currentTitle !== firstTitle) advanced = true;
    fadeResult = await page.evaluate(() => (window as unknown as { __xf?: string }).__xf ?? 'none');
  }
  expect(fadeResult).toBe('taken');
  expect(advanced).toBe(true);
});

test('decoder lanes: Rust is the only product backend; the frozen decoder is oracle-only', async ({
  page,
}) => {
  // No override -> Rust.
  await page.goto('/');
  await page.waitForFunction(() => Boolean(window.__musicpack));
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
  // The historical URL flag is inert (removed in R4.4).
  await page.goto('/?legacyPlayback=1');
  await page.waitForFunction(() => Boolean(window.__musicpack));
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('rust');
  // The test-only oracle key selects the frozen decoder.
  await page.evaluate(() => sessionStorage.setItem('musicpack.oracle-decoder.v1', '1'));
  await page.reload();
  await page.waitForFunction(() => Boolean(window.__musicpack));
  expect(await page.evaluate(() => window.__musicpack?.playbackBackend)).toBe('legacy');
});

test('differential: the same scenario through both backends', async ({ browser }) => {
  async function run(backend: Backend): Promise<BasicResult> {
    // Mirror the project's `use` options: a bare newContext() omits the
    // Desktop Chrome device emulation and changes click/user-activation
    // behaviour for AudioContext.resume().
    const context = await browser.newContext({ ...devices['Desktop Chrome'], baseURL: env.baseUrl });
    try {
      const page = await context.newPage();
      await selectBackend(page, backend);
      await signIn(page);
      return await basicScenario(page);
    } finally {
      await context.close();
    }
  }
  const legacy = await run('legacy');
  const rust = await run('rust');
  // eslint-disable-next-line no-console
  console.log(`differential[legacy] ${JSON.stringify(legacy)}`);
  // eslint-disable-next-line no-console
  console.log(`differential[rust] ${JSON.stringify(rust)}`);

  expect(legacy.backend).toBe('legacy');
  expect(rust.backend).toBe('rust');
  expect(legacy.kind).toBe('musepack');
  expect(rust.kind).toBe('rust');

  // Deterministic: same fixture, same geometry, same identity.
  expect(rust.albumDurationSeconds).toBeCloseTo(legacy.albumDurationSeconds, 2);
  expect(rust.trackDurationSeconds).toBeCloseTo(legacy.trackDurationSeconds, 2);
  expect(rust.currentTitle).toBe(legacy.currentTitle);
  expect(rust.error).toBeUndefined();
  expect(legacy.error).toBeUndefined();
  // `next` left the current track on both paths. Which short fixture track it
  // lands on is scheduling-dependent, so only "changed" is asserted here.
  expect(rust.nextTitle).not.toBe(rust.currentTitle);
  expect(legacy.nextTitle).not.toBe(legacy.currentTitle);

  // Scheduling-dependent: pause freeze and seek landing use explicit tolerances.
  expect(Math.abs(rust.pausedPosition - rust.pausedPosition2)).toBeLessThan(0.3);
  expect(Math.abs(legacy.pausedPosition - legacy.pausedPosition2)).toBeLessThan(0.3);
  const seekDelta = Math.abs(rust.seekedPosition - legacy.seekedPosition);
  expect(seekDelta).toBeLessThan(2);
  // Resume must advance past the seek target on both paths (no stale audio).
  expect(rust.resumedPosition).toBeGreaterThan(rust.seekedPosition);
  expect(legacy.resumedPosition).toBeGreaterThan(legacy.seekedPosition);
});
