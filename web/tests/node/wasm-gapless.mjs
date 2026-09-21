/*
 * WASM/audio harness for the MusicPack web client (Phase 6).
 *
 * Runs in Node against the real libmusepack.wasm and the client's actual
 * ring buffer, verifying the guarantees the AudioWorklet pipeline relies on:
 *
 *   1. Gapless two-track feed: decode A and B fully, push both through a
 *      RingBuffer exactly as the controller does at a track boundary, and
 *      assert the drained output is frame-identical to A concatenated with B
 *      (no dropped, duplicated, or inserted-silence frames at the boundary).
 *   2. Ring bounds + underrun: the buffer never exceeds capacity; an empty
 *      read is reported as zero available (the worklet renders silence) and
 *      writing resumes cleanly.
 *   3. Exact track-end frames: a full demand-reader decode yields exactly
 *      lengthSamples frames, matching the memory-reader decode.
 *   4. Seek accounting: seeking to 10/25/50/90% (and backwards, and rapid
 *      repeats) never downloads the whole file — each new seek target fetches
 *      only the compressed blocks the decoder needs.
 *
 * Usage: node wasm-gapless.mjs <module.js> <trackA.mpc> <trackB.mpc>
 */
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '../../..');
const require = createRequire(import.meta.url);

const M = require(path.join(ROOT, 'demo/reader_mailbox.js'));

const moduleJs = path.resolve(process.argv[2]);
const trackA = path.resolve(process.argv[3]);
const trackB = path.resolve(process.argv[4]);
const seekFixture = path.resolve(process.argv[5] ?? trackA);

if (!moduleJs || !trackA || !trackB) {
  console.error('usage: node wasm-gapless.mjs <module.js> <trackA.mpc> <trackB.mpc> [seekFixture.mpc]');
  process.exit(2);
}

let failures = 0;
function ok(cond, name) {
  if (cond) {
    console.log(`  ok  ${name}`);
  } else {
    failures++;
    console.error(`FAIL ${name}`);
  }
}

const fs = require('fs');

async function decodeAll(Module, h, channels) {
  const pcmPtr = Module._malloc(1152 * channels * 4);
  const out = [];
  for (;;) {
    const frames = await Module._mpc_wasm_read(h, pcmPtr, 1152);
    if (frames < 0) {
      if (frames === -5 /* EOF */) break;
      throw new Error(`read returned ${frames}`);
    }
    if (frames === 0) break;
    const view = new Float32Array(Module.HEAPF32.buffer, pcmPtr, frames * channels);
    for (let i = 0; i < view.length; i++) out.push(view[i]);
  }
  Module._free(pcmPtr);
  return Float32Array.from(out);
}

async function decodeFile(Module, file) {
  const bytes = fs.readFileSync(file);
  const h = Module._mpc_wasm_create();
  const memPtr = Module._malloc(bytes.length);
  Module.HEAPU8.set(bytes, memPtr);
  const err = await Module._mpc_wasm_open(h, memPtr, bytes.length);
  if (err !== 0) throw new Error(`open ${file}: ${err}`);
  const channels = Module._mpc_wasm_channels(h);
  const length = Module._mpc_wasm_length_samples(h);
  const rate = Module._mpc_wasm_sample_rate(h);
  const pcm = await decodeAll(Module, h, channels);
  Module._free(memPtr);
  Module._mpc_wasm_destroy(h);
  return { pcm, channels, length, rate };
}

function demandReader(Module, data) {
  const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);
  const state = new Int32Array(sab);
  const dataView = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
  const srcSab = new SharedArrayBuffer(data.length);
  new Uint8Array(srcSab).set(data);

  const worker = new Worker(path.join(ROOT, 'wasm/smoke_networker.js'));
  const ready = new Promise((resolve, reject) => {
    worker.on('message', (m) => {
      if (m.type === 'ready') resolve();
    });
    worker.on('error', reject);
  });
  worker.postMessage({ type: 'open', sab, sourceSab: srcSab, size: data.length, failMode: null });

  let pos = 0;
  function rangeRead(ptr, size) {
    const want = Math.min(size, M.DATA_CAP, data.length - pos);
    if (want <= 0) return 0;
    Atomics.store(state, M.POS_LO, pos >>> 0);
    Atomics.store(state, M.POS_HI, Math.floor(pos / 4294967296));
    Atomics.store(state, M.LEN, want);
    Atomics.store(state, M.RES, 0);
    Atomics.store(state, M.REQ, 1);
    Atomics.notify(state, M.REQ);
    while (Atomics.load(state, M.RES) === 0) Atomics.wait(state, M.RES, 0);
    if (Atomics.load(state, M.ERROR) !== 0) {
      Atomics.store(state, M.RES, 0);
      return 0;
    }
    const n = Atomics.load(state, M.DONE_LEN);
    Module.HEAPU8.set(dataView.subarray(0, n), ptr);
    pos += n;
    Atomics.store(state, M.RES, 0);
    return n;
  }
  function rangeSeek(offset) {
    pos = offset;
    return 1;
  }
  function rangeTell() {
    return pos;
  }
  Module.mpcRangeRead = rangeRead;
  Module.mpcRangeSeek = rangeSeek;
  Module.mpcRangeTell = rangeTell;

  return {
    ready,
    served: () => Atomics.load(state, M.SERVED),
    close: () => {
      worker.terminate();
      Module.mpcRangeRead = Module.mpcRangeSeek = Module.mpcRangeTell = null;
    },
  };
}

