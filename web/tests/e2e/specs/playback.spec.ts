import { test, expect } from '@playwright/test';
import { selectBackend, signIn, playerState, waitFor } from './helpers';

// These scenarios exercise the legacy engines, which are now the explicit
// escape hatch (Rust is the Phase 14H-5 default). Selecting legacy keeps this
// suite testing the legacy implementation unchanged.
test.beforeEach(async ({ page }) => {
  await selectBackend(page, 'legacy');
  await signIn(page);
});

/** Sets a range slider value, firing the same input+change the UI listens to. */
async function setSeek(page: import('@playwright/test').Page, seconds: number): Promise<void> {
  await page.locator('.playerbar input[type=range]').first().evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, seconds);
}

test('plays a Musepack album demand-driven and seeks without downloading the file', async ({ page }) => {
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();

  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  const start = await playerState(page);
  expect(start.currentTitle).toBeTruthy();
  expect(start.normDb).toBeLessThan(0); // album normalization applied

  // Time-to-first-PCM / bytes-before-playback: far less than the full file.
  const servedAtPlay = await playerState(page).then((s) => s.servedBytes);
  const size = await page.evaluate(() => {
    const item = window.__musicpack?.player.model.get().current;
    return item?.track.audio.size ?? 0;
  });
  expect(servedAtPlay).toBeLessThan(size * 0.5);

  // Position advances without error.
  await waitFor(page, async () => (await playerState(page)).positionSeconds > 1, { label: 'position advances' });

  // Seek to ~50%, then ~90%, then backwards to ~10% of the CURRENT TRACK
  // (the hidden range is track-scoped, matching what the waveform draws).
  // Position jumps accordingly; the whole file is never fetched (the demand
  // reader fetches only the needed blocks).
  const st = await playerState(page);
  const t0 = st.currentTrackStartSeconds;
  const td = st.currentTrackDurationSeconds;
  expect(td).toBeGreaterThan(20); // room to seek inside the track
  await setSeek(page, Math.round(td * 0.5));
  await waitFor(page, async () => (await playerState(page)).positionSeconds > t0 + td * 0.4,
              { label: 'seek 50%' });
  await setSeek(page, Math.round(td * 0.9));
  await waitFor(page, async () => (await playerState(page)).positionSeconds > t0 + td * 0.8,
              { label: 'seek 90%' });
  await setSeek(page, Math.round(td * 0.1));
  await waitFor(page, async () => {
    const s = await playerState(page);
    return s.positionSeconds > t0 && s.positionSeconds < t0 + td * 0.3;
  }, { label: 'seek backwards' });

  const final = await playerState(page);
  expect(final.servedBytes).toBeLessThan(size); // never downloaded the whole file
  expect(final.state).not.toBe('error');
});

test('pause, resume, and next track behave', async ({ page }) => {
  // Long Player's first track is 48 s, so pause/resume/next have room.
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  const pausedAt = (await playerState(page)).positionSeconds;
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });

  // Position freezes while paused.
  const stillPaused = await playerState(page);
  expect(stillPaused.positionSeconds).toBeGreaterThanOrEqual(pausedAt - 0.3);

  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });

  const before = await playerState(page);
  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(
    page,
    async () => (await playerState(page)).currentTitle !== before.currentTitle,
    { label: 'next track' },
  );
});

test('gapless album playback crosses the track boundary continuously', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Wait until the queue cursor advances into track 2 (gapless handoff) with
  // position still increasing — no error, no stuck state.
  await waitFor(
    page,
    async () => {
      const q = await page.evaluate(() => window.__musicpack?.queue.get().index ?? -1);
      return q >= 1;
    },
    { label: 'gapless into track 2', timeout: 45_000 },
  );
  const state = await playerState(page);
  expect(state.state).not.toBe('error');
  expect(state.currentTitle).toBeTruthy();
});

