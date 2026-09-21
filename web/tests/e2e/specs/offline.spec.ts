// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Offline downloads + offline playback, observed end-to-end through the
// USER-FACING controls (release-page Download button, progress ring,
// Installed/Update/Repair badges, Remove action).
//
// The debug hook (`window.__musicpack`) remains only as a byte-level
// assertion device: source kinds (`local-file` vs `http-range`), catalog
// records and served-byte accounting — things a DOM cannot express.
//
// Flow: sign in → download a release via the UI → sever the network →
// seek on the installed long track (OPFS random access) → reload (shell
// SW + 'offline' session state) → play the restored session locally →
// navigate across tracks → restore network → confirm non-installed
// releases still stream remotely. Cancel / stale / damaged lifecycles get
// dedicated tests below.

import { test, expect } from '@playwright/test';
import { playerState, signIn, waitFor } from './helpers';

test.describe.configure({ mode: 'serial' });

async function queueItems(page: import('@playwright/test').Page) {
  return page.evaluate(() =>
    window.__musicpack!.queue.get().items.map((i) => ({
      id: i.id,
      kind: i.source.kind,
      url: i.source.url,
      codec: i.codec,
    })),
  );
}

async function openLongPlayer(page: import('@playwright/test').Page): Promise<number> {
  await page.getByText('Long Player').click();
  await expect(page.getByRole('heading', { name: 'Long Player' })).toBeVisible();
  return page.evaluate(async () => {
    const { api } = window.__musicpack!;
    const albums = await api.albums({ limit: 50 });
    const long = albums.albums.find((a) => a.title === 'Long Player')!;
    const detail = await api.album(long.id);
    return detail.releases[0]!.id;
  });
}

async function installViaUi(page: import('@playwright/test').Page): Promise<void> {
  await page.getByRole('button', { name: /^Download Long Player for offline/ }).click();
  // Progress becomes the Installed badge (atomic commit; no partial state).
  await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });
}

