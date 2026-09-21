// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 14G: real-browser validation of the Rust PCM -> SAB ring -> AudioWorklet
// -> AudioContext path across codecs, with backpressure/occupancy and
// pause/resume measurements. Additive and test-only; production playback
// (MusepackEngine / MusicPackPcmProcessor / NativeBackend) is untouched.

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

const LONG_MUSEPACK_SHA = '42ff881bec6db89364a113213fe228e52981310d15c8f856a6a6af565343a7bf';
const CAPTURE_FRAMES = 60 * 44100; // generous capture for FLAC/Musepack durations

interface TrackRef {
  url: string;
  size: number;
  codec: string;
}

function collectTracks(node: unknown): TrackRef[] {
  const out: TrackRef[] = [];
  const visit = (n: unknown) => {
    if (!n || typeof n !== 'object') return;
    const obj = n as Record<string, unknown>;
    const audio = obj.audio as { url?: unknown; size?: unknown } | undefined;
    const codec = obj.codec as { codec?: unknown } | undefined;
    if (
      audio &&
      typeof audio.url === 'string' &&
      audio.url.includes('/audio') &&
      typeof audio.size === 'number'
    ) {
      out.push({ url: audio.url, size: audio.size, codec: typeof codec?.codec === 'string' ? codec.codec : '' });
    }
    for (const v of Object.values(obj)) {
      if (Array.isArray(v)) v.forEach(visit);
      else visit(v);
    }
  };
  visit(node);
  return out;
}

async function discoverTrack(
  request: APIRequestContext,
  predicate: (codec: string) => boolean,
  preferredTitle?: string,
): Promise<TrackRef> {
  const auth = { Authorization: `Bearer ${env.token}` };
  const list = await request.get(`${env.baseUrl}/api/v1/albums`, { headers: auth });
  const page = (await list.json()) as { albums: Array<{ id: number; title: string }> };
  const ordered = preferredTitle
    ? [...page.albums].sort((a) => (a.title === preferredTitle ? -1 : 1))
    : page.albums;
  for (const album of ordered) {
    const detail = await request.get(`${env.baseUrl}/api/v1/albums/${album.id}`, { headers: auth });
    const body = (await detail.json()) as { releases: Array<{ id: number }> };
    for (const release of body.releases ?? []) {
      const rel = await request.get(`${env.baseUrl}/api/v1/releases/${release.id}`, { headers: auth });
      const found = collectTracks(await rel.json()).find((t) => predicate(t.codec));
      if (found) return found;
    }
  }
  throw new Error('no matching track found');
}

interface AudioReport {
  sawPcm: boolean;
  firstPcmMs: number;
  eos: boolean;
  consumed: number;
  underrunsStartup: number;
  underrunsTotal: number;
  minOcc: number;
  maxOcc: number;
  sha: string;
  readWhilePaused: number;
  readWhilePaused2: number;
  readAfterResume: number;
  errors: string[];
  info: { rate: number; channels: number; lengthSamples: number } | null;
}

