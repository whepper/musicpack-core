# musepack.js / musepack.wasm — frozen legacy decoder artifacts

These two files are the Emscripten build of the legacy C Musepack decoder
(`musepack_wasm` target in the immutable oracle repository,
`whepper/musicpack`, built with emsdk from that repo's `wasm/` CMake
target). They are **committed frozen compatibility artifacts**, copied at
the R2 cutover from the oracle's working tree.

Why committed instead of built:

- Their **source is immutable** (the oracle repository is never modified),
  so these artifacts cannot go stale the way a generated artifact can.
  There is deliberately no build step that refreshes them silently.
- Since **R4.4** they are **oracle-only**: normal playback (online and
  offline/OPFS) runs through the Rust/WASM engine, and no product setting,
  URL parameter or `localStorage` key selects this decoder. It remains
  reachable solely through the differential test lane
  (`sessionStorage['musicpack.oracle-decoder.v1']`, set by the Playwright
  `selectBackend` helper) used to compare the two decoders. See
  `docs/adr/0013-web-offline-rust-playback.md`.
- Consequence for the bundle: the app shell service worker still precaches
  these files, but production code only fetches them when the oracle lane is
  enabled, so normal sessions never download or execute them.

Rules:

- Never edit these files by hand.
- To refresh them (e.g. after a deliberate oracle-side change), build the
  oracle's `musepack_wasm` CMake target with emsdk and copy
  `wasm/musepack.js` + `wasm/musepack.wasm` here in a reviewed commit that
  notes the oracle commit it was cut from.
- The **Rust** WASM binding (`rust/musicpack_wasm.*`) is a different
  artifact: generated fresh from `crates/musicpack-wasm` in this
  repository by `web/scripts/build-wasm.mjs` and gitignored.
