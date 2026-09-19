// musepack_dump_pcm.mjs — dev tool: writes reference PCM (raw LE f32) for the
// committed fixtures so the Rust decoder can be diffed sample-by-sample.
//
//   node tools/musepack_dump_pcm.mjs <outdir>
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const outdir = process.argv[2] ?? '/tmp/mpc_ref';
const moduleJs =
  process.env.MUSICPACK_MPC_WASM_JS ??
  path.resolve(root, '../musicpack/build-wasm/wasm/musepack.js');
const fixturesDir = path.join(root, 'tests/fixtures/musepack');
fs.mkdirSync(outdir, { recursive: true });
const Module = await createRequire(import.meta.url)(moduleJs)();

for (const file of fs.readdirSync(fixturesDir).filter((f) => f.endsWith('.mpc')).sort()) {
  const bytes = fs.readFileSync(path.join(fixturesDir, file));
  const h = Module._mpc_wasm_create();
  const ptr = Module._malloc(bytes.length);
  Module.HEAPU8.set(bytes, ptr);
  if (Module._mpc_wasm_open(h, ptr, bytes.length) !== 0) throw new Error(file);
  const channels = Module._mpc_wasm_channels(h);
  const pcmPtr = Module._malloc(1152 * channels * 4);
  const chunks = [];
  for (;;) {
    const frames = Module._mpc_wasm_read(h, pcmPtr, 1152);
    if (frames < 0) break;
    if (frames === 0) break;
    chunks.push(
      Buffer.from(
        new Uint8Array(
          Module.HEAPF32.buffer,
          pcmPtr,
          frames * channels * 4,
        ),
      ),
    );
  }
  const out = Buffer.concat(chunks);
  fs.writeFileSync(path.join(outdir, `${file}.f32`), out);
  console.log(`${file}: ${out.length / 4 / channels} frames, ${channels} ch`);
  Module._free(pcmPtr);
  Module._free(ptr);
  Module._mpc_wasm_destroy(h);
}