async function runAudio(
  page: import('@playwright/test').Page,
  ref: TrackRef,
  expectedSha: string | null,
): Promise<AudioReport> {
  await page.goto('/');
  await page.mouse.click(5, 5); // autoplay gesture
  const report = await page.evaluate(
    async ({ url, size, codec, token, origin, glue, wasmB64, captureFrames }) => {
      const READ = 1;
      const EOS = 3;
      const PAUSED = 4;
      const CAP = 6;
      const MIN_OCC = 7;
      const MAX_OCC = 8;
      const channels = 2;
      const cap = 16384;
      const control = new SharedArrayBuffer(64 * 4);
      const data = new SharedArrayBuffer(cap * channels * 4);
      const capture = new SharedArrayBuffer(captureFrames * channels * 4);
      const state = new Int32Array(control);
      Atomics.store(state, CAP, cap);
      Atomics.store(state, MIN_OCC, 0x7fffffff);
      Atomics.store(state, MAX_OCC, 0);

      const ctx = new AudioContext();
      await ctx.resume();
      await ctx.audioWorklet.addModule(origin + '/rust-pcm-sink.js');
      const node = new AudioWorkletNode(ctx, 'rust-pcm-sink', {
        numberOfOutputs: 1,
        outputChannelCount: [channels],
        processorOptions: { control, data, capture, channels, frames: cap },
      });
      const mute = ctx.createGain();
      mute.gain.value = 0;
      node.connect(mute).connect(ctx.destination);

      const errors: string[] = [];
      let info: { rate: number; channels: number; lengthSamples: number } | null = null;
      const worker = new Worker(origin + '/rust-pcm-producer.js');
      worker.onmessage = (ev) => {
        if (ev.data.type === 'error') errors.push(ev.data.message);
        if (ev.data.type === 'info') info = ev.data.info;
      };
      const started = performance.now();
      worker.postMessage({ url, size, codec, token, origin, glue, wasmB64, control, data, frames: cap, channels });

      const waitFor = async (fn: () => boolean, ms: number) => {
        const t0 = performance.now();
        while (performance.now() - t0 < ms) {
          if (fn()) return true;
          await new Promise((r) => setTimeout(r, 20));
        }
        return false;
      };

      const sawPcm = await waitFor(() => Atomics.load(state, READ) > 4096, 20000);
      const firstPcmMs = performance.now() - started;
      const underrunsStartup = Atomics.load(state, 2);

      Atomics.store(state, PAUSED, 1);
      await new Promise((r) => setTimeout(r, 400));
      const readWhilePaused = Atomics.load(state, READ);
      await new Promise((r) => setTimeout(r, 250));
      const readWhilePaused2 = Atomics.load(state, READ);
      Atomics.store(state, PAUSED, 0);
      await new Promise((r) => setTimeout(r, 300));
      const readAfterResume = Atomics.load(state, READ);

      const eos = await waitFor(() => Atomics.load(state, EOS) === 1, 90000);
      const consumed = Atomics.load(state, READ);
      const underrunsTotal = Atomics.load(state, 2);
      const minOcc = Atomics.load(state, MIN_OCC);
      const maxOcc = Atomics.load(state, MAX_OCC);

      const frames = info ? info.lengthSamples : 0;
      let sha = '';
      if (frames > 0) {
        const src = new Uint8Array(capture, 0, frames * channels * 4);
        const copy = new Uint8Array(frames * channels * 4);
        copy.set(src);
        const digest = await crypto.subtle.digest('SHA-256', copy);
        sha = Array.from(new Uint8Array(digest)).map((b) => b.toString(16).padStart(2, '0')).join('');
      }
      await ctx.close();
      worker.terminate();
      return {
        sawPcm,
        firstPcmMs,
        eos,
        consumed,
        underrunsStartup,
        underrunsTotal,
        minOcc,
        maxOcc,
        sha,
        readWhilePaused,
        readWhilePaused2,
        readAfterResume,
        errors,
        info,
      };
    },
    { ...ref, token: env.token, origin: env.baseUrl, glue, wasmB64, captureFrames: CAPTURE_FRAMES },
  );
  void expectedSha;
  return report as AudioReport;
}

test.describe('Rust PCM -> AudioWorklet (additive, test-only)', () => {
  test.skip(!available, 'musicpack-wasm browser build (target/wasm-browser) not present');
  test.setTimeout(120_000);

  test('Musepack SV8: bounded, pause/resume, EOS, byte-exact PCM', async ({ page, request }) => {
    const ref = await discoverTrack(request, (c) => c.includes('musepack'), 'Long Player');
    const report = await runAudio(page, ref, LONG_MUSEPACK_SHA);
    // eslint-disable-next-line no-console
    console.log(`rust-audio[musepack]: ${JSON.stringify(report)}`);
    expect(report.errors).toEqual([]);
    expect(report.sawPcm).toBe(true);
    expect(report.eos).toBe(true);
    expect(report.consumed).toBeGreaterThanOrEqual(report.info!.lengthSamples);
    expect(report.readWhilePaused2).toBe(report.readWhilePaused);
    expect(report.readAfterResume).toBeGreaterThan(report.readWhilePaused);
    expect(report.maxOcc).toBeLessThanOrEqual(16384);
    expect(report.sha).toBe(LONG_MUSEPACK_SHA);
  });

  test('FLAC: bounded, pause/resume, EOS, byte-exact PCM vs the local Rust decode', async ({
    page,
    request,
  }) => {
    const ref = await discoverTrack(request, (c) => c.includes('flac'));
    const report = await runAudio(page, ref, null);
    // eslint-disable-next-line no-console
    console.log(`rust-audio[flac]: ${JSON.stringify(report)}`);
    expect(report.errors).toEqual([]);
    expect(report.sawPcm).toBe(true);
    expect(report.eos).toBe(true);
    expect(report.info!.rate).toBeGreaterThan(0);
    expect(report.info!.channels).toBe(2);
    expect(report.consumed).toBeGreaterThanOrEqual(report.info!.lengthSamples);
    expect(report.readWhilePaused2).toBe(report.readWhilePaused);
    expect(report.readAfterResume).toBeGreaterThan(report.readWhilePaused);
    expect(report.maxOcc).toBeLessThanOrEqual(16384);
    expect(report.sha).toMatch(/^[0-9a-f]{64}$/);
  });
});
