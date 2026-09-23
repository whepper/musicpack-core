# R4 completion — closure audit and final legacy inventory

Status: **R4 closed** (R4.1–R4.5). This document records the end-state audit
the R3.7 readiness plan asked for (`docs/r3.7-r4-readiness.md` §13), the
classification of every remaining legacy component, and the final gate
results. Decisions: ADR 0010–0014. Server operations:
`docs/server-production.md`.

## 1. R4 closure audit

| Area | Final state | Evidence |
|---|---|---|
| **Author** | Rust-authoritative. `author/` runs the in-process `RustBackend` over `musicpack-author`; the C-CLI lane is a non-default escape hatch (`MUSICPACK_AUTHOR_LEGACY=1`). | `docs/author-runtime.md`, ADR 0012, R4.3 gates |
| **Package** | Rust-authoritative. `musicpack_core::authoring::build_directory` is the sole constructor; no second builder/serializer/verifier. | ADR 0010, `docs/package-builder.md`, R4.1 differentials |
| **`.mpak` writer** | Rust-authoritative: `storage::directory::pack_directory`; the R3.5 lyric-specific branch is gone. | R4.1/R4.2 gates |
| **Server** | Rust-authoritative. Release build verified against the live C oracle; deployment documented; C server oracle-only. | `docs/server-production.md`, ADR 0014, R4.5 release differential (188/0) |
| **Online player** | Rust-authoritative. `chooseBackend` routes Musepack/FLAC/WAV to the Rust/WASM engine. | `docs/web-offline-playback.md`, R4.4 gates |
| **Offline/OPFS player** | Rust-authoritative. OPFS bytes are served to the same `WasmEngine` through the range callback. | ADR 0013, R4.4 gates |
| **Encoder** | Isolated LGPL-2.1-or-later `musicpack-musepack-encoder`; used by the Author pipeline; no C encoder in the runtime. | ADR 0011/0012, `encoder_whole` corpus |
| **Lyrics** | Complete vertical: Author → package → server → online/offline player (R3.2–R3.6). | `crates/musicpack-server/tests/lyrics_server.rs` (13), `web` lyrics specs |
| **Legacy runtime** | Removed from every production path; retained only as oracle, historical reference, or the explicit development escape hatch. | §2 below |

## 2. Final legacy inventory

Every remaining non-Rust-origin component, classified exactly once.

| Component | Location | Classification | Notes |
|---|---|---|---|
| `musicpack` C CLI (inspect/validate/build/… authoring pipeline) | `../musicpack` | HISTORICAL REFERENCE | Immutable oracle repo; the Author runtime no longer invokes it (R4.3). Used by `musicpack-author` differentials. |
| `mpcenc` (C encoder) | `../musicpack/build/codec/mpcenc` | TEST ORACLE | Encoder byte-identity differential only; no runtime path. |
| `musicpack-sonic` (C + ONNX) | `../musicpack/build` | DEVELOPMENT ESCAPE HATCH | Reachable only behind `MUSICPACK_AUTHOR_LEGACY=1`; the capability itself is retired from the product (ADR 0012 §5). |
| Frozen Emscripten Musepack decoder (`musepack.js`/`.wasm` + readers) | `web/app/public/` | TEST ORACLE | Differential lane only (`musicpack.oracle-decoder.v1`, test helper); removed from production playback (ADR 0013). |
| C server (`musicpack-server` C) | `../musicpack/build/server` | TEST ORACLE | Live differential oracle; documented rollback option on the same database file. Never the production runtime. |
| Encoder compatibility corpus (frozen 282-vector manifest, `tests/data`) | `crates/musicpack-musepack-encoder/` | TEST ORACLE | Permanent bit-exactness boundary. |
| C server compatibility corpus (C-created DB fixture, `*_oracle` suites, oracle `.jsonl`) | `crates/musicpack-server/tests/` | TEST ORACLE | Permanent DB/API contract pinning. |
| Old Web decoder fixtures / provenance | `web/app/public/PROVENANCE-legacy-decoder.md` | HISTORICAL REFERENCE | Provenance record for the frozen artifact. |
| `tools/` C/Python/MJS generators | `tools/` | TEST ORACLE | Fixture/oracle generators; never wired into builds. |
| Legacy repository | `../musicpack` | HISTORICAL REFERENCE | Immutable; never modified by R4 work. |
| `author/src-tauri` C-CLI resolution (`author_service.rs`) | `author/src-tauri/src/` | DEVELOPMENT ESCAPE HATCH | Non-default, removable without touching the Rust path. |
| `web/player-core` (TS playback domain) | `web/player-core/` | PRODUCTION | Web orchestrator by ADR 0003 (Rust owns the decoder/engine). |
| `musicpack-mpc-tools` (LGPL tool library) | `crates/musicpack-mpc-tools/` | DEFERRED PRODUCT CAPABILITY | Complete, oracled, but no runtime consumer yet. |