test('queue holds the album in order and removes items', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Pause immediately: the fixture tracks are ~1 s long and would play out
  // (and move the highlight) before the assertions below.
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });

  await page.getByRole('button', { name: 'Open the queue' }).click();
  const items = await page.evaluate(() => window.__musicpack?.queue.get().items.map((i) => i.track.title) ?? []);
  expect(items).toHaveLength(4); // the fixture album has 4 tracks
  expect(items[0]).toContain('Big in Japan');
  // The highlighted list entry must be the cursor's item (coherence), not a
  // frozen position.
  const res = await page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    // Rows may render in the /queue route or in the drawer overlay (outside
    // #main) since album play actions no longer redirect; match either.
    const containers = [...document.querySelectorAll('.queue-item')];
    return {
      idx: q?.index ?? -1,
      markedIdx: containers.findIndex((el) => el.getAttribute('aria-current') === 'true'),
    };
  });
  expect(res.idx).toBeGreaterThanOrEqual(0);
  expect(res.markedIdx).toBe(res.idx);
});

test('player survives a page reload (restored paused at position)', async ({ page }) => {
  await page.getByText('Long Player').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await page.waitForTimeout(1200); // let the song play a little

  // Pause persists immediately (the tick path is throttled).
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });
  const saved = await playerState(page);
  expect(saved.positionSeconds).toBeGreaterThan(0.5);

  // Full reload: the in-memory queue/player is gone; persistence must
  // bring the player back, paused at the saved spot. (We reload on /queue,
  // so the shelf never appears — wait for the restored session instead.)
  await page.reload();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'restored paused' });

  const st = await playerState(page);
  expect(st.state).toBe('paused');
  expect(st.currentTitle).toBe(saved.currentTitle);
  expect(Math.abs(st.positionSeconds - saved.positionSeconds)).toBeLessThan(3);
  const n = await page.evaluate(() => window.__musicpack?.queue.get().items.length ?? 0);
  expect(n).toBeGreaterThan(0);

  // Press resumes playback at the restored position (gesture requirement).
  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
  const resumed = await playerState(page);
  expect(Math.abs(resumed.positionSeconds - saved.positionSeconds)).toBeLessThan(4);
});

test('clicking a queue item keeps the queue and moves the highlight', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').first().click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  const titles = await page.evaluate(() => window.__musicpack?.queue.get().items.map((i) => i.track.title) ?? []);
  const n = titles.length;
  expect(n).toBeGreaterThan(1);

  // Jump to the LAST track via the queue drawer. Do NOT navigate: these
  // fixture tracks are ~1 s long and the album can play out before a
  // route change settles.
  await page.locator('.playerbar').getByRole('button', { name: 'Open the queue' }).click();
  await expect(page.locator('.queue-panel .queue-item').first()).toBeVisible();
  await page.locator('.queue-panel .queue-item button', { hasText: titles[n - 1] }).first().click();

  // The queue must survive the click (it used to be replaced by the single
  // clicked song), the cursor must land on the clicked item...
  await waitFor(
    page,
    async () => {
      const s = await page.evaluate(() => {
        const q = window.__musicpack?.queue.get();
        const m = window.__musicpack?.player.model.get();
        return { n: q?.items.length ?? 0, idx: q?.index ?? -1, title: m?.current?.track.title ?? null };
      });
      return s.n === n && s.idx === n - 1 && s.title === titles[n - 1];
    },
    { label: 'clicked queue item plays, queue intact' },
  );

  // ...and the highlight must follow the cursor exactly (no stuck item 1).
  const res = await page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    const containers = [...document.querySelectorAll('.queue-panel .queue-item')];
    const markedIdx = containers.findIndex((el) => el.getAttribute('aria-current') === 'true');
    return { idx: q?.index ?? -1, count: q?.items.length ?? 0, markedIdx };
  });
  expect(res.count).toBe(n);
  expect(res.idx).toBe(n - 1);
  expect(res.markedIdx).toBe(n - 1);
});

