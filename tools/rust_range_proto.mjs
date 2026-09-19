// rust_range_proto.mjs — end-to-end proof of the worker-hosted range source.
//
// Reproduces the browser architecture without a browser: a decoder worker
// hosts the Rust/WASM engine and a synchronous range callback that blocks on
// the real MusicPack SAB mailbox (`demo/reader_mailbox.js`); a range-server
// worker answers with 64 KiB block-cached ranges over fixture bytes. This is
// tooling/experimentation, not a product dependency.
//
//   node tools/rust_range_proto.mjs
//
// Reports bytes served at open / after decode / after seeks and the decoded
// Musepack PCM SHA-256 (compared with the committed oracle).

import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const WASM_JS = process.env.MUSICPACK_WASM_NODE ?? path.join(root, 'target/wasm-node/musicpack_wasm.js');
const MAILBOX_JS =
  process.env.MUSICPACK_MAILBOX ??
  path.resolve(root, '../musicpack/demo/reader_mailbox.js');
const FIXTURE = process.argv[2] ?? path.join(root, 'tests/fixtures/musepack/sine44-q5-48s.mpc');
const ORACLE = path.join(root, 'tests/data/musepack_oracle.jsonl');

if (!isMainThread) {
  const require = createRequire(import.meta.url);
  if (workerData.role === 'server') runServer(require);
  else runDecoder(require);
}

// --------------------------------------------------------------------------
// Range-server worker: serves block-cached ranges over the fixture bytes.
// --------------------------------------------------------------------------
function runServer(require) {
  const M = require(MAILBOX_JS);
  const { sab, bytes } = workerData;
  const state = new Int32Array(sab);
  const data = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
  const size = bytes.length;
  const cache = new Map();
  let served = 0;
  parentPort.postMessage({ type: 'ready' });
  for (;;) {
    while (Atomics.load(state, M.REQ) === 0) Atomics.wait(state, M.REQ, 0);
    const pos = Atomics.load(state, M.POS_LO) >>> 0;
    const len = Atomics.load(state, M.LEN);
    Atomics.store(state, M.REQ, 0);
    const want = Math.min(len, M.DATA_CAP, size - pos);
    let outLen = 0;
    let p = pos;
    while (p < pos + want && outLen < M.DATA_CAP) {
      const blockIdx = Math.floor(p / M.BLOCK);
      if (!cache.has(blockIdx)) {
        const start = blockIdx * M.BLOCK;
        const end = Math.min(start + M.BLOCK, size);
        cache.set(blockIdx, bytes.subarray(start, end));
        served += end - start;
      }
      const block = cache.get(blockIdx);
      const offInBlock = p - blockIdx * M.BLOCK;
      const n = Math.min(block.length - offInBlock, pos + want - p, M.DATA_CAP - outLen);
      data.set(block.subarray(offInBlock, offInBlock + n), outLen);
      outLen += n;
      p += n;
    }
    Atomics.store(state, M.SERVED, served);
    Atomics.store(state, M.ERROR, 0);
    Atomics.store(state, M.DONE_LEN, outLen);
    Atomics.store(state, M.RES, 1);
    Atomics.notify(state, M.RES);
  }
}

