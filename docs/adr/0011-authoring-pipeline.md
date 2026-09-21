# ADR 0011: Rust authoring pipeline (`musicpack-author`)

- Status: Accepted (2026-09-21, R4.2)

## Context

R4.1 added the core package-directory builder
(`musicpack-core::authoring::build_directory`) and promoted collector
identity (`docs/adr/0010-core-package-builder.md`). R3.7 identified the next
step (`docs/r3.7-r4-readiness.md` §13, R4.2): a Rust authoring pipeline that
replaces the functional responsibilities of the legacy C `musicpack`
authoring CLI while the existing Author runtime stays available.

The C pipeline (`inspect`, `validate-draft`, `identify-draft`,
`encode-draft`, `waveform-draft`, `build-draft`, `pack`) splits its work
between the CLI, `libmusicpack` and `mpcenc`. Its *transport* (MusicBrainz
HTTP) already lives in the Rust Author host; its *matching* lives in C.

## Decision

1. **New application crate `crates/musicpack-author`** with a library API
   and one coherent CLI (`validate`, `identify`, `build`). It owns only
   orchestration: draft JSON, source discovery, encoding, waveform
   accumulation, MusicBrainz matching, assembling
   `AuthoringDraft`, invoking the core builder, and optional `.mpak`
   packing.
2. **The core builder remains the sole package constructor.** The pipeline
   never writes `manifest.json`, never re-implements path validation,
   verification, identity or lyrics, and reuses
   `storage::directory::pack_directory` for `.mpak`.
3. **JSON draft surface** mirrors the C draft schema (plus the R3.5
   per-track `lyrics[]`). Only authored input is represented; derived
   artifacts a legacy stage may have written (`waveformAnalysis`, durations,
   measurements, hashes) are ignored and re-derived.
4. **Encoder boundary preserved.** Encoding calls the isolated
   LGPL-2.1-or-later `musicpack-musepack-encoder` crate. No encoder logic is
   copied into this crate and no C encoder is invoked. Packaging the crate
   as BSD-3-Clause (original orchestration) while depending on the isolated
   LGPL encoder mirrors how an application consumes an LGPL library; the
   encoder's own license and isolation are unchanged. Revisit if the R4.3
   cutover requires a different linkage.
5. **MusicBrainz provider abstraction.** Transport is injected via the
   `MusicBrainzProvider` trait; matching, candidate extraction and draft
   application are pure and deterministic (`identify`). The CLI is
   offline-only (`--mb-json`/`--mb-search-json`); the Tauri host supplies
   live transport at R4.3.
6. **Waveform generated, loudness delegated.** The pipeline decodes the
   packaged audio and writes canonical v1 envelopes via the core
   `WaveformAccumulator`; loudness/duration stay with the core builder
   (`LoudnessMode`, default `Measure`, `--no-loudness` to omit).
7. **No legacy runtime dependency.** The Rust pipeline never invokes the C
   `musicpack` CLI, `mpcenc` or `musicpack-sonic`, never spawns a
   subprocess, and adds no HTTP client or FFmpeg. R4.3 performs the cutover.

## Consequences

- A package can be authored end-to-end in Rust and is accepted by the core
  verifier, the C verifier, and the server ingestion path
  (`crates/musicpack-server/tests/author_pipeline.rs`).
- MusicBrainz matching is now Rust and tested against a mock provider; live
  transport stays host-side.
- Two intentional, documented differences remain: the encoder's frozen
  `(quality, sample-rate)` matrix (rejected fail-closed rather than silently
  remapped) and `--sync-tags`/sonic analysis (deferred; not required for
  package correctness).
- R4.3 can cut the Tauri app over by calling this library/CLI; the C
  sidecars are not yet removed.
