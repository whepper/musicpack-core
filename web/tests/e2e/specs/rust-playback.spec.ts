// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14H-1: real Chromium smoke test for the production Rust playback
// adapter, constructed through the application's own `createWebEngine('rust')`
// seam (exposed as a test hook at `window.__musicpack.createRustEngine`).
//
// System under test: app bundle -> createWebEngine -> RustPlaybackEngine ->
// rust-playback.worker.js -> WASM/Rust + real range source -> bounded SAB ring
// -> rust-playback-sink AudioWorklet -> AudioContext.
//
// It is additive and test-only: normal application configuration still selects
// the legacy backend (chooseBackend never returns 'rust').

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type APIRequestContext } from '@playwright/test';
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

const RING_FRAMES = 16384;

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
      ? (value.map(findAudio).find((r): r is AudioRef => r !== null) ?? null)
      : findAudio(value);
    if (found) return found;
  }
  return null;
}

function collectAudios(node: unknown, out: AudioRef[] = []): AudioRef[] {
  if (!node || typeof node !== 'object') return out;
  const obj = node as Record<string, unknown>;
  if (typeof obj.url === 'string' && obj.url.includes('/audio') && typeof obj.size === 'number') {
    out.push({ url: obj.url, size: obj.size });
  }
  for (const value of Object.values(obj)) {
    if (Array.isArray(value)) value.forEach((v) => collectAudios(v, out));
    else collectAudios(value, out);
  }
  return out;
}

