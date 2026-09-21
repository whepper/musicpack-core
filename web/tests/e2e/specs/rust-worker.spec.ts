// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14E: real-browser verification of the Rust/WASM decoder worker behind
// the existing demand-range infrastructure. Additive and test-only: it does
// not touch the production playback path, the AudioWorklet, or the Emscripten
// engines.
//
// The Rust binding (glue + module bytes) is injected into the worker over
// postMessage, so no CSP-hostile blob/data worker is needed. The app serves
// the real `networker.js` and `reader_mailbox.js` from dist and owns HTTP Range,
// the 64 KiB block cache, and the SAB mailbox.

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { env } from './helpers';

const here = path.dirname(fileURLToPath(import.meta.url));
// The binding is generated into app/public/rust by scripts/build-wasm.mjs
// (R2 pipeline); the legacy sibling-checkout target/wasm-browser location
// remains as a fallback.
const wasmDir = [
  path.resolve(here, '../../../app/public/rust'),
  path.resolve(here, '../../../../../musicpack-core/target/wasm-browser'),
].find((dir) => existsSync(path.join(dir, 'musicpack_wasm.js')));
const wasmJsPath = path.join(wasmDir ?? '', 'musicpack_wasm.js');
const wasmBgPath = path.join(wasmDir ?? '', 'musicpack_wasm_bg.wasm');
const available = existsSync(wasmJsPath) && existsSync(wasmBgPath);
const glue = available ? readFileSync(wasmJsPath, 'utf8') : '';
const wasmB64 = available ? readFileSync(wasmBgPath).toString('base64') : '';

if (!available) {
  // eslint-disable-next-line no-console
  console.warn(
    'note: Rust worker e2e skipped (build musicpack-wasm with ' +
      '`wasm-bindgen --target no-modules --out-dir target/wasm-browser ...`)',
  );
}

const LONG_MUSEPACK_SHA = '42ff881bec6db89364a113213fe228e52981310d15c8f856a6a6af565343a7bf';

interface AudioRef {
  url: string;
  size: number;
}

function findAudio(node: unknown): AudioRef | null {
  if (!node || typeof node !== 'object') return null;
  const obj = node as Record<string, unknown>;
  if (typeof obj.url === 'string' && obj.url.includes('/audio') && typeof obj.size === 'number') {
    return { url: obj.url, size: obj.size };
  }
  for (const value of Object.values(obj)) {
    const found = Array.isArray(value)
      ? value.map(findAudio).find((r): r is AudioRef => r !== null) ?? null
      : findAudio(value);
    if (found) return found;
  }
  return null;
}

async function albumAudio(request: APIRequestContext, title: string): Promise<AudioRef> {
  const auth = { Authorization: `Bearer ${env.token}` };
  const list = await request.get(`${env.baseUrl}/api/v1/albums`, { headers: auth });
  expect(list.ok()).toBeTruthy();
  const page = (await list.json()) as { albums: Array<{ id: number; title: string }> };
  const album = page.albums.find((a) => a.title === title);
  expect(album, `album '${title}' present`).toBeTruthy();
  const detail = await request.get(`${env.baseUrl}/api/v1/albums/${album!.id}`, { headers: auth });
  expect(detail.ok()).toBeTruthy();
  const body = (await detail.json()) as { releases: Array<{ id: number }> };
  const release = body.releases?.[0];
  expect(release, `release in '${title}'`).toBeTruthy();
  const rel = await request.get(`${env.baseUrl}/api/v1/releases/${release!.id}`, { headers: auth });
  expect(rel.ok()).toBeTruthy();
  const ref = findAudio(await rel.json());
  expect(ref, `audio reference in '${title}'`).toBeTruthy();
  return ref!;
}

async function runRustWorker(page: Page, url: string, size: number) {
  return page.evaluate(
    async ({ url, size, token, origin, glue, wasmB64 }) => {
      const worker = new Worker(origin + '/rust-decoder-probe.js');
      const progress: unknown[] = [];
      const timeoutMs = 30000;
      const done = new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => resolve({ ok: false, message: `timeout after ${timeoutMs}ms progress=${JSON.stringify(progress)}`, progress }),
          timeoutMs,
        );
        worker.onmessage = (ev) => {
          if (ev.data && ev.data.progress) {
            progress.push(ev.data.progress);
            return;
          }
          clearTimeout(timer);
          resolve(ev.data);
        };
        worker.onerror = (e) => {
          clearTimeout(timer);
          reject(new Error(`worker error: ${e.message} progress=${JSON.stringify(progress)}`));
        };
      });
      worker.postMessage({ url, size, token, origin, glue, wasmB64 });
      return (await done) as Record<string, unknown>;
    },
    { url, size, token: env.token, origin: env.baseUrl, glue, wasmB64 },
  );
}

test.describe('Rust/WASM decoder worker (additive, test-only)', () => {
  test.skip(!available, 'musicpack-wasm browser build (target/wasm-browser) not present');

  test('Musepack: range-fed decode is byte-exact and never fetches the whole member at open', async ({
    page,
    request,
  }) => {
    const ref = await albumAudio(request, 'Long Player');
    const url = new URL(ref.url, env.baseUrl).href;
    await page.goto('/');
    const report = await runRustWorker(page, url, ref.size);
    // eslint-disable-next-line no-console
    console.log(
      `rust-worker: size=${report.size} open=${report.servedAtOpen} seek10=${report.servedAfterSeek} full=${report.servedAfter} reads=${report.calls} sha=${report.pcmSha}`,
    );
    expect(report.ok, String(report.message)).toBe(true);
    expect(report.error ?? null).toBeNull();
    const info = report.info as { rate: number; channels: number; lengthSamples: number };
    expect(info.rate).toBe(44100);
    expect(info.channels).toBe(2);
    expect(info.lengthSamples).toBe(2116800);
    expect(report.pcmSha).toBe(LONG_MUSEPACK_SHA);

    const size = report.size as number;
    expect(report.servedAtOpen as number).toBeLessThan(size);
    expect(report.servedAfterSeek as number).toBeLessThan(size);
    expect(report.servedAfter as number).toBeLessThanOrEqual(size);
    expect(report.calls as number).toBeGreaterThan(0);
  });

  test('repeated open/decode/close on fresh workers does not hang', async ({ page, request }) => {
    const ref = await albumAudio(request, 'Long Player');
    const url = new URL(ref.url, env.baseUrl).href;
    await page.goto('/');
    for (let i = 0; i < 3; i++) {
      const report = await runRustWorker(page, url, ref.size);
      expect(report.ok, String(report.message)).toBe(true);
    }
  });
});
