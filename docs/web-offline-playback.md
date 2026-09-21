# Web offline playback (R4.4)

Status: implemented. Decisions: `docs/adr/0013-web-offline-rust-playback.md`.
Legacy-decoder provenance: `web/app/public/PROVENANCE-legacy-decoder.md`.

Since R4.4 the web player decodes **all** supported codecs through the
Rust/WASM engine, online and offline. The frozen Emscripten Musepack decoder
is retained only as a differential oracle.

## 1. Architecture

```text
                       Web player (player-core + controller)
                                   │  chooseBackend(item)
                                   ▼
                        RustPlaybackEngine (Engine port)
                                   │  worker messages
                                   ▼
                    rust-playback.worker.js  ── WasmEngine (musicpack-wasm)
                       │  synchronous read(url, offset, len)
                 ┌─────┴───────────────────────────┐
                 ▼                                 ▼
        NetworkSource                       OpfsSource
        (networker.js SAB mailbox)          (FileSystemSyncAccessHandle)
        http-range / stream URLs            local-file OPFS keys
                 │                                 │
                 └───────────────┬─────────────────┘
                                 ▼
                          PCM → bounded SAB ring → rust-playback-sink AudioWorklet
```

The decoder never learns where the bytes came from: `WasmEngine::newRangeSource`
takes one synchronous range callback, and the worker dispatches per URL.

## 2. Source abstraction

There is **no offline-specific decoder API**. The source seam is the existing
`RangeFetch` (`read(url, offset, len) -> bytes`) already used online:

- **NetworkSource** — one `networker.js` worker per URL with a
  SharedArrayBuffer mailbox (`reader_mailbox.js`); `readRange` writes
  offset/length into the control words and blocks on `Atomics.wait` for the
  response. Unchanged.
- **OpfsSource** — a `FileSystemSyncAccessHandle` on the committed OPFS file,
  opened inside the dedicated worker (where sync access handles are legal).
  `readLocal` clamps to EOF and copies at most 64 KiB per call. `createSyncAccessHandle`
  is available because the worker already requires cross-origin isolation for
  the online SAB path.

`openSource(kind, url, size, token)` selects the source: `local-file` → OPFS,
anything else → network. `readRange` dispatches on whether the URL is a known
OPFS key. `servedBytes()` sums networker `SERVED` counters and OPFS bytes read.

## 3. OPFS layout and identity

Unchanged from R3.6: `musicpack-offline-v1/releases/<key>`, where `<key>` is
the installer's stable per-record name (`source.url` on a `local-file` item).
The worker opens exactly that key and verifies `getSize() === source.byteSize`
when a size is known; a mismatch fails the open with a typed error rather than
playing unverified bytes. The installer's SHA-256 / atomic-commit model is
untouched — playback consumes the same committed bytes.

## 4. WASM boundary and distribution

The engine is the same generated binding as the online path
(`web/scripts/build-wasm.mjs` → `app/public/rust/musicpack_wasm*.js/.wasm`,
no-modules target). No new binding was introduced. The worker keeps the
existing ownership rules: `initSync` with an explicit `WebAssembly.Module`,
one `WasmEngine` per generation, `render()` returns a copied `Float32Array`,
`close()` releases the engine and both source registries, and typed errors are
returned through `take_error()`/worker `error` messages (no eval, no dynamic
code).

Distribution: the app shell service worker precaches the Rust playback runtime
(`/rust-playback-sink.js`, `/rust-playback.worker.js`,
`/rust/musicpack_wasm.js`, `/rust/musicpack_wasm_bg.wasm`) so a **cold offline
start** can load it. SW cache version bumped to `v6`. CSP is unchanged.

## 5. Seeking

Unchanged strategy: `seek(samples)` in Rust reopens the source and
decodes/skips to the target; the worker resets the ring and reports `seeked`.
This works over OPFS because the sync access handle is randomly addressable
(`read(buffer, { at: offset })`). No second seeking implementation exists.
Covered offline: forward/backward seeks, seek while paused, seek after a track
change, and repeated seeks (e2e + differential).

## 6. Offline/network behavior

Source selection is unchanged (R3.6 local-first rule): an installed candidate
resolves to `local-file`, otherwise to the network candidate. Only the
*decoder* changed. Fallback semantics are preserved:

- installed + network up → local-first (OPFS);
- installed + network down → OPFS playback (the R4.4 e2e severs the network);
- not installed + network down → the existing availability/error state
  (no silent substitution);
- corrupt/missing OPFS asset → refused, typed error; the boot audit surfaces
  "Needs repair" and Reinstall heals it (unchanged installer integrity model).

The `legacyPlayback`/`rustPlayback` flags are **removed**; there is no
decoder-level fallback. A Rust failure is a visible Rust failure.

## 7. Errors, cancellation, lifecycle

- Malformed/truncated OPFS audio → `render`/`open` error surfaces as the
  engine's typed error and the player's error state; no partial playback.
- Cancellation: track change/stop tears the generation down (`close`), which
  terminates the producer worker, closes the OPFS handle and closes the source
  registries; worker generations are isolated by `generation`.
- No stale handles: `teardownWorker`/`close` terminate the worker (and its
  OPFS handles) before a fresh `open`; the sink is closed on `close()`.

## 8. Security

- `musicpack-core`/`musicpack-engine` gain no browser/OPFS/DOM dependency;
  core crates keep `#![forbid(unsafe_code)]`.
- OPFS reads are bounded (64 KiB) and clamped to the file size; filenames and
  MIME types are never trusted (bytes are decoded by the same bounded Rust
  parser as the network path).
- No `eval`, no dynamic executable code, no DOM/HTML injection, no CSP
  relaxation; the browser never executes `.mpack` content.

## 9. Performance findings

- **Startup (offline, Rust):** the R4.4 e2e logs `offline-rust[play]
  timeToPlayingMs`; observed ≈0.2–1.2 s for the 48 s fixture, comparable to
  the online differential (~180 ms both lanes).
- **Seek (offline):** `offline-rust[seek] latencyMs` is logged; OPFS random
  access avoids any full-file refetch.
- **Memory:** bounded 64 KiB reads; PCM lives only in the fixed 16 384-frame
  SAB ring — an album is never materialised in JS memory.
- **Throughput/stability:** the soak/underrun specs exercise repeated
  teardown/rebuild and long sessions; the offline crossfade e2e exercises an
  overlapped boundary through the Rust mixer.

No material regression was observed; no optimisation was attempted.

## 10. Legacy decoder status

`musepack.js` / `musepack.wasm` / `decoder.worker.js` / `localreader.js` /
`musepack-engine.ts` remain in the tree **as an oracle**, reachable only via
the differential session key (`musicpack.oracle-decoder.v1`) set by the
Playwright `selectBackend` helper. Production never selects them. See
`PROVENANCE-legacy-decoder.md`.