Production components (all Rust unless noted): `musicpack-core`,
`musicpack-engine`, `musicpack-host`, `musicpack-wasm`, `musicpack`
(CLI), `musicpack-server`, `musicpack-author`,
`musicpack-musepack-encoder` (LGPL boundary), the Author frontend and the Web
app (`web/`) with the frozen shell assets in `web/app/public/`.

## 3. Final architecture

```text
Author (Tauri) ──► musicpack-author ──► musicpack-core::authoring
                                          │  build_directory / pack_directory
                                          ▼
                                     .mpack / .mpak
                                          │
        (Rust server) musicpack-server ────┤ ingestion + API
                                          ▼
                       Web player ──► Rust/WASM engine (online + OPFS offline)
```

No legacy binary, FFI, FFmpeg, interpreter or subprocess is on any production
path.

## 4. Performance

Release server, one fixture package: scan+verify 18 ms, startup→health 33 ms,
health 14.5 ms, albums 17.5 ms, track detail 17.2 ms, 64 KiB range 17.1 ms,
8 concurrent ranges 23 ms total, graceful drain 74 ms (`tools/server_bench.sh`).
Offline Rust playback: time-to-playing ≈ 125 ms, seek ≈ 148 ms (R4.4 e2e).

## 5. Gate results (R4.5)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo test --workspace` | **857 passed / 0 failed** |
| `cargo clippy --workspace --all-targets -D warnings` | clean |
| server + live-C (debug) | **188 passed / 0 failed** |
| **server + live-C (`--release`)** | **188 passed / 0 failed** |
| `cargo build --release -p musicpack-server` | ok (single arm64 Mach-O, 3.1 MB, libc only) |
| WASM check + clippy (core; engine/host/wasm) | clean |
| fuzz targets | compile clean |
| `encoder_whole` / `cut_compat` | 2 / 1 passed |
| Web unit / offline unit | 472 (2 skipped) / 52 passed |
| Web svelte-check | 0 errors, 2 pre-existing warnings |
| Web e2e ui / visual / playback | **51 / 8 / 66 green under the CI retry policy** (124 passed + 1 flaky-pass on retry; see note) |
| Web e2e against the **release** server (offline + differential) | 15 passed |
| Author native (fmt/clippy/tests) | 84 passed, clean |
| Author frontend (unit/component/svelte-check) | 38 / 54, 0 errors |

## 6. Remaining work (post-R4)

- **E2E timing-flake hardening (web, oracle lane only).** The `chained
  musepack fades (BUG-1)` differential test (frozen-decoder lane) is timing
  sensitive: it races waveform-profile fetches against the fade window, and a
  late-engaging fade can produce a sampled clock re-anchor step or a
  declined-after-call. Observed 4 failures across 5 long runs in two modes,
  always green solo and green on the configured retry; no production path is
  involved. A follow-up may tolerate the re-anchor sample or make the spy
  distinguish "declined before call" from "declined after call" — explicitly
  not done in R4.5 (no web-player changes per scope).
- **Deployment execution** — requires infrastructure/credentials that do not
  exist in this repository (see `docs/server-production.md` §11).
- **Encoder fractional-quality parity (J.2)** — **completed by the J.2
  slice**: C-compatible `--quality` values such as `4.25`/`5.5` (profile
  interpolation) and out-of-range clipping are byte-parity for any finite
  `f32` quality at the four SV8 rates via the frozen-ATH-base deterministic
  path (ADR 0012 §7 amended); the sparse fractional corpus
  (`tests/data/encoder/fractional_manifest.txt`, 27 rows incl. a tonal
  signal) is byte-matched to the scalar C reference, non-finite qualities
  are rejected as an intentional boundary, and the full **integer** matrix
  (`0..=10` × 44.1/48/37.8/32 kHz, 44 configurations) plus mono differential
  coverage remain frozen regression oracles.
- **Embedded artwork extraction** — **completed by the embedded-artwork
  slice**: FLAC `PICTURE` and APEv2 `Cover Art (Front)` pictures
  (JPEG/PNG by signature, external `front` wins, other roles fill in
  deterministic order) are discovered at album scan and extracted
  byte-exactly at staging (see `docs/author-pipeline.md` §3).
- **SV7** — permanently out of scope unless SV7 is un-retired (ADR 0014 §5).
- **`musicpack-mpc-tools`** — needs a product workflow before it gains a
  runtime consumer.

## 7. Status

```text
R4 COMPLETE — Rust-authoritative runtime; production deployment pending external infrastructure
```

Repository-level production readiness is complete; no production deployment
was performed and none is claimed.