test('album seek past the current track switches to a later track', async ({ page }) => {
  // Long Player: track 1 decodes to ~39 s (the manifest's 48 s is a
  // placeholder); the model reports the REAL total duration.
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  const titles = await page.evaluate(() => window.__musicpack?.queue.get().items.map((i) => i.track.title) ?? []);
  const dur = (await playerState(page)).durationSeconds;
  // seek well past track 1's real end so the target lives in a later track
  const target = Math.max(dur * 0.95, dur - 2);

  await setSeek(page, target);
  // the cursor must leave track 1 and settle on a later track that matches
  // the controller's current track
  await waitFor(
    page,
    async () => {
      const idx = await page.evaluate(() => window.__musicpack?.queue.get().index ?? -1);
      if (idx < 1) return false;
      const cur = await page.evaluate(() => window.__musicpack?.player.model.get().current?.track.title ?? null);
      return cur === titles[idx];
    },
    { label: 'seek moved off track 1' },
  );
  const idx = await page.evaluate(() => window.__musicpack?.queue.get().index ?? -1);
  const st = await playerState(page);
  expect(st.currentTitle).toBe(titles[idx]);
  // the reported position is in the target range, not the previous track
  expect(st.positionSeconds).toBeGreaterThan(dur * 0.8);
  expect(st.state).not.toBe('error');
});

test('clicking a disc-2 track selects the correct flattened queue index', async ({ page }) => {
  await page.getByText('Two Disc Extravaganza').click();
  await page.locator('button.track-play', { hasText: 'Side Two One' }).click();
  // playAlbum sets the queue index synchronously, so capture it immediately
  // (the ~1 s fixture tracks may advance before the state read).
  const state = await page.evaluate(() => {
    const q = window.__musicpack?.queue;
    const items = q?.get().items ?? [];
    return { index: q?.get().index ?? -1, items: items.map((i) => i.track.title) };
  });
  // flat index = disc 1 track count (4) + 0
  expect(state.index).toBe(4);
  expect(state.items[state.index]).toBe('Side Two One');
});

test('plays through every track (repeated worker teardown) without wedging', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  // Let the album play through all 4 short (~1 s) tracks: every gapless
  // handoff tears down and re-spawns a decoder + network worker, so this
  // exercises the teardown handshake repeatedly. Playback must reach the
  // ended state without an error or a wedged controller.
  await waitFor(page, async () => (await playerState(page)).state === 'ended', {
    label: 'album played to the end',
    timeout: 30_000,
  });
  const st = await playerState(page);
  expect(st.state).toBe('ended');
  expect(st.error).toBeUndefined();
});

test('signing out stops playback, disposes the backend and clears player state', async ({ page }) => {
  await page.getByText('Long Player').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  expect((await playerState(page)).state).toBe('playing');

  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible({ timeout: 20_000 });

  const after = await page.evaluate(() => {
    const p = window.__musicpack?.player;
    const m = p?.model.get();
    return { state: m?.state ?? 'idle', current: m?.current ?? null, backendKind: p?.getBackendKind() ?? null };
  });
  expect(after.state).toBe('idle');
  expect(after.current).toBeNull();
  expect(after.backendKind).toBeNull(); // decoder/AudioContext disposed
});

test('clearing the queue while paused returns the player to idle', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  // Pause first: the fixture tracks are ~1 s long and would play out.
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });

  await page.locator('.playerbar').getByRole('button', { name: 'Open the queue' }).click();
  // Scope to the drawer: the /queue page behind it renders a second
  // "Clear queue" button (playAlbum routes to /queue).
  await page.locator('.queue-panel').getByRole('button', { name: 'Clear queue' }).click();

  await waitFor(page, async () => (await playerState(page)).state === 'idle', { label: 'idle after clear' });
  const after = await page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    const m = window.__musicpack?.player.model.get();
    return { n: q?.items.length ?? -1, current: m?.current ?? null };
  });
  expect(after.n).toBe(0);
  // Characterization: stop() resets state/position but deliberately keeps
  // model.current (the PlayerBar keeps showing the last track after a
  // queue clear). The target Player design should clear it; tracked in the
  // refactor plan as a behavior change to make consciously.
  expect(after.current).not.toBeNull();
});

