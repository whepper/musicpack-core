// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
//
// Production Rust playback worker (Phase 14H-1).
//
// Joins the proven Phase 14E/14D range source (real `networker.js` +
// `reader_mailbox.js` SAB mailbox) with the Phase 14C streaming decoder and
// the Phase 14F bounded PCM SharedArrayBuffer ring. PCM never crosses
// `postMessage`: it flows from this worker into the sink AudioWorklet through
// the ring. Control/state messages use `postMessage`.
//
// The adapter (`rust-playback-engine.ts`) owns the worker lifecycle; this file
// owns WASM, the range sources (one networker per URL, so a standby crossfade
// lane can decode a *different* track), the Rust engine, and the bounded pump.
// It performs no application decisions and never becomes the player.

'use strict';

const CTRL = {
  WRITE: 0,
  READ: 1,
  UNDERRUNS: 2,
  EOS: 3,
  PAUSED: 4,
  IDLE: 5,
  CAP: 6,
  MIN_OCC: 7,
  MAX_OCC: 8,
  GEN: 9,
};

const BLOCK = 1024; // frames per Rust render, matches Phase 14F
// "Primed" requires the ring to be within one block of full: the AudioContext
// resumes on the first `primed`, so a shallow prime would immediately underrun
// and pin the player in `buffering` (legacy decodes ahead the same way).
const PRIME_MARGIN = BLOCK;
const TICK_MS = 50; // throttled `rendered` ticks
const IDLE_WAIT_MS = 2; // capacity wait when the ring is full

let mailbox = null;
let wasm = null; // wasm_bindgen module
let engine = null; // WasmEngine over the range sources
let state = null; // Int32Array over the PCM control SAB
let ring = null; // Float32Array over the PCM data SAB
let cap = 0;
let channels = 2;
let generation = 0;
let pumping = false;
let primed = false;
let eosEmitted = false;
let xfadeActive = false;
let closed = false;
let loopStarted = false;
let lastUnderruns = 0;
let lastTick = 0;

/** url -> { worker, state, data, size } (one networker per source URL). */
const networkers = new Map();

// ---- offline (OPFS) sources ----------------------------------------------
//
// Offline items address a committed OPFS file by key (`source.kind ===
// 'local-file'`, `source.url === key`). The Rust engine sees exactly the same
// synchronous `read(url, offset, len)` range contract it uses online, so the
// same decoder serves both paths; only the byte source differs. The sync
// access handle is legal because this is a dedicated worker that already uses
// SharedArrayBuffer for the online path.
const OPFS_ROOT = 'musicpack-offline-v1';
const OPFS_RELEASES = 'releases';
/** Bytes per OPFS read (mirrors the Rust reader's 64 KiB window). */
const OPFS_READ_CHUNK = 64 * 1024;

/** key -> { access, size, read } (one sync access handle per installed file). */
const locals = new Map();

async function openLocal(key, size) {
  const existing = locals.get(key);
  if (existing) return existing;
  const root = await navigator.storage.getDirectory();
  const base = await root.getDirectoryHandle(OPFS_ROOT);
  const releases = await base.getDirectoryHandle(OPFS_RELEASES);
  const fh = await releases.getFileHandle(key, { create: false });
  const access = await fh.createSyncAccessHandle();
  const actual = access.getSize();
  if (size > 0 && actual !== size) {
    access.close();
    throw new Error('local file size mismatch: ' + actual + ' != ' + size);
  }
  const entry = { access, size: actual, read: 0 };
  locals.set(key, entry);
  return entry;
}

function closeLocals() {
  for (const entry of locals.values()) {
    try {
      entry.access.close();
    } catch {
      /* already gone */
    }
  }
  locals.clear();
}

/** Synchronous OPFS range read; only invoked from inside `engine.render()`. */
function readLocal(key, offset, len) {
  const entry = locals.get(key);
  if (!entry) throw new Error('no local source for ' + key);
  const want = Math.min(len, OPFS_READ_CHUNK, entry.size - offset);
  if (want <= 0) return new Uint8Array(0);
  const scratch = new Uint8Array(want);
  const got = entry.access.read(scratch, { at: offset });
  if (got <= 0) return new Uint8Array(0);
  entry.read += got;
  return got === want ? scratch : scratch.subarray(0, got);
}

