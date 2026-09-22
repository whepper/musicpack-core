# AGENTS.md — musicpack-core

This repo is the production **Rust** implementation of MusicPack. Keep changes
small, WASM-clean, and license-aware. Details live in `docs/architecture.md`
and the crate READMEs; this file is the non-negotiable subset.

## Repositories

- Work here, in `musicpack-core`. The sibling `musicpack` repository is the
  **immutable C reference implementation** — never modify it as part of work
  on this repo (no cleanup, deletions, build/CI/docs changes there either).
- Since R2, this repository also hosts the frontend applications under
  **`web/`** (Svelte 5 web player; Vite + Playwright; commands in
  `web/README.md`) and **`author/`** (Tauri 2 authoring app; `author/
  src-tauri` is excluded from the Cargo workspace). Frontend CI lives in
  `.github/workflows/{web,author}.yml` behind the `ci.yml` path router.
  `web/app/public/rust/` is a **generated** WASM artifact
  (`web/scripts/build-wasm.mjs`); the committed `musepack.{js,wasm}` are
  frozen oracle artifacts (`web/app/public/PROVENANCE-legacy-decoder.md`).

## Licensing / provenance boundaries (do not blur these)

- `crates/musicpack-musepack-encoder/` and `crates/musicpack-mpc-tools/` are
  **source-derived LGPL-2.1-or-later** code (ported from the legacy C
  encoder). Keep their `LICENSE` files, SPDX headers, and attribution intact;
  the LGPL status does not lapse just because the C sources live elsewhere.
- Core/permissive crates declare **BSD-3-Clause** in their `Cargo.toml`
  (root `LICENSE`). New code takes its crate's declared license; never move
  source-derived encoder logic into a permissively-licensed crate.
- `musicpack-core` must never depend on `musicpack-musepack-encoder`
  (one-way isolation; see the encoder README).

## Hard technical constraints

- No `unsafe` (all crates carry `#![forbid(unsafe_code)]`), no FFI, no
  bindgen, no FFmpeg, no C dependencies, no `build.rs`, and no
  subprocess-based **production** dependencies — with one scoped exception:
  `crates/musicpack-server/` (native-only, never a WASM target) uses
  `rusqlite` with bundled SQLite, whose C/`unsafe` stays inside that
  dependency behind the server's `Store` trait. See
  `docs/server-migration.md` (D-S2/O-S1). The rule is unchanged for every
  other crate; nothing else may add a C/`unsafe` dependency, and no other
  crate may depend on `musicpack-server`.
- Keep the crates that guarantee it WASM-compatible (`musicpack-core`,
  encoder, tools, engine/host/wasm).

## Musepack bit-exactness (the frozen boundary)

- Encoder compatibility is intentionally **bit-exact** against the frozen
  reference corpus. Do not casually change numerical semantics, FP
  evaluation order, tables, rounding, or compiler assumptions
  (no FMA/contraction, reassociation, or fast-math).
- Preserve the encoder's state ownership, stage execution order, and scalar
  numerical reference semantics.
- Frozen fixtures/manifests (e.g.
  `crates/musicpack-musepack-encoder/tests/data/encoder_reference_manifest.txt`)
  must never be regenerated or edited to make tests pass. Any change needs a
  demonstrated compatibility reason plus explicit review.

## Deliberately out of scope (do not resurrect opportunistically)

- SV7 and `mpc2sv8` are unsupported/retired — SV7 decoding is decoder scope.
- Legacy C CLI functionality outside the migration (`mpcgain`, `mpcchap`,
  authoring draft/identify pipeline, server/sonic, file-level `mpcdec`) has
  no Rust equivalent by decision. Add new functionality only when a current
  product workflow requires it.
- The Rust encoder is a **library**; do not add a WAV→MPC CLI just because
  the legacy repo had `mpcenc`.

## Validate before declaring substantial work complete

Canonical gates (mirror `.github/workflows/ci.yml`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
cargo check -p musicpack-core --target wasm32-unknown-unknown
cargo clippy -p musicpack-core --target wasm32-unknown-unknown -- -D warnings
cargo check --manifest-path fuzz/Cargo.toml
```

Targeted compatibility corpora (also covered by `cargo test --workspace`):

```sh
cargo test -p musicpack-musepack-encoder --test encoder_whole   # 21/21, 140,420 bytes exact
cargo test -p musicpack-musepack-encoder --test encoder_matrix  # 44-config integer matrix + mono, byte-identical
cargo test -p musicpack-musepack-encoder --test encoder_fractional # 27-row fractional corpus (J.2), byte-identical to scalar C
cargo test -p musicpack-mpc-tools --test cut_compat             # 12/12 byte-identical
```

Notes: `tools/` `.c`/`.py` files are oracle/fixture generators, not
production code — never wire them into builds or tests. `fuzz/` is excluded
from the workspace (nightly + libfuzzer) but its targets must keep compiling.

## Test and I/O discipline (time + SSD lifespan)

Wall-clock time and disk write volume are first-class costs: full suites
exist for gates, not for iteration.

- **Iterate targeted; gate consolidated.** During a change, run the
  smallest relevant suite (`cargo test -p <crate>`, `cargo check -p`,
  a single Playwright spec/tier). Each full gate above runs **once** when
  the work is complete — not after every edit.
- **Never rebuild past a staleness guard.** Generated artifacts have
  guards exactly so nobody runs unconditional rebuilds (e.g.
  `web/scripts/build-wasm.mjs --force` re-runs the whole wasm32 release
  chain — only with a demonstrated guard bug). For one-shot verification
  runs set `CARGO_INCREMENTAL=0`: incremental intermediates are pure
  extra writes when nothing will build on top of them.
- **Reuse test harnesses where the repo provides it** (e.g.
  `MUSICPACK_E2E_REUSE=1` keeps the web e2e server + fixture library up
  across targeted spec runs; CI never reuses). When adding tests, keep
  fixtures small and harness work idempotent/single-pass (hash a library
  once, not per stage).
- **Clean generated test output** (`test-results/`, diff/actual
  artifacts) instead of letting failed-run debris accumulate; debug
  traces stay opt-in (`on-first-retry`).
- Do not weaken or skip tests to save time — reduce *how often* and *how
  much* runs, not coverage.