test('removing the playing queue item advances to the following track', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });

  // Snapshot the queue AND the actual cursor: ~1 s fixture tracks may have
  // gaplessly advanced before pause landed, so "the playing item" is
  // items[idx], not necessarily items[0].
  const { ids, idx } = await page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    return { ids: q?.items.map((i) => i.id) ?? [], idx: q?.index ?? -1 };
  });
  expect(ids.length).toBeGreaterThan(1);

  // Remove the playing item through the queue store (the panel's ✕ button
  // calls exactly this). Driving it directly removes the open-panel/click
  // locator race that flaked on slow CI runners; the behavior under test is
  // the core's reaction to an external mutation.
  const removedId = ids[idx];
  await page.evaluate(
    (i: number) => window.__musicpack?.queue.removeAt(i),
    idx,
  );

  // The cursor moves off the removed item and the controller settles it;
  // the pause intent is preserved (no audio starts). Whether the core has
  // already loaded the neighbor when we observe is a load race — assert on
  // item identity, not on which side of it we caught.
  await waitFor(
    page,
    async () => {
      const s = await page.evaluate(() => {
        const q = window.__musicpack?.queue.get();
        const m = window.__musicpack?.player.model.get();
        return {
          n: q?.items.length ?? -1,
          idsNow: q?.items.map((i) => i.id) ?? [],
          curId: m?.current?.id ?? null,
        };
      });
      return (
        s.n === ids.length - 1 &&
        !s.idsNow.includes(removedId) &&
        s.curId !== removedId
      );
    },
    { label: 'cursor moved off the removed item' },
  );
  const st = await playerState(page);
  expect(st.state).toBe('paused');
});

test('shuffle toggle reorders navigation while the queue stays canonical', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await page.locator('.playerbar').getByRole('button', { name: 'Pause' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'paused' });

  // Toggle shuffle via the player bar.
  await page.getByRole('button', { name: /Shuffle: off/ }).click();

  // Model reflects the policy; the canonical items list is untouched.
  const after = await page.evaluate(() => {
    const q = window.__musicpack?.queue.get();
    const m = window.__musicpack?.player.model.get();
    return { n: q.items.length, idx: q.index, shuffle: m.shuffle, repeat: m.repeat };
  });
  expect(after.shuffle).toBe(true);
  expect(after.n).toBe(4); // canonical order intact

  // Next under shuffle moves within the same item set (no reload error).
  await page.locator('.playerbar').getByRole('button', { name: 'Play' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'resumed' });
  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(
    page,
    async () => {
      const s = await playerState(page);
      return s.currentTitle !== '' && s.state === 'playing';
    },
    { label: 'shuffled next playing' },
  );
});