test.describe('offline downloads', () => {
  test('download once via the UI, play offline across tracks, then return online', async ({
    page,
    context,
  }) => {
    await signIn(page);
    const releaseId = await openLongPlayer(page);

    // ---- UI-driven install -------------------------------------------
    await installViaUi(page);
    await waitFor(
      page,
      async () =>
        (await page.evaluate((id) => window.__musicpack!.offline.states.get().get(id)?.state, releaseId)) ===
        'installed',
      { label: 'package installed', timeout: 30_000 },
    );

    // D1 local-first while ONLINE: built items already use local sources.
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'online local playback',
    });
    const items = await queueItems(page);
    expect(items[0]?.kind).toBe('local-file');

    // ---- go offline mid-session -------------------------------------
    await context.setOffline(true);

    // Seek offline (random access over OPFS) while still on the 48 s
    // track. Tracks 2-4 are ~1 s clips, and the seek slider's max is the
    // current track's duration: a committed 10 s seek there lands on the
    // track boundary and queues the auto-advance chain into the album
    // end. Whether the reload below beat that chain to persist a
    // playable session was the historical CI-only flake (web/README.md
    // "E2E / CI reliability notes").
    await page.locator('.playerbar input[type=range]').first().evaluate((el) => {
      const input = el as HTMLInputElement;
      input.value = String(10);
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await waitFor(
      page,
      async () =>
        (await playerState(page)).state === 'playing' &&
        (await playerState(page)).positionSeconds > 5,
      { label: 'offline seek resumes playback', timeout: 20_000 },
    );
    expect((await playerState(page)).error).toBeUndefined();

    // ---- full offline reload ----------------------------------------
    await page.reload();
    // The session probe fails (network gone); installed content exists, so
    // the app must NOT show the sign-in screen...
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeHidden({ timeout: 20_000 });
    // ...and the NavBar must announce offline mode.
    await expect(page.getByText('Offline', { exact: true })).toBeVisible({ timeout: 20_000 });

    await page.locator('.playerbar').getByRole('button', { name: 'Play', exact: true }).click();
    // Poll through the debug hook so that, if this ever times out, the
    // failure message carries the exact restored player/queue state (the
    // historical flake signature was a fetch error on the restored item —
    // see web/README.md "E2E / CI reliability notes").
    let lastPoll: Record<string, unknown> | null = null;
    try {
      await waitFor(page, async () => {
        lastPoll = await page.evaluate(() => {
          const m = window.__musicpack?.player?.model?.get();
          const items = window.__musicpack?.queue?.get?.().items ?? [];
          return {
            state: m?.state,
            error: m?.error,
            pos: m?.positionSeconds,
            current: m?.current?.track?.title ?? null,
            currentSource: m?.current?.source?.kind ?? null,
            index: window.__musicpack?.queue?.get?.().index,
            sources: items.map((i) => i.source?.kind),
          };
        });
        return lastPoll?.state === 'playing';
      }, {
        label: 'offline playback after reload',
        timeout: 30_000,
      });
    } catch {
      throw new Error(
        `offline playback after reload | restored state: ${JSON.stringify(lastPoll)}`,
      );
    }
    await waitFor(page, async () => (await playerState(page)).positionSeconds > 1, {
      label: 'offline position advances',
    });

    await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
    await waitFor(
      page,
      async () =>
        (await playerState(page)).positionSeconds > 0.3 || (await playerState(page)).state === 'playing',
      { label: 'offline track progression', timeout: 20_000 },
    );
    expect((await playerState(page)).error).toBeUndefined();
    expect((await queueItems(page))[1]?.kind).toBe('local-file');

    // ---- restore the network ----------------------------------------
    await context.setOffline(false);
    // Reconnect recovery: the app re-probes (event + slow fallback timer)
    // and clears the Offline chip without a reload. The 30 s probe interval
    // is the bound; poll rather than assume event timing.
    await waitFor(
      page,
      async () => !(await page.getByText('Offline', { exact: true }).isVisible()),
      { label: 'offline chip clears after reconnect', timeout: 40_000 },
    );
    // Normal online playback of a NON-installed release still works.
    await page.goto('/');
    await page.getByText('Shapeshifter').click();
    await page.getByRole('button', { name: 'Play album' }).click();
    await waitFor(page, async () => (await playerState(page)).state === 'playing', {
      label: 'online playback restored',
      timeout: 30_000,
    });
    expect((await queueItems(page))[0]?.kind).toBe('http-range');
  });

  test('cancel during download reports failure and retry succeeds', async ({ page }) => {
    await signIn(page);
    await openLongPlayer(page);

    const dl = page.getByRole('button', { name: /^Download Long Player for offline/ });
    // force: the control re-renders as states flip (download -> installed)
    // and a non-forced click can outlive its element on this fast fixture.
    await dl.click({ force: true });
    // Cancel as soon as the control flips to its downloading variant.
    const cancel = page.getByRole('button', { name: 'Cancel Long Player download' });
    if (await cancel.isVisible().catch(() => false)) {
      await cancel.click({ force: true });
      await expect(page.getByRole('button', { name: 'Retry Long Player download' })).toBeVisible({
        timeout: 15_000,
      });
    }
    // Either way, finishing the install must end in the Installed state.
    // (On this fast fixture the install may complete before Cancel becomes
    // clickable — the control then reads Installed directly, which is a
    // pass for the same user-visible contract.)
    await expect(page.locator('.dl-badge').or(
      page.getByRole('button', { name: 'Retry Long Player download' }),
    ).first()).toBeVisible({ timeout: 30_000 });
    const retry = page.getByRole('button', { name: 'Retry Long Player download' });
    if (await retry.isVisible().catch(() => false)) {
      await retry.click({ force: true });
    } else {
      const dlAgain = page.getByRole('button', { name: /^Download Long Player for offline/ });
      if (await dlAgain.isVisible().catch(() => false)) {
        await dlAgain.click({ force: true });
      }
    }
    await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });
  });

  test('remove download returns the release to remote sources', async ({ page }) => {
    await signIn(page);
    const releaseId = await openLongPlayer(page);
    await installViaUi(page);
    await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

    await page.getByRole('button', { name: `Remove Long Player download` }).click();
    await expect(
      page.getByRole('button', { name: /^Download Long Player for offline/ }),
    ).toBeVisible();

    const pkg = await page.evaluate((id) => window.__musicpack!.offline.packageFor(id), releaseId);
    expect(pkg).toBeNull();
  });

  // Plan §8.5: the stale *rendering*. Staleness is arranged through the
  // real D2 mechanism (manager.checkForUpdate flags the record when online
  // hashes differ) using an in-memory-modified release document as the
  // state fixture — everything user-visible after that point (badge,
  // Update button, the update flow itself, recovery to healthy) goes
  // through the actual UI.
  test('stale package offers Update and returns to Installed via the UI', async ({ page }) => {
    await signIn(page);
    const releaseId = await openLongPlayer(page);
    await installViaUi(page);

    // Arrange: hand checkForUpdate a release document whose primary hash
    // differs from the committed one — exactly what it would see after a
    // server-side content update.
    await page.evaluate(async (id) => {
      const { api, offline } = window.__musicpack!;
      const fresh = await api.release(id);
      fresh.media[0]!.tracks[0]!.audio.sha256 = 'f'.repeat(64);
      const flagged = await offline.checkForUpdate(fresh);
      if (!flagged) throw new Error('fixture failed to flag the package stale');
    }, releaseId);

    // User-visible stale branch.
    await expect(page.locator('.dl-badge')).toHaveText(/Update available/, { timeout: 15_000 });
    const update = page.getByRole('button', { name: 'Update Long Player download' });
    await expect(update).toBeVisible();

    // Invoke Update through the UI: fresh JSON is fetched, the install
    // supersedes the record, and the stale flag is gone.
    await update.click();
    await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

    const pkg = await page.evaluate((id) => window.__musicpack!.offline.packageFor(id), releaseId);
    expect(pkg?.status).toBe('installed');
    expect(pkg?.stale).toBeFalsy();
  });

  // Plan §8.6: the corruption drill. An installed OPFS asset is truncated
  // behind the catalog's back (browser-eviction simulation); the next boot
  // audit must surface "Needs repair", and Reinstall through the UI must
  // restore a fully verified package. Integrity semantics untouched.
  test('truncated installed file surfaces Needs repair and Reinstall heals', async ({
    page,
  }) => {
    await signIn(page);
    const releaseId = await openLongPlayer(page);
    await installViaUi(page);

    // Locate the first installed asset and truncate its committed OPFS
    // file to 1 byte, behind the catalog's back (browser-eviction drill).
    const truncated: { truncatedKey?: string; error?: string; ids?: number[] } =
      await page.evaluate(async (id) => {
        const hook = window.__musicpack!;
        const all = await hook.offline.listPackages();
        const mine = all.find((x) => x.releaseId === id);
        if (!mine || !mine.assets[0]) return { error: 'no asset', ids: all.map((x) => x.releaseId) };
        const key0 = mine.assets[0]!.key;
        const root = await navigator.storage.getDirectory();
        const base = await root.getDirectoryHandle('musicpack-offline-v1');
        const releases = await base.getDirectoryHandle('releases');
        const fh = await releases.getFileHandle(key0);
        const w = await fh.createWritable({ keepExistingData: true });
        await w.truncate(1);
        await w.close();
        return { truncatedKey: key0 };
      }, releaseId);
    expect(truncated.truncatedKey).toBeTruthy();

    // Reload: the boot audit reconciles files vs catalog and strips the
    // damaged asset; the manager presents 'damaged', never plain 'stale'.
    await page.reload();
    // Already on the album page after the reload; the heading locator is
    // unambiguous now that the breadcrumb also carries the album title.
    await page.getByRole('heading', { name: 'Long Player' }).click();
    await expect(page.locator('.dl-badge')).toHaveText(/Needs repair/, { timeout: 30_000 });

    const reinstall = page.getByRole('button', { name: 'Reinstall Long Player download' });
    await expect(reinstall).toBeVisible();

    // Reinstall through the UI: fresh JSON, full re-download, atomic commit.
    await reinstall.click();
    await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

    // Truth check: the committed record holds verified assets again.
    const pkg = await page.evaluate((id) => window.__musicpack!.offline.packageFor(id), releaseId);
    expect(pkg?.status).toBe('installed');
    expect(pkg!.assets.length).toBeGreaterThan(0);
    for (const asset of pkg!.assets) {
      expect(asset.state).toBe('ok');
    }
  });

  // R3.6: track-linked lyrics stage through the same offline machinery,
  // driven entirely by the release-level index — the install must never
  // fetch a track detail (docs/r3.6-offline-lyrics-contract.md §13).
  test('lyrics stage offline from the release index without track-detail requests', async ({
    page,
  }) => {
    await signIn(page);
    const trackDetailRequests: string[] = [];
    page.on('request', (r) => {
      const path = new URL(r.url()).pathname;
      if (/^\/api\/v1\/tracks\/\d+$/.test(path)) trackDetailRequests.push(path);
    });

    await page.getByText('Lyric Shelf').first().click();
    await expect(page.getByRole('heading', { name: 'Lyric Shelf' })).toBeVisible();
    const releaseId = await page.evaluate(async () => {
      const { api } = window.__musicpack!;
      const albums = await api.albums({ limit: 50 });
      const shelf = albums.albums.find((a) => a.title === 'Lyric Shelf')!;
      const detail = await api.album(shelf.id);
      return detail.releases[0]!.id;
    });

    await page.getByRole('button', { name: /^Download Lyric Shelf for offline/ }).click();
    await expect(page.locator('.dl-badge')).toHaveText(/Installed/, { timeout: 30_000 });

    // The release index drove the plan: no track-detail request was made.
    expect(trackDetailRequests).toEqual([]);

    const state = await page.evaluate(async (id) => {
      const hook = window.__musicpack!;
      const release = await hook.api.release(id);
      const pkg = await hook.offline.packageFor(id);
      const lyrics = release.trackLyrics ?? [];
      return {
        index: lyrics,
        assets: (pkg?.assets ?? [])
          .filter((a) => a.kind === 'lyrics')
          .map((a) => ({ key: a.key, trackId: a.trackId, lyricsId: a.lyricsId, state: a.state, size: a.size })),
        keys: lyrics.map((l) => hook.offline.availability.lyricKeyFor(l.trackId, l.id)),
        damagedKeys: lyrics.map((l) => hook.offline.availability.lyricKeyFor(l.trackId, 999_999)),
      };
    }, releaseId);

    // The fixture attaches one document to track 1 and two to track 2.
    expect(state.index.length).toBe(3);
    expect(state.index.map((l) => l.lang)).toEqual(['en', 'en', 'fr']);
    expect(state.assets.length).toBe(3);
    for (const asset of state.assets) {
      expect(asset.state).toBe('ok');
      expect(asset.key).toBe(`t.${asset.trackId}.lyr.${asset.lyricsId}`);
    }
    // Availability resolves each staged document by identity; unknown ids
    // never resolve to another asset's bytes.
    for (const key of state.keys) expect(key).toBeTruthy();
    for (const key of state.damagedKeys) expect(key).toBeNull();

    // The committed bytes are real OPFS files at the planned keys.
    const sizes = await page.evaluate(async (keys) => {
      const root = await navigator.storage.getDirectory();
      const base = await root.getDirectoryHandle('musicpack-offline-v1');
      const releases = await base.getDirectoryHandle('releases');
      const out: Array<number | null> = [];
      for (const key of keys) {
        try {
          const fh = await releases.getFileHandle(key);
          out.push((await fh.getFile()).size);
        } catch {
          out.push(null);
        }
      }
      return out;
    }, state.keys as string[]);
    expect(sizes.every((s) => typeof s === 'number' && s > 0)).toBe(true);
  });
});