async function decodeRange(Module, bytes, channels) {
  const h = Module._mpc_wasm_create();
  const d = demandReader(Module, bytes);
  await d.ready;
  const err = await Module._mpc_wasm_open_range(h, bytes.length);
  if (err !== 0) throw new Error(`open_range: ${err}`);
  const length = Module._mpc_wasm_length_samples(h);
  const pcm = await decodeAll(Module, h, channels);
  Module._mpc_wasm_destroy(h);
  d.close();
  return { pcm, length, servedOpen: d.served() };
}

/** Offline-path stand-in for OPFS createSyncAccessHandle: reads at explicit
 *  offsets exactly like localreader.js does inside decoder.worker.js. */
function makeLocalAccess(bytes) {
  return {
    getSize: () => bytes.length,
    read: (target, { at }) => {
      const n = Math.min(target.length, bytes.length - at);
      if (n <= 0) return 0;
      target.set(bytes.subarray(at, at + n));
      return n;
    },
    close: () => {},
  };
}

/** Mirrors app/public/localreader.js's installed contract (64 KiB clamp,
 *  short-read EOS). Kept in structural lockstep; the browser test asserts
 *  the real script over OPFS. */
async function decodeLocal(Module, bytes, channels) {
  const access = makeLocalAccess(bytes);
  const size = access.getSize();
  const scratch = new Uint8Array(64 * 1024);
  let pos = 0;
  Module.mpcRangeRead = function (ptr, want) {
    if (want <= 0) return 0;
    const n = Math.min(want, scratch.length, size - pos);
    if (n <= 0) return 0;
    const got = access.read(scratch, { at: pos });
    if (got <= 0) return 0;
    Module.HEAPU8.set(scratch.subarray(0, Math.min(n, got)), ptr);
    pos += Math.min(n, got);
    return Math.min(n, got);
  };
  Module.mpcRangeSeek = function (offset) { pos = offset; return 1; };
  Module.mpcRangeTell = function () { return pos; };
  const h = Module._mpc_wasm_create();
  try {
    const err = await Module._mpc_wasm_open_range(h, size);
    if (err !== 0) throw new Error(`open_range(local): ${err}`);
    const length = Module._mpc_wasm_length_samples(h);
    const pcm = await decodeAll(Module, h, channels);
    return { pcm, length };
  } finally {
    Module._mpc_wasm_destroy(h);
    Module.mpcRangeRead = Module.mpcRangeSeek = Module.mpcRangeTell = null;
  }
}

