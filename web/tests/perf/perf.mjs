/*
 * MusicPack Phase 6 performance report.
 *
 * Measures the web client against the real server + wasm module and prints a
 * table: payload sizes, album-shelf first render, startup API requests,
 * artwork requests, Musepack time-to-first-PCM, compressed bytes fetched
 * before playback, ring bounds, and seek responsiveness.
 *
 * Usage: node perf.mjs            (starts its own server on 127.0.0.1:8099)
 *        node perf.mjs <baseUrl>  (uses an already-running server; reads the
 *                                 token from tests/e2e/.server-env.json)
 */
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, '../../..');
const E2E = path.join(here, '../e2e');

let base = process.argv[2];
let server = null;

if (!base) {
  server = spawn('bash', ['web/tests/e2e/start-server.sh'], { cwd: ROOT, stdio: 'ignore' });
  base = 'http://127.0.0.1:8099';
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(base + '/api/v1/health');
      if (r.ok) break;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 300));
  }
}

const env = JSON.parse(readFileSync(path.join(E2E, '.server-env.json'), 'utf8'));
const browser = await chromium.launch();

const report = {};
const reqCounts = {};
let apiRequests = 0;

function bytesOf(n) {
  return n >= 1048576 ? `${(n / 1048576).toFixed(2)} MiB` : `${Math.round(n / 1024)} KiB`;
}

async function measure() {
  const page = await browser.newPage();
  page.on('request', (req) => {
    const url = new URL(req.url());
    if (url.pathname.startsWith('/api/')) apiRequests++;
    if (url.pathname.startsWith('/api/v1/assets/')) reqCounts.artwork = (reqCounts.artwork ?? 0) + 1;
  });

  // ---- startup -------------------------------------------------------------
  const t0 = Date.now();
  await page.goto(base + '/');
  await page.getByLabel('Server token').fill(env.token);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('heading', { name: 'The shelf' }).waitFor({ timeout: 20000 });
  report.shelfFirstRenderMs = Date.now() - t0;
  report.startupApiRequests = apiRequests;

  // ---- shelf / artwork ----------------------------------------------------
  report.shelfAlbums = await page.locator('.album-card').count();
  await page.waitForTimeout(800);
  report.artworkRequests = reqCounts.artwork ?? 0;

  // ---- playback on the long (48 s) album ----------------------------------
  await page.getByText('Long Player').click();
  const openAlbumMs = Date.now();
  await page.getByRole('button', { name: 'Play album' }).click();
  const clickPlayMs = Date.now();

  // first position tick > 0 => first PCM rendered
  await page.waitForFunction(
    () => (window.__musicpack?.player.model.get().positionSeconds ?? 0) > 0.05,
    undefined,
    { timeout: 20000 },
  );
  report.timeToFirstPcmMs = Date.now() - clickPlayMs;
  report.albumOpenMs = clickPlayMs - openAlbumMs;

  const size = await page.evaluate(() => {
    const item = window.__musicpack?.player.model.get().current;
    return item?.track.audio.size ?? 0;
  });
  const early = await page.evaluate(() => window.__musicpack?.player.getServedBytes() ?? 0);
  report.bytesFetchedBeforePlayback = bytesOf(early);
  report.bytesBeforePlaybackFraction = size ? (early / size).toFixed(2) : 'n/a';

  // ---- seek responsiveness ------------------------------------------------
  const seekStart = Date.now();
  await page.locator('.playerbar input[type=range]').first().evaluate((el) => {
    const input = el;
    input.value = '30';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await page.waitForFunction(
    () => (window.__musicpack?.player.model.get().positionSeconds ?? 0) > 25,
    undefined,
    { timeout: 15000 },
  );
  report.seek30sMs = Date.now() - seekStart;

  // bytes after the 30 s seek (should stay far below the whole file)
  const afterSeek = await page.evaluate(() => window.__musicpack?.player.getServedBytes() ?? 0);
  report.bytesAfter30sSeek = bytesOf(afterSeek);
  report.fileSize = size ? bytesOf(size) : 'n/a';
  report.seekFetchFraction = size ? (afterSeek / size).toFixed(2) : 'n/a';

  // ---- memory --------------------------------------------------------------
  report.jsHeapMB = await page.evaluate(() => {
    const m = performance.memory;
    return m ? Math.round(m.usedJSHeapSize / 1048576) : null;
  });

  // ---- ring bounds (from the model, best available) -----------------------
  report.albumDurationSeconds = await page.evaluate(
    () => window.__musicpack?.player.model.get().durationSeconds ?? 0,
  );

  await page.close();
}

try {
  await measure();
} finally {
  await browser.close();
  if (server) server.kill();
}

// ---- payload sizes ---------------------------------------------------------
import { readdirSync } from 'node:fs';
const dist = path.join(here, '../../app/dist');
const assets = readdirSync(path.join(dist, 'assets'));
const js = readFileSync(path.join(dist, 'assets', assets.find((f) => f.startsWith('main-') && f.endsWith('.js'))));
const css = readFileSync(path.join(dist, 'assets', assets.find((f) => f.endsWith('.css'))));
const wasm = readFileSync(path.join(dist, 'musepack.wasm'));
report.jsGzip = bytesOf(js.length);
report.cssGzip = bytesOf(css.length);
report.wasm = bytesOf(wasm.length);

console.log('\n=== MusicPack Phase 6 performance report ===');
for (const [k, v] of Object.entries(report)) console.log(`  ${k}: ${v}`);
