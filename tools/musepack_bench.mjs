// musepack_bench.mjs — measures the Rust/WASM Musepack decoder (Node build).
// Reports decode throughput and process memory for the fixture corpus. This is
// a tooling measurement, not a product dependency.
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const pkg = process.env.MUSICPACK_WASM_JS
  ? path.dirname(process.env.MUSICPACK_WASM_JS)
  : path.resolve(root, 'target/wasm-node');
const wasm = createRequire(import.meta.url)(path.join(pkg, 'musicpack_wasm.js'));
const dir = path.join(root, 'tests/fixtures/musepack');

function bench(file) {
  const bytes = fs.readFileSync(path.join(dir, file));
  const t0 = process.hrtime.bigint();
  const h = wasm.decode_open(new Uint8Array(bytes), 44100, 2);
  const info = JSON.parse(wasm.decode_info(h));
  let frames = 0;
  for (;;) {
    const pcm = wasm.decode_read(h, 1152);
    if (pcm.length === 0) break;
    frames += pcm.length / 2;
  }
  wasm.decode_close(h);
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  const rss = process.memoryUsage().rss / (1024 * 1024);
  console.log(
    `${file}: ${(bytes.length / 1024).toFixed(0)} KiB compressed, ${frames} frames, ` +
      `${ms.toFixed(1)} ms, ${(frames / ms / 1000).toFixed(2)} Mframe/s, rss=${rss.toFixed(0)} MiB`,
  );
}

for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.mpc')).sort()) bench(f);
