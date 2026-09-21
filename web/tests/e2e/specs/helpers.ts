import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, type Page } from '@playwright/test';

interface ServerEnv {
  token: string;
  baseUrl: string;
  libdir: string;
}

const here = path.dirname(fileURLToPath(import.meta.url));

export const env: ServerEnv = JSON.parse(
  readFileSync(path.join(here, '..', '.server-env.json'), 'utf8'),
);

/** Selects the decoder lane for a differential/oracle run.
 *
 *  `'rust'` is the only product backend. `'legacy'` selects the frozen
 *  Emscripten Musepack decoder through the app's **test-only** oracle key
 *  (session-scoped so it survives the reloads some specs perform); there is
 *  no product flag or URL parameter any more (R4.4). */
export async function selectBackend(page: Page, backend: 'rust' | 'legacy'): Promise<void> {
  await page.addInitScript((b: string) => {
    try {
      if (b === 'legacy') sessionStorage.setItem('musicpack.oracle-decoder.v1', '1');
      else sessionStorage.removeItem('musicpack.oracle-decoder.v1');
    } catch {
      /* storage unavailable */
    }
  }, backend);
}

export async function signIn(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByLabel('Server token').fill(env.token);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'The shelf' })).toBeVisible({ timeout: 20_000 });
}

/** Reads the controller state through the debug hook. */
export async function playerState(page: Page): Promise<{
  state: string;
  positionSeconds: number;
  durationSeconds: number;
  currentTrackStartSeconds: number;
  currentTrackDurationSeconds: number;
  currentTitle: string | null;
  servedBytes: number;
  normDb: number;
  error: string | undefined;
}> {
  return page.evaluate(() => {
    const p = window.__musicpack?.player;
    const m = p?.model.get();
    return {
      state: m?.state ?? 'idle',
      positionSeconds: m?.positionSeconds ?? 0,
      durationSeconds: m?.durationSeconds ?? 0,
      currentTrackStartSeconds: m?.currentTrackStartSeconds ?? 0,
      currentTrackDurationSeconds: m?.currentTrackDurationSeconds ?? 0,
      currentTitle: m?.current?.track.title ?? null,
      servedBytes: p?.getServedBytes() ?? 0,
      normDb: m?.normDb ?? 0,
      error: m?.error,
    };
  });
}

export async function waitFor(
  page: Page,
  fn: () => Promise<boolean>,
  opts: { timeout?: number; label?: string } = {},
): Promise<void> {
  const { timeout = 15_000, label = 'condition' } = opts;
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await fn()) return;
    await page.waitForTimeout(120);
  }
  throw new Error(`timed out waiting for ${label}`);
}