// --------------------------------------------------------------------------
// Decoder worker: Rust/WASM engine + synchronous range callback.
// --------------------------------------------------------------------------
function runDecoder(require) {
  const M = require(MAILBOX_JS);
  const wasm = require(WASM_JS);
  const { sab, url, totalSize } = workerData;
  const state = new Int32Array(sab);
  const data = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
  let readCalls = 0;

  const rangeRead = (_u, offset, len) => {
    readCalls += 1;
    const want = Math.min(len, M.DATA_CAP, totalSize - offset);
    if (want <= 0) return new Uint8Array(0);
    Atomics.store(state, M.POS_LO, offset >>> 0);
    Atomics.store(state, M.POS_HI, Math.floor(offset / 4294967296));
    Atomics.store(state, M.LEN, want);
    Atomics.store(state, M.RES, 0);
    Atomics.store(state, M.REQ, 1);
    Atomics.notify(state, M.REQ);
    while (Atomics.load(state, M.RES) === 0) Atomics.wait(state, M.RES, 0);
    if (Atomics.load(state, M.ERROR) !== 0) {
      const e = Atomics.load(state, M.ERROR);
      Atomics.store(state, M.RES, 0);
      throw new Error(`range source error ${e}`);
    }
    const n = Atomics.load(state, M.DONE_LEN);
    const out = new Uint8Array(n);
    out.set(data.subarray(0, n));
    Atomics.store(state, M.RES, 0);
    return out;
  };
  const served = () => Atomics.load(state, M.SERVED);
  const scratch = Buffer.alloc(1024 * 2 * 4);
  /** Hashes exactly `frames` decoded frames, incrementally (no full PCM). */
  const hashFrames = (engine, frames) => {
    const h = createHash('sha256');
    let produced = 0;
    for (let i = 0; i < 100000 && produced < frames; i++) {
      const block = engine.render(1024);
      const take = Math.min(block.length / 2, frames - produced);
      for (let j = 0; j < take * 2; j++) scratch.writeFloatLE(block[j], j * 4);
      h.update(scratch.subarray(0, take * 2 * 4));
      produced += take;
    }
    return h.digest('hex');
  };
  /** Decodes up to `frames`, returning the count (used after seeks). */
  const countFrames = (engine, frames) => {
    let produced = 0;
    for (let i = 0; i < 100000 && produced < frames; i++) {
      const block = engine.render(1024);
      produced += block.length / 2;
      if (engine.rendered_samples() >= frames) break;
    }
    return produced;
  };

  const engine = wasm.WasmEngine.newRangeSource(44100, 2, rangeRead);
  const item = JSON.stringify({ id: 't1', trackId: 1, url, codec: 'musepack-sv8' });
  const info = JSON.parse(engine.open(item));
  const servedAtOpen = served();
  engine.start();
  engine.play();

  // A 10% seek before any full decode: must fetch far less than the member
  // (the generic seek is reopen + decode-and-skip, block-cached).
  engine.seek(Math.floor(info.lengthSamples * 0.1));
  const seek10 = countFrames(engine, 2000);
  const servedAfterSeek10 = served();

  // Full decode from the start (mostly cached), hashed against the oracle.
  engine.seek(0);
  const pcmSha = hashFrames(engine, info.lengthSamples);
  const servedAfterDecode = served();

  // A far seek after the full decode is served entirely from cache.
  engine.seek(Math.floor(info.lengthSamples * 0.9));
  const seek90 = countFrames(engine, 2000);
  const servedAfterSeek90 = served();
  engine.close();

  parentPort.postMessage({
    type: 'result',
    info,
    totalSize,
    readCalls,
    servedAtOpen,
    servedAfterSeek10,
    servedAfterDecode,
    servedAfterSeek90,
    pcmSha,
    pcmFrames: info.lengthSamples,
    seekSamples: [seek10, seek90],
  });
}

// --------------------------------------------------------------------------
// Main
// --------------------------------------------------------------------------
async function main() {
  if (!existsSync(WASM_JS) || !existsSync(FIXTURE)) {
    console.error(`missing wasm (${WASM_JS}) or fixture; build musicpack-wasm first`);
    process.exit(2);
  }
  const bytes = new Uint8Array(readFileSync(FIXTURE));
  const M = createRequire(import.meta.url)(MAILBOX_JS);
  const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);

  const server = new Worker(new URL(import.meta.url), {
    workerData: { role: 'server', sab, bytes },
  });
  await new Promise((resolve, reject) => {
    server.on('message', (m) => m.type === 'ready' && resolve());
    server.on('error', reject);
  });

  const decoder = new Worker(new URL(import.meta.url), {
    workerData: { role: 'decoder', sab, url: '/f.mpc', totalSize: bytes.length },
  });
  const result = await new Promise((resolve, reject) => {
    decoder.on('message', (m) => m.type === 'result' && resolve(m));
    decoder.on('error', reject);
  });
  await decoder.terminate();
  await server.terminate();

  const name = path.basename(FIXTURE);
  const oracle = readFileSync(ORACLE, 'utf8')
    .trim()
    .split('\n')
    .map((l) => JSON.parse(l))
    .find((r) => r.file === name);
  const expectedSha = oracle?.pcmSha256;

  console.log(`compressed member: ${bytes.length} B`);
  console.log(`metadata: rate=${result.info.rate} channels=${result.info.channels} frames=${result.info.lengthSamples}`);
  console.log(`range read calls: ${result.readCalls}`);
  console.log(`served at open: ${result.servedAtOpen} B`);
  console.log(`served after 10% seek + decode: ${result.servedAfterSeek10} B`);
  console.log(`served after full decode: ${result.servedAfterDecode} B`);
  console.log(`served after 90% seek (cached): ${result.servedAfterSeek90} B`);
  console.log(`seek decoded frames: ${JSON.stringify(result.seekSamples)}`);
  console.log(`PCM frames: ${result.pcmFrames}`);
  console.log(`PCM sha256: ${result.pcmSha}`);
  console.log(`oracle   : ${expectedSha}`);

  const okOpen = result.servedAtOpen < bytes.length;
  const okSeek = result.servedAfterSeek10 < bytes.length;
  const okNotWhole = result.servedAfterDecode <= bytes.length;
  const okSha = !expectedSha || result.pcmSha === expectedSha;
  console.log(`\nopen did not fetch the whole member: ${okOpen}`);
  console.log(`10% seek did not fetch the whole   : ${okSeek}`);
  console.log(`decode served <= member size        : ${okNotWhole}`);
  console.log(`PCM matches the reference oracle    : ${okSha}`);
  if (!okOpen || !okSeek || !okNotWhole || !okSha) process.exit(1);
}

if (isMainThread) {
  await main();
}