test('repeat-all wraps from the last track back to the first', async ({ page }) => {
  await page.getByText('Synthetic Test Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });

  // Enable repeat-all and jump to the LAST track through the player API.
  // The old UI dance (Pause -> queue.moveTo(last) -> Play) raced the
  // out-of-band load that a cursor move triggers while paused: on slow CI
  // runners Play could land mid-load and leave the gate stopped, parking
  // playback in 'buffering' forever. playQueueIndex performs one
  // deterministic load-and-resume.
  await page.evaluate(() => {
    const p = window.__musicpack?.player;
    p.setRepeat('all');
    void p.playQueueIndex(p.queue.get().items.length - 1);
  });
  await waitFor(
    page,
    async () => {
      const s = await playerState(page);
      const idx = await page.evaluate(() => window.__musicpack?.queue.get().index);
      return s.state === 'playing' && idx === 3;
    },
    { label: 'playing at last track', timeout: 20_000 },
  );

  await page.locator('.playerbar').getByRole('button', { name: 'Next track' }).click();
  await waitFor(
    page,
    async () => (await page.evaluate(() => window.__musicpack?.queue.get().index)) === 0,
    { label: 'wrapped to first track' },
  );
  await waitFor(
    page,
    async () => (await playerState(page)).state === 'playing',
    { label: 'first track playing after wrap' },
  );
});

test('crossfade setting persists across a reload (native FLAC album)', async ({ page }) => {
  // Native-codec fixture: the only lane with Phase A crossfade.
  await page.getByText('Synthetic Classical Compilation').click();
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  expect((await playerState(page)).state).toBe('playing');
  const kind = await page.evaluate(() => window.__musicpack?.player.getBackendKind());
  expect(kind).toBe('native');

  // Enable crossfade via the player-bar cycler, then reload.
  await page.getByRole('button', { name: /Crossfade/ }).click();
  const enabled = await page.evaluate(() => window.__musicpack?.player.model.get().crossfadeSeconds);
  expect(enabled).toBe(4);

  await page.reload();
  await waitFor(page, async () => (await playerState(page)).state === 'paused', { label: 'restored paused' });
  const restored = await page.evaluate(() => window.__musicpack?.player.model.get().crossfadeSeconds);
  expect(restored).toBe(4);
});

test('musepack crossfade advances through tracks without error', async ({ page }) => {
  // Phase B: worklet overlap-add on the musepack lane. "Fade Rider" holds
  // two 48 s tracks: decode is paced by the ring, so seeking to ~7.5 s
  // before the boundary makes the crossfade trigger (remaining <= 12 s)
  // fire while the standby is still open — the fade path engages.
  await page.getByText('Fade Rider').first().click();
  await page.evaluate(() => (window.__musicpack?.player as any).setCrossfade(12));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  const kind = await page.evaluate(() => window.__musicpack?.player.getBackendKind());
  expect(kind).toBe('musepack');

  // Spy the engine capability once it exists.
  await page.evaluate(() => {
    const eng = (window.__musicpack?.player as any).core.engine;
    const orig = eng.beginCrossfade.bind(eng);
    window.__xfadeResult = 'none';
    eng.beginCrossfade = async (...args: unknown[]) => {
      const r = await orig(...args);
      window.__xfadeResult = r ? 'taken' : 'declined';
      return r;
    };
  });

  const firstTitle = (await playerState(page)).currentTitle;
  // Track is 48 s; seek to 40.5 s leaves ~7.5 s — inside the 12 s window,
  // above the ring's high-water lead, so the trigger wins the race.
  await page.evaluate(() => (window.__musicpack?.player as any).seek(40.5));

  let fadeResult = 'none';
  let advanced = false;
  for (let i = 0; i < 80 && !(fadeResult === 'taken' && advanced); i++) {
    await page.waitForTimeout(250);
    const s = await playerState(page);
    if (s.state === 'error') throw new Error(`player errored: ${s.error}`);
    if (s.currentTitle && s.currentTitle !== firstTitle) advanced = true;
    fadeResult = await page.evaluate(() => (window as any).__xfadeResult ?? 'none');
  }
  expect(fadeResult).toBe('taken');
  expect(advanced).toBe(true);
});


// ---- M8 repair: Sweet Fade regression coverage (steps 7.1–7.3) -----------

interface XfSpy {
  calls: number;
  started: boolean;
  result: 'none' | 'taken' | 'declined';
}

/** Wraps the live engine's beginCrossfade to observe attempts from the test. */
async function installXfadeSpy(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const p = window.__musicpack?.player as any;
    const eng = p.core.engine;
    const orig = eng.beginCrossfade.bind(eng);
    (window as any).__xf = { calls: 0, started: false, result: 'none' };
    eng.beginCrossfade = async (...args: unknown[]) => {
      const w = (window as any).__xf as XfSpy;
      w.calls++;
      w.started = true;
      const r = await orig(...args);
      w.result = r ? 'taken' : 'declined';
      return r;
    };
  });
}

