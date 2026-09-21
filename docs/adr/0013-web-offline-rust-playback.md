# ADR 0013: Web offline (OPFS) playback on the Rust engine

- Status: Accepted (2026-09-21, R4.4)

## Context

Network playback already ran on the Rust/WASM engine
(`rust-playback.worker.js` → `musicpack-wasm` `WasmEngine`), driven by a
synchronous `read(url, offset, len)` range callback that the worker served
from an HTTP `networker` mailbox. Offline/OPFS Musepack playback, however,
still routed to the frozen Emscripten decoder (`musepack-engine.ts` +
`public/musepack.{js,wasm}` + `decoder.worker.js` + `localreader.js`), and
`?legacyPlayback=1` selected that lane for the whole app. R3.7 identified this
as the last legacy playback runtime (`docs/r3.7-r4-readiness.md` §2.D, §12).

The Rust engine's public surface (`WasmEngine::newRangeSource`) already takes
the byte source as a synchronous callback. The only missing piece was an OPFS
byte source and the backend-selection change.

## Decision

1. **One decoder, one source seam.** The OPFS path serves the *same*
   `WasmEngine` through the *same* range callback. There is no offline-only
   decoder API. The worker keeps a registry of byte sources keyed by URL:
   HTTP URLs are served by their `networker` (SharedArrayBuffer mailbox);
   offline keys are served by a `FileSystemSyncAccessHandle` opened on the
   committed OPFS file. `read(url, offset, len)` dispatches on which registry
   holds the URL. Read chunks are bounded to 64 KiB (the Rust reader's
   window), so no album is ever read into JS memory.
2. **Backend selection simplifies.** `chooseBackend` routes every codec the
   Rust engine decodes — Musepack/FLAC/WAV — to Rust, including OPFS
   `local-file` sources. `local-file` is no longer an exception.
3. **The frozen Emscripten decoder becomes oracle-only.** It is retained (with
   its provenance) for differential tests, reachable solely through a
   test-only session key (`musicpack.oracle-decoder.v1`, set by the Playwright
   `selectBackend` helper). The product flag is removed: `?legacyPlayback`,
   `?rustPlayback`, `musicpack.legacy-playback.v1` and
   `musicpack.rust-playback.v1` no longer exist. There is no automatic
   fallback from Rust to the legacy decoder.
4. **Offline shell completeness.** A cold offline start must be able to load
   the Rust runtime, so the service worker precaches
   `/rust-playback-sink.js`, `/rust-playback.worker.js`,
   `/rust/musicpack_wasm.js` and `/rust/musicpack_wasm_bg.wasm` (SW version
   bumped). Previously only the legacy decoder scripts were precached.
5. **Seeking is unchanged.** The engine's established strategy (reopen the
   source and decode/skip to the target) works over OPFS because the sync
   access handle is randomly addressable; there is no second seeking
   implementation for offline Musepack.
6. **No core/engine changes.** `musicpack-core` and `musicpack-engine` gain no
   browser, OPFS, worker or DOM dependency. The browser boundary stays in the
   web worker; the range callback is the existing `RangeFetch` seam.

## Consequences

- Offline Musepack plays through the same decoder as network Musepack; the
  OPFS adapter is proven fidelity-neutral (range vs complete-bytes PCM
  identical) and the Rust decoder is proven byte-identical to the libmpcdec
  oracle.
- The differential oracle lane still exercises the frozen decoder, so the
  compatibility comparison remains available without being a runtime path.
- The app shell service worker must be precached with the Rust runtime; a
  cache-version bump is required whenever the runtime file set changes.
- `sound`/security posture is unchanged: Rust `forbid(unsafe_code)` where
  applicable, bounded reads, typed errors, no eval, no DOM injection, untrusted
  package bytes treated as untrusted.