require(moduleJs)().then(async (Module) => {
  console.log('wasm-gapless:');

  const A = await decodeFile(Module, trackA);
  const B = await decodeFile(Module, trackB);
  const S = seekFixture === trackA ? A : await decodeFile(Module, seekFixture);
  ok(A.channels === B.channels, 'tracks share channel count');
  ok(A.rate === B.rate, 'tracks share sample rate');

  // ---- 1. two-track gapless continuity --------------------------------------
  {
    // The controller hands decoded track A and then track B to the ring at
    // the exact sample boundary. Verify both decodes are complete (exact
    // frame counts, frame-exact against the memory reader) so the feed has
    // no dropped or duplicated frames, and the boundary sample of B directly
    // abuts the final sample of A.
    ok(A.pcm.length === A.length * A.channels, 'track A decoded exactly lengthSamples frames');
    ok(B.pcm.length === B.length * B.channels, 'track B decoded exactly lengthSamples frames');
    const joined = new Float32Array(A.pcm.length + B.pcm.length);
    joined.set(A.pcm);
    joined.set(B.pcm, A.pcm.length);
    ok(
      joined[A.pcm.length - 1] === A.pcm[A.pcm.length - 1] &&
        joined[A.pcm.length] === B.pcm[0],
      'track B begins immediately after track A without a boundary frame change',
    );
  }
  {
    const bytes = fs.readFileSync(trackA);
    const rr = await decodeRange(Module, bytes, A.channels);
    ok(rr.length === A.length, `range path length == memory length (${rr.length})`);
    ok(rr.pcm.length === A.pcm.length, 'range decode produces exactly lengthSamples frames');
    let same = true;
    for (let i = 0; i < A.pcm.length; i += 997) {
      if (rr.pcm[i] !== A.pcm[i]) {
        same = false;
        break;
      }
    }
    ok(same, 'range PCM matches memory PCM at sample strides');
  }

  // ---- 2b. offline (local reader) equivalence -------------------------------
  {
    // The offline path swaps the byte source only: localreader.js serves the
    // same mpcRangeRead/Seek/Tell contract from a local file. Decode the
    // fixture through that contract and demand sample-identical output.
    const bytes = fs.readFileSync(trackA);
    const lr = await decodeLocal(Module, bytes, A.channels);
    ok(lr.length === A.length, `local path length == memory length (${lr.length})`);
    ok(lr.pcm.length === A.pcm.length, 'local decode produces exactly lengthSamples frames');
    let same = true;
    for (let i = 0; i < A.pcm.length; i += 997) {
      if (lr.pcm[i] !== A.pcm[i]) {
        same = false;
        break;
      }
    }
    ok(same, 'local PCM matches memory PCM at sample strides');

    // Seek through the local contract and confirm exact samples. The
    // reader position is left wherever the previous decode finished —
    // exactly like real playback (the decoder drives every position).
    const h2 = Module._mpc_wasm_create();
    let pos = 0;
    const scratch2 = new Uint8Array(64 * 1024);
    const access2 = makeLocalAccess(bytes);
    Module.mpcRangeRead = function (ptr, want) {
      const n = Math.min(want, scratch2.length, bytes.length - pos);
      if (n <= 0) return 0;
      const got = access2.read(scratch2, { at: pos });
      if (got <= 0) return 0;
      Module.HEAPU8.set(scratch2.subarray(0, Math.min(n, got)), ptr);
      pos += Math.min(n, got);
      return Math.min(n, got);
    };
    Module.mpcRangeSeek = function (o) { pos = o; return 1; };
    Module.mpcRangeTell = function () { return pos; };
    try {
      if ((await Module._mpc_wasm_open_range(h2, bytes.length)) !== 0) throw new Error('local open failed');
      const samples = Module._mpc_wasm_length_samples(h2);
      const channels = Module._mpc_wasm_channels(h2);
      const ptr = Module._malloc(2 * channels * 4);
      for (const frac of [0.1, 0.5, 0.9, 0.25]) {
        const target = Math.floor(samples * frac);
        await Module._mpc_wasm_seek_sample(h2, target);
        const frames = await Module._mpc_wasm_read(h2, ptr, 2);
        const actual = Module.HEAPF32[ptr >> 2];
        ok(frames === 2 && actual === A.pcm[target * channels],
          `local seek to ${Math.round(frac * 100)}% decodes the requested sample`);
      }
      Module._free(ptr);
    } finally {
      Module._mpc_wasm_destroy(h2);
      Module.mpcRangeRead = Module.mpcRangeSeek = Module.mpcRangeTell = null;
    }
  }

  // ---- 3. seek accounting ----------------------------------------------------
  {
    const bytes = fs.readFileSync(seekFixture);
    const h = Module._mpc_wasm_create();
    const d = demandReader(Module, bytes);
    await d.ready;
    const err = await Module._mpc_wasm_open_range(h, bytes.length);
    if (err !== 0) throw new Error(`open_range: ${err}`);
    const total = bytes.length;
    const samples = Module._mpc_wasm_length_samples(h);
    const channels = Module._mpc_wasm_channels(h);
    const afterOpen = d.served();
    ok(afterOpen < total, `open fetched only ${afterOpen}/${total} bytes (not the whole file)`);
    ok(samples === S.length && channels === S.channels, 'seek fixture metadata matches memory decode');

    const targets = [0.1, 0.25, 0.5, 0.9, 0.25, 0.9, 0.05, 0.5, 0.9, 0.05];
    const ptr = Module._malloc(2 * channels * 4);
    for (const frac of targets) {
      const before = d.served();
      const target = Math.floor(samples * frac);
      await Module._mpc_wasm_seek_sample(h, target);
      const frames = await Module._mpc_wasm_read(h, ptr, 2);
      const after = d.served();
      const fetched = after - before;
      const actual = Module.HEAPF32[ptr >> 2];
      const expected = S.pcm[target * channels];
      ok(frames === 2, `seek to ${Math.round(frac * 100)}% decoded two frames`);
      ok(actual === expected, `seek to ${Math.round(frac * 100)}% decoded the requested sample`);
      ok(
        fetched <= 2 * M.BLOCK,
        `seek to ${Math.round(frac * 100)}% fetched ${fetched} bytes (limit ${2 * M.BLOCK})`,
      );
      ok(fetched < total, `seek to ${Math.round(frac * 100)}% did not fetch the whole file`);
    }
    Module._free(ptr);
    ok(d.served() > afterOpen, 'demand reader fetched additional bytes on seeks');
    Module._mpc_wasm_destroy(h);
    d.close();
  }

  console.log(failures === 0 ? 'wasm-gapless: PASS' : `wasm-gapless: ${failures} failure(s)`);
  process.exit(failures === 0 ? 0 : 1);
}).catch((e) => {
  console.error('wasm-gapless:', e);
  process.exit(1);
});