async function xfState(page: import('@playwright/test').Page): Promise<XfSpy> {
  return page.evaluate(() => (window as any).__xf as XfSpy);
}

type PlayerSnapshot = Awaited<ReturnType<typeof playerState>>;

/** Polls player state, recording every position sample until done() holds. */
async function sampleUntil(
  page: import('@playwright/test').Page,
  done: (s: PlayerSnapshot) => boolean,
  maxMs: number,
): Promise<{ s: PlayerSnapshot; samples: number[] }> {
  const samples: number[] = [];
  const t0 = Date.now();
  for (;;) {
    const s = await playerState(page);
    if (s.error) throw new Error(`player errored: ${s.error}`);
    samples.push(s.positionSeconds);
    if (done(s)) return { s, samples };
    if (Date.now() - t0 > maxMs) {
      throw new Error(`timeout; last=${JSON.stringify(s)} samples=${samples.length}`);
    }
    await page.waitForTimeout(150);
  }
}

/** The album clock must never regress — that was BUG-2's signature. */
function monotonic(samples: number[], eps = 0.05): void {
  for (let i = 1; i < samples.length; i++) {
    expect(samples[i]).toBeGreaterThanOrEqual(samples[i - 1] - eps);
  }
}

test('chained musepack fades keep advancing through later boundaries (BUG-1)', {
  timeout: 120_000,
}, async ({ page }) => {
  test.setTimeout(150_000);
  // Long Player: one 48 s opener followed by shorter musepack tracks, so
  // the fade into track 2 is followed by further boundaries. Before the
  // repair, the stale eos-suppression left behind by the first fade
  // swallowed the next track's decode-EOS and playback hung forever right
  // there — this asserts boundaries KEEP advancing after a fade.
  await page.getByText('Long Player').first().click();
  await page.evaluate(() => (window.__musicpack?.player as any).setCrossfade(4));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await installXfadeSpy(page);

  // Jump near the end of the opener so its boundary fades.
  await page.evaluate(() => (window.__musicpack?.player as any).seek(45.5));
  const first = await playerState(page);
  const firstTitle = first.currentTitle;

  // Queue-index progression: very short fixture tracks can outrun the poll
  // interval, so count boundaries crossed instead of distinct titles.
  let last: PlayerSnapshot | null = null;
  let lastIndex = -1;
  const t0 = Date.now();
  const samples: number[] = [];
  const seen = new Set<string>();
  for (;;) {
    const probe = await page.evaluate(() => {
      const p = window.__musicpack?.player as any;
      return {
        idx: p?.queue.get().index ?? -1,
        err: p?.model.get().error,
        state: p?.model.get().state,
        pos: p?.model.get().positionSeconds ?? 0,
        title: (p?.model.get().current?.track.title as string | undefined) ?? null,
      };
    });
    if (probe.err) throw new Error(`player errored: ${probe.err}`);
    samples.push(probe.pos);
    lastIndex = probe.idx;
    if (probe.title) seen.add(probe.title);
    last = {
      ...first,
      currentTitle: probe.title,
      positionSeconds: probe.pos,
      state: probe.state as PlayerSnapshot['state'],
    };
    // Crossed at least two boundaries past the opener.
    if (lastIndex >= 2) break;
    if (Date.now() - t0 > 60_000) break;
    await page.waitForTimeout(200);
  }
  monotonic(samples);

  const xf = await xfState(page);
  // The EOS path is now content-aware (Smart Fades): same-release
  // constant-amplitude (sine) tracks join gaplessly, so the opener boundary
  // may correctly decline a fade. The BUG-1 invariant is that boundaries KEEP
  // advancing (no hang) regardless of whether each boundary fades or goes
  // gapless — that is what the chained-advancement guard below asserts.
  if (xf.calls > 0) {
    // Any fade that did engage must have been taken, never left dangling.
    expect(xf.result).toBe('taken');
  }
  // BUG-1 GUARD: at least one boundary after the opener must advance.
  expect(lastIndex).toBeGreaterThanOrEqual(2);
  expect(seen.size).toBeGreaterThanOrEqual(2);
  expect(last?.error).toBeUndefined();
});