async function albumAudios(request: APIRequestContext, title: string): Promise<AudioRef[]> {
  const auth = { Authorization: `Bearer ${env.token}` };
  const list = await request.get(`${env.baseUrl}/api/v1/albums`, { headers: auth });
  const page = (await list.json()) as { albums: Array<{ id: number; title: string }> };
  const album = page.albums.find((a) => a.title === title);
  expect(album, `album '${title}' present`).toBeTruthy();
  const detail = await request.get(`${env.baseUrl}/api/v1/albums/${album!.id}`, { headers: auth });
  const body = (await detail.json()) as { releases: Array<{ id: number }> };
  const rel = await request.get(`${env.baseUrl}/api/v1/releases/${body.releases[0]!.id}`, { headers: auth });
  return collectAudios(await rel.json());
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

interface SmokeReport {
  info: { rate: number; channels: number; version: number; lengthSamples: number } | null;
  sawPcm: boolean;
  firstPcmMs: number;
  producedPlaying: number;
  consumedPlaying: number;
  minOcc: number;
  maxOcc: number;
  underruns: number;
  consumedAfterPause: number;
  consumedAfterPause2: number;
  consumedAfterResume: number;
  seekProduced: number;
  seekConsumed: number;
  seekThenProduced: number;
  events: string[];
  errors: string[];
}

test.describe('Rust playback adapter via the application seam (Phase 14H-1)', () => {
  test.skip(!available, 'musicpack-wasm browser build (target/wasm-browser) not present');
  test.setTimeout(120_000);

  test('construct -> open -> play -> pause -> resume -> seek -> close', async ({ page, request }) => {
    const ref = await albumAudio(request, 'Long Player');
    const url = new URL(ref.url, env.baseUrl).href;

    await page.goto('/');
    await page.waitForFunction(() => Boolean(window.__musicpack?.createRustEngine));
    await page.mouse.click(5, 5); // autoplay gesture for AudioContext.resume()

    const report = (await page.evaluate(
      async ({ url, size, token, glue, wasmB64, ringFrames }) => {
        const errors: string[] = [];
        const events: string[] = [];
        const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
        const waitFor = async (fn: () => boolean, ms: number) => {
          const t0 = performance.now();
          while (performance.now() - t0 < ms) {
            if (fn()) return true;
            await sleep(25);
          }
          return false;
        };

        const engine = window.__musicpack!.createRustEngine({ glue, wasmB64 });
        engine.on('primed', () => events.push('primed'));
        engine.on('buffering', () => events.push('buffering'));
        engine.on('eos', () => events.push('eos'));
        engine.on('tick', () => events.push('tick'));
        engine.on('error', () => events.push('error'));

        let info: SmokeReport['info'] = null;
        const tStart = performance.now();
        let firstPcmMs = 0;
        try {
          await engine.init(token);
          info = await engine.open({
            id: 'e2e-rust',
            trackId: 1,
            source: { kind: 'http-range', url, byteSize: size },
            title: 'Rust Smoke',
            artist: 'E2E',
            albumTitle: 'E2E',
            codec: 'musepack-sv8',
          } as never);
          await engine.startPumping();
          await engine.play();

          const sawPcm = await waitFor(
            () => engine.renderedSamples() > 44100 && engine.getDiagnostics().read > 0,
            30000,
          );
          firstPcmMs = performance.now() - tStart;
          const producedPlaying = engine.renderedSamples();
          const consumedPlaying = engine.getDiagnostics().read;
          const maxOcc = engine.getDiagnostics().maxOcc;

          await engine.pause();
          await sleep(350);
          const consumedAfterPause = engine.getDiagnostics().read;
          await sleep(250);
          const consumedAfterPause2 = engine.getDiagnostics().read;
          await engine.play();
          await sleep(400);
          const consumedAfterResume = engine.getDiagnostics().read;
          const minOcc = engine.getDiagnostics().minOcc;

          // Seek in the middle: the old generation is invalidated and the ring
          // is flushed, then playback resumes from the new position.
          const beforeSeek = engine.getDiagnostics().write;
          void beforeSeek;
          await engine.seekSample(1_000_000);
          const seekProduced = engine.renderedSamples();
          const seekConsumed = engine.getDiagnostics().read;
          await engine.startPumping();
          await engine.play();
          await waitFor(() => engine.getDiagnostics().read > 0, 30000);
          const seekThenProduced = engine.renderedSamples();

          await engine.close();
          return {
            info,
            sawPcm,
            firstPcmMs,
            producedPlaying,
            consumedPlaying,
            minOcc,
            maxOcc,
            underruns: engine.getDiagnostics().underruns,
            consumedAfterPause,
            consumedAfterPause2,
            consumedAfterResume,
            seekProduced,
            seekConsumed,
            seekThenProduced,
            events,
            errors,
          } as SmokeReport;
        } catch (err) {
          errors.push(String((err as Error)?.message ?? err));
          try {
            await engine.close();
          } catch {
            /* already closed */
          }
          return {
            info,
            sawPcm: false,
            firstPcmMs,
            producedPlaying: engine.renderedSamples(),
            consumedPlaying: 0,
            minOcc: 0,
            maxOcc: 0,
            underruns: 0,
            consumedAfterPause: 0,
            consumedAfterPause2: 0,
            consumedAfterResume: 0,
            seekProduced: 0,
            seekConsumed: 0,
            seekThenProduced: 0,
            events,
            errors,
          } as SmokeReport;
        }
      },
      { url, size: ref.size, token: env.token, glue, wasmB64, ringFrames: RING_FRAMES },
    )) as SmokeReport;

    // eslint-disable-next-line no-console
    console.log(`rust-playback: ${JSON.stringify(report)}`);

    expect(report.errors).toEqual([]);
    // The adapter renders at the AudioContext output rate (device-dependent:
    // 44100 or 48000), exactly as the legacy engine normalizes source rate.
    expect(report.info?.rate).toBeGreaterThan(0);
    expect(report.info?.channels).toBe(2);
    expect(report.info?.lengthSamples).toBe(Math.round(48 * (report.info?.rate ?? 0)));
    expect(report.sawPcm).toBe(true);
    expect(report.consumedPlaying).toBeGreaterThan(0);
    expect(report.maxOcc).toBeLessThanOrEqual(RING_FRAMES);
    // Pause freezes consumption; resume advances it.
    expect(report.consumedAfterPause2).toBe(report.consumedAfterPause);
    expect(report.consumedAfterResume).toBeGreaterThan(report.consumedAfterPause);
    // Seek invalidates the old generation: consumed resets, then advances again.
    expect(report.seekProduced).toBe(0);
    expect(report.seekConsumed).toBe(0);
    expect(report.seekThenProduced).toBeGreaterThan(0);
    expect(report.events).toContain('primed');
  });

  test('preload a second range source and advance onto it (standby path)', async ({ page, request }) => {
    const audios = await albumAudios(request, 'Fade Rider');
    expect(audios.length).toBeGreaterThanOrEqual(2);
    const a = new URL(audios[0]!.url, env.baseUrl).href;
    const b = new URL(audios[1]!.url, env.baseUrl).href;

    await page.goto('/');
    await page.waitForFunction(() => Boolean(window.__musicpack?.createRustEngine));
    await page.mouse.click(5, 5);

    const report = (await page.evaluate(
      async ({ a, aSize, b, bSize, token, glue, wasmB64 }) => {
        const errors: string[] = [];
        const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
        const waitFor = async (fn: () => boolean, ms: number) => {
          const t0 = performance.now();
          while (performance.now() - t0 < ms) {
            if (fn()) return true;
            await sleep(25);
          }
          return false;
        };
        const engine = window.__musicpack!.createRustEngine({ glue, wasmB64 });
        engine.on('error', () => errors.push('error'));
        const mk = (url: string, size: number, id: string, trackId: number) => ({
          id,
          trackId,
          source: { kind: 'http-range' as const, url, byteSize: size },
          title: id,
          artist: 'E2E',
          albumTitle: 'E2E',
          codec: 'musepack-sv8',
        });
        let firstInfo: unknown = null;
        let preparedInfo: unknown = null;
        let advancedInfo: unknown = null;
        try {
          await engine.init(token);
          firstInfo = await engine.open(mk(a, aSize, 'e2e-a', 1) as never);
          await engine.startPumping();
          await engine.play();
          await waitFor(() => engine.renderedSamples() > 44100, 30000);
          preparedInfo = await engine.prepareNext(mk(b, bSize, 'e2e-b', 2) as never);
          advancedInfo = await engine.advance(mk(b, bSize, 'e2e-b', 2) as never);
          await engine.startPumping();
          await engine.play();
          const advanced = await waitFor(() => engine.renderedSamples() > 22050, 30000);
          errors.push(advanced ? 'advanced' : 'not-advanced');
          await engine.close();
        } catch (err) {
          errors.push('throw:' + String((err as Error)?.message ?? err));
        }
        return { firstInfo, preparedInfo, advancedInfo, errors };
      },
      { a, aSize: audios[0]!.size, b, bSize: audios[1]!.size, token: env.token, glue, wasmB64 },
    )) as {
      firstInfo: { rate: number; lengthSamples: number } | null;
      preparedInfo: { rate: number; lengthSamples: number } | null;
      advancedInfo: { rate: number; lengthSamples: number } | null;
      errors: string[];
    };

    // eslint-disable-next-line no-console
    console.log(`rust-playback[standby]: ${JSON.stringify(report)}`);
    expect(report.errors).toEqual(['advanced']);
    expect(report.firstInfo?.lengthSamples).toBeGreaterThan(0);
    expect(report.preparedInfo?.lengthSamples).toBeGreaterThan(0);
    expect(report.advancedInfo?.lengthSamples).toBeGreaterThan(0);
  });
});
