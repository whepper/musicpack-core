# musicpack-wasm

A **thin `wasm-bindgen` binding foundation** over
[`musicpack-core`](../..) and
[`musicpack-engine`](../musicpack-engine).

It is deliberately **not** the browser player application, the Web Audio
engine, the HTTP layer, the persistence layer, or the UI. It exposes the
smallest API that lets JavaScript drive the deterministic Rust core over
byte-backed sources.

## API

Byte-backed decode handles:

```js
const handle = wasm.decode_open(bytes, 44100, 2); // Uint8Array → handle
const info = JSON.parse(wasm.decode_info(handle)); // { rate, channels, lengthSamples, sourceRate, sourceChannels }
const pcm = wasm.decode_read(handle, 4410);         // Float32Array
wasm.decode_seek(handle, 44100);                    // output-rate frames
wasm.decode_close(handle);
```

A player handle:

```js
const player = new wasm.WasmPlayer(44100, 2);
player.add_source('/fixture/a.wav', bytes);        // complete bytes only
const state = player.load(itemsJson);              // load + start
const out = player.render(1024);                   // Float32Array (host output)
player.command(JSON.stringify({ op: 'pause' }));   // { model, events }
const model = JSON.parse(player.info());
const snapshot = player.snapshot();                // string | undefined
player.restore(snapshot);
```

The representation-selection policy (the same resolver the Rust core uses):
```js
const result = wasm.representation_select(
  JSON.stringify({ codec, mimeType, representations: [{ id, codec, mimeType }] }),
  JSON.stringify({ mode: 'codec', codec: 'flac' }), // or undefined for default
  JSON.stringify({ codecs: ['flac'], rejectMimes: [], rejectIds: [] }),
);
// -> { representationId: number | null }  (null = the primary audio)
```

Items are JSON objects: `{ id, trackId, url, durationHintSeconds?, title?,
artist?, albumTitle?, kind?, codec? }`.

## Range source (worker host)

`WasmEngine.newRangeSource(rate, channels, read)` builds the engine over a
**synchronous range callback** `read(url, offset, len) -> Uint8Array` instead of
`add_source`'s complete bytes. The Rust side (`RangeBackend`/`RangeRead` over the
`RangeFetch` trait) stays an ordinary `Read`; there is no browser API in Rust.

The callback must be synchronous and, in the browser, invoked only from the
decoder worker — where `Atomics.wait` is legal — so it can reuse the existing
MusicPack SharedArrayBuffer mailbox (`rangereader.js`/`reader_mailbox.js`) and
network/local workers. The main thread must never call it.

See `tools/rust_range_proto.mjs` for an end-to-end worker + SAB + block-cache
proof (byte-exact Musepack PCM; open fetches one block, not the whole member).

## Boundaries

- Sources are **complete byte buffers**. Live HTTP/OPFS/range fetching is a
  JavaScript/platform concern and is not implemented here.
- PCM crosses as a `Float32Array` with **one copy** out of WASM memory; no
  zero-copy/transfer claim is made.
- No `AudioContext`, no `HTMLAudioElement`, no Media Session, no async
  runtime, no threads. Those belong to the host.

## Testing

Native unit tests exercise the plain-Rust logic:

```sh
cargo test -p musicpack-wasm
```

The Node smoke test builds the binding and runs a real WAV end to end (no
browser, no `wasm-pack`):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128
../../tools/wasm_smoke.sh
```