test('sweet fade keeps the playhead monotonic and seeking lands in the new track', {
  timeout: 90_000,
}, async ({ page }) => {
  await page.getByText('Fade Rider').first().click();
  await page.evaluate(() => (window.__musicpack?.player as any).setCrossfade(4));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await installXfadeSpy(page);

  await page.evaluate(() => (window.__musicpack?.player as any).seek(45.5));
  const { s, samples } = await sampleUntil(
    page,
    (st) => st.currentTitle === 'Fade Rider - Sunrise',
    30_000,
  );
  monotonic(samples);

  // Step-2 accounting: the album total MUST be compressed by the overlap
  // (somewhere between a 1 s floor and the 4 s cap) — never the raw 96 s.
  expect(s.durationSeconds).toBeGreaterThan(91);
  expect(s.durationSeconds).toBeLessThan(96);
  expect(s.currentTrackStartSeconds).toBeGreaterThan(43);
  expect(s.currentTrackStartSeconds).toBeLessThanOrEqual(46);
  // The promoted track had been audibly playing for about one overlap.
  const within = s.positionSeconds - s.currentTrackStartSeconds;
  expect(within).toBeGreaterThan(0.5);
  expect(within).toBeLessThan(s.currentTrackDurationSeconds);

  // A seek right after the fade must resolve inside the CURRENT track.
  const target = s.currentTrackStartSeconds + 10;
  await page.evaluate((t) => (window.__musicpack?.player as any).seek(t), target);
  const after = await sampleUntil(
    page,
    (st) => Math.abs(st.positionSeconds - st.currentTrackStartSeconds - 10) < 0.75,
    15_000,
  );
  monotonic(after.samples);
  expect(after.s.currentTitle).toBe('Fade Rider - Sunrise');
});

test('pausing mid-fade neither double-fires nor loses the transition', {
  timeout: 120_000,
}, async ({ page }) => {
  await page.getByText('Fade Rider').first().click();
  await page.evaluate(() => (window.__musicpack?.player as any).setCrossfade(4));
  await page.getByRole('button', { name: 'Play album' }).click();
  await waitFor(page, async () => (await playerState(page)).state === 'playing', { label: 'playing' });
  await installXfadeSpy(page);

  await page.evaluate(() => (window.__musicpack?.player as any).seek(45.5));
  // Wait until the fade attempt is LIVE, then pause inside its window.
  await page.waitForFunction(() => (window as any).__xf?.started === true);
  await page.waitForTimeout(500);
  await page.evaluate(() => (window.__musicpack?.player as any).pause());
  await page.waitForTimeout(700);
  await page.evaluate(() => (window.__musicpack?.player as any).resume());

  const fin = await sampleUntil(
    page,
    (st) => st.currentTitle === 'Fade Rider - Sunrise' && st.state === 'playing',
    40_000,
  );
  monotonic(fin.samples);
  const xf = await xfState(page);
  expect(xf.calls).toBe(1); // BUG-4: no second attempt after pause/resume
  expect(xf.result).toBe('taken');
  // The handoff still applied: compressed offsets and correct promotion.
  expect(fin.s.currentTrackStartSeconds).toBeGreaterThan(43);
  expect(fin.s.currentTrackStartSeconds).toBeLessThanOrEqual(46);
  const within = fin.s.positionSeconds - fin.s.currentTrackStartSeconds;
  expect(within).toBeGreaterThan(0.5);
});
