// node_smoke.mjs — Node/WASM smoke test for the musicpack-wasm binding.
//
// Builds are produced by tools/wasm_smoke.sh (or manually):
//   cargo build -p musicpack-wasm --target wasm32-unknown-unknown --release
//   wasm-bindgen --target nodejs --out-dir target/wasm-node \
//     target/wasm32-unknown-unknown/release/musicpack_wasm.wasm
//   node crates/musicpack-wasm/tests/node_smoke.mjs
//
// The binding is byte-backed only (no HTTP/OPFS/audio device); this test
// exercises a real WAV fixture end to end.

import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import fs from 'node:fs';
import path from 'node:path';

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const pkgDir =
  process.env.MUSICPACK_WASM_PKG ?? path.resolve(here, '../../../target/wasm-node');
const wasm = require(path.join(pkgDir, 'musicpack_wasm.js'));

function wav(frames, rate = 44100, channels = 2) {
  const block = channels * 2;
  const data = frames * channels * 2;
  const buf = Buffer.alloc(44 + data);
  buf.write('RIFF', 0);
  buf.writeUInt32LE(36 + data, 4);
  buf.write('WAVE', 8);
  buf.write('fmt ', 12);
  buf.writeUInt32LE(16, 16);
  buf.writeUInt16LE(1, 20);
  buf.writeUInt16LE(channels, 22);
  buf.writeUInt32LE(rate, 24);
  buf.writeUInt32LE(rate * block, 28);
  buf.writeUInt16LE(block, 32);
  buf.writeUInt16LE(16, 34);
  buf.write('data', 36);
  buf.writeUInt32LE(data, 40);
  for (let i = 0; i < frames; i++) {
    const v = Math.round(Math.sin((i / rate) * 2 * Math.PI * 440) * 0.5 * 32767);
    buf.writeInt16LE(v, 44 + i * block);
    buf.writeInt16LE(Math.round(v * 0.9), 44 + i * block + 2);
  }
  return buf;
}

let failures = 0;
function check(name, cond, detail = '') {
  if (cond) {
    console.log(`ok   ${name}`);
  } else {
    failures += 1;
    console.log(`FAIL ${name} ${detail}`);
  }
}

// ---- decode handles ---------------------------------------------------------

const bytes = wav(44100);
const h = wasm.decode_open(bytes, 44100, 2);
check('decode_open returns a handle', Number.isInteger(h));
const info = JSON.parse(wasm.decode_info(h));
check('decode_info reports the output rate', info.rate === 44100, JSON.stringify(info));
check('decode_info reports the source rate', info.sourceRate === 44100, JSON.stringify(info));
check('decode_info reports stereo', info.sourceChannels === 2, JSON.stringify(info));
check('decode_info length is 44100 frames', info.lengthSamples === 44100, JSON.stringify(info));

const first = wasm.decode_read(h, 4410);
check('decode_read returns a Float32Array', first instanceof Float32Array);
check('decode_read returns frames*channels samples', first.length === 4410 * 2, String(first.length));
check('decoded audio is non-silent', first.some((s) => Math.abs(s) > 0.01));

wasm.decode_seek(h, 22050);
const afterSeek = wasm.decode_read(h, 441);
check('decode_read after seek returns audio', afterSeek.some((s) => Math.abs(s) > 0.01));
wasm.decode_close(h);

// ---- Musepack decode (same Rust decoder compiled to wasm32) ----------------

const mpcBytes = new Uint8Array(
  fs.readFileSync(
    new URL('../../../tests/fixtures/musepack/sine44-q5.mpc', import.meta.url),
  ),
);
const mh = wasm.decode_open(mpcBytes, 44100, 2);
const mInfo = JSON.parse(wasm.decode_info(mh));
check('Musepack decode_info reports the rate', mInfo.rate === 44100, JSON.stringify(mInfo));
check('Musepack decode_info reports the length', mInfo.lengthSamples === 44100, JSON.stringify(mInfo));
let mpcFrames = 0;
let mpcAudible = false;
for (let i = 0; i < 200; i++) {
  const pcm = wasm.decode_read(mh, 1152);
  if (pcm.length === 0) break;
  mpcFrames += pcm.length / 2;
  if (!mpcAudible && pcm.some((s) => Math.abs(s) > 0.01)) mpcAudible = true;
}
check('Musepack decodes the declared length', mpcFrames === 44100, String(mpcFrames));
check('Musepack WASM PCM is non-silent', mpcAudible);
wasm.decode_close(mh);

// ---- representation-selection policy ---------------------------------------

const trackJson = JSON.stringify({
  codec: 'musepack-sv8',
  mimeType: 'audio/musepack',
  representations: [
    { id: 10, codec: 'flac', mimeType: 'audio/flac' },
    { id: 11, codec: 'wav', mimeType: 'audio/wav' },
  ],
});
const primary = JSON.parse(wasm.representation_select(trackJson, undefined, '{}'));
check('representation_select default picks primary', primary.representationId === null, JSON.stringify(primary));
const byCodec = JSON.parse(
  wasm.representation_select(trackJson, JSON.stringify({ mode: 'codec', codec: 'flac' }), '{}'),
);
check('representation_select codec picks flac', byCodec.representationId === 10, JSON.stringify(byCodec));
const rescued = JSON.parse(
  wasm.representation_select(trackJson, undefined, JSON.stringify({ codecs: ['flac'] })),
);
check('representation_select rescues unplayable primary', rescued.representationId === 10, JSON.stringify(rescued));

// ---- player handle ----------------------------------------------------------

const player = new wasm.WasmPlayer(44100, 2);
player.add_source('/fixture/a.wav', bytes);
const items = JSON.stringify([
  { id: 't1', trackId: 1, url: '/fixture/a.wav', durationHintSeconds: 1.0, codec: 'wav' },
]);
const loaded = JSON.parse(player.load(items));
check('player.load returns a model', typeof loaded.model === 'object');

let rendered = 0;
let silent = true;
for (let i = 0; i < 100; i++) {
  const out = player.render(1024);
  rendered += out.length / 2;
  if (out.some((s) => Math.abs(s) > 0.01)) silent = false;
}
check('player.render produced PCM', rendered > 0 && !silent);
const model = JSON.parse(player.info());
check('player.info reports a state', typeof model.state === 'string', JSON.stringify(model));

const snapshot = player.snapshot();
check('player.snapshot returns a string', typeof snapshot === 'string');
if (snapshot) {
  const restored = JSON.parse(player.restore(snapshot));
  check('player.restore returns a model', typeof restored.model === 'object');
}

const paused = JSON.parse(player.command(JSON.stringify({ op: 'pause' })));
check('player.command pause works', typeof paused.model === 'object');

if (failures > 0) {
  console.error(`\n${failures} WASM smoke check(s) failed`);
  process.exit(1);
}
console.log('\nall WASM smoke checks passed');
