// musepack_oracle.mjs — generates the committed Phase 13 Musepack corpus
// oracle used by tests/musepack_oracle.rs.
//
// The reference decoder is the project's vendored libmpcdec, built to
// WebAssembly (`build-wasm/wasm/musepack.js`, or set MUSICPACK_MPC_WASM_JS).
// This tool decodes every committed fixture and records the stream metadata
// the reference exposes, plus the reference PCM digest (recorded for the
// deferred synthesis key; the metadata layer is what this phase verifies).
//
// Reproducible: running it twice over the same inputs yields byte-identical
// JSONL. It is a test/tooling dependency only; nothing in the product links
// against the reference.
//
// Usage:
//   node tools/musepack_oracle.mjs > tests/data/musepack_oracle.jsonl
//   MUSICPACK_MPC_WASM_JS=/abs/path/musepack.js node tools/musepack_oracle.mjs
//
// The generator fails clearly (exit 2) when the reference module is absent.

import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const moduleJs =
  process.env.MUSICPACK_MPC_WASM_JS ??
  path.resolve(root, '../musicpack/build-wasm/wasm/musepack.js');
const fixturesDir = path.join(root, 'tests/fixtures/musepack');

if (!fs.existsSync(moduleJs)) {
  console.error(
    `reference decoder not found: ${moduleJs}\n` +
      'Build the reference build-wasm target or set MUSICPACK_MPC_WASM_JS.',
  );
  process.exit(2);
}

const require = createRequire(import.meta.url);
const Module = await require(moduleJs)();
const EOF = -5;

function decodeAll(h, channels) {
  const ptr = Module._malloc(1152 * channels * 4);
  const out = [];
  try {
    for (;;) {
      const frames = Module._mpc_wasm_read(h, ptr, 1152);
      if (frames === EOF) break;
      if (frames < 0) throw new Error(`read returned ${frames}`);
      if (frames === 0) break;
      const view = new Float32Array(Module.HEAPF32.buffer, ptr, frames * channels);
      out.push(Float32Array.from(view));
    }
  } finally {
    Module._free(ptr);
  }
  const total = out.reduce((n, c) => n + c.length, 0);
  const pcm = new Float32Array(total);
  let at = 0;
  for (const chunk of out) {
    pcm.set(chunk, at);
    at += chunk.length;
  }
  return pcm;
}

function digestF32(samples) {
  const buf = Buffer.alloc(samples.length * 4);
  for (let i = 0; i < samples.length; i++) buf.writeFloatLE(samples[i], i * 4);
  return createHash('sha256').update(buf).digest('hex');
}

const files = fs
  .readdirSync(fixturesDir)
  .filter((f) => f.endsWith('.mpc'))
  .sort();

const lines = [];
for (const file of files) {
  const bytes = fs.readFileSync(path.join(fixturesDir, file));
  const h = Module._mpc_wasm_create();
  const memPtr = Module._malloc(bytes.length);
  Module.HEAPU8.set(bytes, memPtr);
  const err = Module._mpc_wasm_open(h, memPtr, bytes.length);
  if (err !== 0) {
    Module._free(memPtr);
    Module._mpc_wasm_destroy(h);
    throw new Error(`open ${file}: error ${err}`);
  }
  const streamVersion = Module._mpc_wasm_stream_version(h);
  const sampleRate = Module._mpc_wasm_sample_rate(h);
  const channels = Module._mpc_wasm_channels(h);
  const lengthSamples = Module._mpc_wasm_length_samples(h);
  const pcm = decodeAll(h, channels);
  Module._free(memPtr);
  Module._mpc_wasm_destroy(h);

  const head = [];
  for (let i = 0; i < Math.min(16, pcm.length); i++) head.push(pcm[i]);

  lines.push(
    JSON.stringify({
      kind: 'musepack',
      file,
      bytesSha256: createHash('sha256').update(bytes).digest('hex'),
      streamVersion,
      sampleRate,
      channels,
      lengthSamples,
      // Recorded for the deferred synthesis key; not asserted by the
      // metadata differential (the decoder builds no PCM in this phase).
      pcmFrames: pcm.length / channels,
      pcmSha256: digestF32(pcm),
      head,
    }),
  );
}

process.stdout.write(lines.join('\n') + '\n');