/** Opens the byte source for an item: OPFS for `local-file`, HTTP otherwise. */
async function openSource(kind, url, size, token) {
  if (kind === 'local-file') {
    await openLocal(url, size);
    return;
  }
  await openNetworker(url, size, token);
}

function post(message) {
  self.postMessage(message);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function b64ToBytes(text) {
  const bin = atob(text);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function ensureWasm(assets) {
  if (wasm) return;
  if (assets && assets.glue && assets.wasmB64) {
    // Lower-level test path (14H-1 hook): glue + module bytes injected.
    wasm = new Function(assets.glue + '\n;return wasm_bindgen;')();
    wasm.initSync({ module: new WebAssembly.Module(b64ToBytes(assets.wasmB64)) });
    return;
  }
  // Production/dev path: `scripts/build-wasm.mjs` generates the no-modules
  // glue and module under /rust/. The glue declares a global lexical
  // `wasm_bindgen`; `new Function` reads it from global scope.
  importScripts('rust/musicpack_wasm.js');
  const response = await fetch('rust/musicpack_wasm_bg.wasm');
  if (!response.ok) {
    throw new Error('Rust WASM asset unavailable (HTTP ' + response.status + ')');
  }
  wasm = new Function('return wasm_bindgen;')();
  wasm.initSync({ module: new WebAssembly.Module(await response.arrayBuffer()) });
}

function ensureMailbox() {
  if (!mailbox) {
    importScripts('reader_mailbox.js');
    mailbox = self.MusicPackMailbox;
  }
  return mailbox;
}

async function openNetworker(url, size, token) {
  const M = ensureMailbox();
  const existing = networkers.get(url);
  if (existing) return existing;
  const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);
  const st = new Int32Array(sab);
  const dt = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
  const worker = new Worker('networker.js');
  const ready = new Promise((resolve, reject) => {
    worker.onmessage = (ev) => {
      if (ev.data && ev.data.type === 'ready') resolve();
    };
    worker.onerror = (ev) => reject(new Error('networker: ' + (ev.message || 'error')));
  });
  worker.postMessage({ type: 'open', sab, url, size, token });
  await ready;
  const entry = { worker, state: st, data: dt, size };
  networkers.set(url, entry);
  return entry;
}

function closeNetworkers() {
  for (const entry of networkers.values()) {
    try {
      entry.worker.terminate();
    } catch {
      /* already gone */
    }
  }
  networkers.clear();
}

/** Releases every byte source (HTTP networkers + OPFS handles). */
function closeSources() {
  closeNetworkers();
  closeLocals();
}

/** Synchronous range read, invoked only from inside `engine.render()` here.
 *  Dispatches on the URL: an installed OPFS key reads from its sync access
 *  handle, anything else from its HTTP networker. */
function readRange(url, offset, len) {
  if (locals.has(url)) return readLocal(url, offset, len);
  const entry = networkers.get(url);
  if (!entry) throw new Error('no range source for ' + url);
  const M = mailbox;
  const want = Math.min(len, M.DATA_CAP, entry.size - offset);
  if (want <= 0) return new Uint8Array(0);
  const st = entry.state;
  Atomics.store(st, M.POS_LO, offset >>> 0);
  Atomics.store(st, M.POS_HI, Math.floor(offset / 4294967296));
  Atomics.store(st, M.LEN, want);
  Atomics.store(st, M.ERROR, 0);
  Atomics.store(st, M.RES, 0);
  Atomics.store(st, M.REQ, 1);
  Atomics.notify(st, M.REQ);
  while (Atomics.load(st, M.RES) === 0) Atomics.wait(st, M.RES, 0);
  if (Atomics.load(st, M.ERROR) !== 0) {
    const code = Atomics.load(st, M.ERROR);
    Atomics.store(st, M.RES, 0);
    throw new Error('range error ' + code);
  }
  const n = Atomics.load(st, M.DONE_LEN);
  const out = new Uint8Array(n);
  out.set(entry.data.subarray(0, n));
  Atomics.store(st, M.RES, 0);
  return out;
}

function servedBytes() {
  let total = 0;
  for (const entry of networkers.values()) total += Atomics.load(entry.state, mailbox.SERVED);
  for (const entry of locals.values()) total += entry.read;
  return total;
}

function resetRing() {
  Atomics.store(state, CTRL.WRITE, 0);
  Atomics.store(state, CTRL.READ, 0);
  Atomics.store(state, CTRL.EOS, 0);
  Atomics.store(state, CTRL.UNDERRUNS, 0);
  Atomics.store(state, CTRL.MIN_OCC, 0x7fffffff);
  Atomics.store(state, CTRL.MAX_OCC, 0);
  Atomics.store(state, CTRL.GEN, generation);
  lastUnderruns = 0;
}

async function handleOpen(d) {
  generation = d.generation;
  channels = d.channels || 2;
  cap = d.frames;
  state = new Int32Array(d.control);
  ring = new Float32Array(d.data);
  Atomics.store(state, CTRL.CAP, cap);
  pumping = false;
  primed = false;
  eosEmitted = false;
  xfadeActive = false;
  lastTick = 0;
  resetRing();
  if (engine) {
    try {
      engine.close();
    } catch {
      /* already closed */
    }
    engine = null;
  }
  closeSources();
  await ensureWasm(d.assets);
  await openSource(d.kind, d.url, d.size, d.token);
  engine = wasm.WasmEngine.newRangeSource(d.rate, channels, readRange);
  const info = JSON.parse(engine.open(d.itemJson));
  post({ type: 'opened', generation, info });
  startLoop();
}

function startLoop() {
  if (loopStarted) return;
  loopStarted = true;
  void pumpLoop();
}

async function pumpLoop() {
  for (;;) {
    if (closed) return;
    if (!engine || !state) {
      await sleep(5);
      continue;
    }
    const underruns = Atomics.load(state, CTRL.UNDERRUNS);
    if (underruns !== lastUnderruns) {
      lastUnderruns = underruns;
      // Allow a fresh `primed` once the ring refills so the host can leave
      // the buffering state (an underrun is not terminal).
      primed = false;
      post({ type: 'buffering', generation });
    }
    if (!pumping) {
      await sleep(5);
      continue;
    }
    // During an overlapped fade the outgoing session may drain while the
    // incoming lane is still being mixed, so its drained state is NOT the
    // track's end: keep rendering until the mixer completes the swap.
    if (!xfadeActive && engine.is_output_drained()) {
      if (Atomics.load(state, CTRL.READ) >= Atomics.load(state, CTRL.WRITE)) {
        if (!eosEmitted) {
          eosEmitted = true;
          Atomics.store(state, CTRL.EOS, 1);
          post({ type: 'eos', generation });
        }
      }
      await sleep(10);
      continue;
    }
    const write = Atomics.load(state, CTRL.WRITE);
    const read = Atomics.load(state, CTRL.READ);
    if (cap - (write - read) >= BLOCK) {
      let block;
      try {
        block = engine.render(BLOCK);
      } catch (err) {
        post({ type: 'error', generation, message: String((err && err.message) || err) });
        pumping = false;
        await sleep(10);
        continue;
      }
      const ringFloats = cap * channels;
      let idx = (write % cap) * channels;
      for (let i = 0; i < block.length; i++) {
        ring[idx] = block[i];
        idx += 1;
        if (idx === ringFloats) idx = 0;
      }
      Atomics.store(state, CTRL.WRITE, write + BLOCK);
      Atomics.notify(state, CTRL.IDLE);
      const error = engine.take_error();
      if (error) post({ type: 'error', generation, message: error });
      const crossfade = engine.take_crossfade_result();
      if (crossfade) {
        xfadeActive = false;
        primed = false;
        eosEmitted = false;
        post({ type: 'crossfade', generation, status: 'completed', result: JSON.parse(crossfade) });
      }
      const occupancy = write + BLOCK - Atomics.load(state, CTRL.READ);
      if (!primed && occupancy >= cap - PRIME_MARGIN) {
        primed = true;
        post({ type: 'primed', generation });
      }
      const now = Date.now();
      if (now - lastTick >= TICK_MS) {
        lastTick = now;
        post({
          type: 'rendered',
          generation,
          samples: engine.rendered_samples(),
          served: servedBytes(),
          read: Atomics.load(state, CTRL.READ),
          write: Atomics.load(state, CTRL.WRITE),
        });
      }
    } else {
      await sleep(IDLE_WAIT_MS);
    }
  }
}

async function handleCommand(d) {
  switch (d.type) {
    case 'open':
      await handleOpen(d);
      return;
    case 'startPumping':
      pumping = true;
      if (engine) engine.start();
      return;
    case 'pausePumping':
      pumping = false;
      if (engine) engine.stop();
      return;
    case 'play':
      if (engine) engine.play();
      return;
    case 'pause':
      if (engine) engine.pause();
      return;
    case 'seek': {
      if (!engine) return;
      generation = d.generation;
      pumping = false;
      primed = false;
      eosEmitted = false;
      xfadeActive = false;
      engine.seek(d.samples);
      resetRing();
      post({ type: 'seeked', generation, samples: engine.rendered_samples() });
      return;
    }
    case 'setGain':
      if (engine) engine.set_gain(d.linear);
      return;
    case 'prepareNext': {
      if (!engine) {
        post({ type: 'prepared', generation, info: null });
        return;
      }
      await openSource(d.kind, d.url, d.size, d.token);
      const info = engine.prepare_next(d.itemJson);
      post({ type: 'prepared', generation, info: info ? JSON.parse(info) : null });
      return;
    }
    case 'advance': {
      if (!engine) {
        post({ type: 'advanced', generation, info: null });
        return;
      }
      const info = engine.advance(d.expectedJson);
      // A null result means nothing was promoted (end of queue): the current
      // session is unchanged and may still be drained, so keep the EOS latch
      // so the engine reports the same drained state rather than re-firing.
      if (info) {
        primed = false;
        eosEmitted = false;
      }
      post({ type: 'advanced', generation, info: info ? JSON.parse(info) : null });
      return;
    }
    case 'beginCrossfade': {
      if (!engine) {
        post({ type: 'crossfade', generation, status: 'declined' });
        return;
      }
      await openSource(d.kind, d.url, d.size, d.token);
      const tag = engine.begin_crossfade(d.itemJson, d.fadeSeconds);
      if (tag === 'declined') {
        xfadeActive = false;
        post({ type: 'crossfade', generation, status: 'declined' });
        return;
      }
      if (tag === 'completed') {
        const result = engine.take_crossfade_result();
        xfadeActive = false;
        primed = false;
        eosEmitted = false;
        post({
          type: 'crossfade',
          generation,
          status: 'completed',
          result: result ? JSON.parse(result) : null,
        });
        return;
      }
      xfadeActive = true;
      post({ type: 'crossfade', generation, status: 'pending' });
      return;
    }
    case 'close': {
      closed = true;
      pumping = false;
      if (engine) {
        try {
          engine.close();
        } catch {
          /* already closed */
        }
        engine = null;
      }
      closeSources();
      post({ type: 'closed', generation });
      self.close();
      return;
    }
    default:
      return;
  }
}

self.onmessage = (ev) => {
  const data = ev.data || {};
  Promise.resolve()
    .then(() => handleCommand(data))
    .catch((err) => {
      // A failed open/render/lane command must surface to the host instead of
      // leaving the adapter's pending promise unresolved (which would hang the
      // player in loading). Report against the command's generation.
      post({
        type: 'error',
        generation: typeof data.generation === 'number' ? data.generation : generation,
        message: String((err && err.message) || err),
      });
    });
};
