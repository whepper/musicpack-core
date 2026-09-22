# ADR 0012: Author runtime cutover

- Status: Accepted (2026-09-21, R4.3)

## Context

R4.1 added the core package-directory builder; R4.2 added the Rust authoring
pipeline (`crates/musicpack-author`) and proved it against the C oracle. The
Author desktop app (`author/`) still shelled out to the frozen C `musicpack`
CLI, `mpcenc` and `musicpack-sonic` for every authoring operation, bundling
them as Tauri sidecars (`author/src-tauri/tauri.conf.json` `externalBin`).

R4.3 makes the Rust pipeline the runtime and retires the sidecars, while the
C implementation stays available as a test/compatibility oracle.

## Decision

1. **The Rust authoring pipeline is the default Author runtime.** A new
   in-process adapter, `author/src-tauri/src/rust_backend.rs`, translates the
   existing UI commands onto `musicpack-author`. The production dependency
   graph is `Tauri host → musicpack-author → musicpack-core` (+ the isolated
   encoder crate). No Tauri dependency enters `musicpack-author` or
   `musicpack-core`.
2. **No silent legacy fallback.** A Rust failure is reported as a typed
   `{code, message}` error. The C-CLI path is not retried, is not the
   default, and is reachable only through the development escape hatch
   `MUSICPACK_AUTHOR_LEGACY=1` (`AuthorService`). The escape hatch is
   removable without touching the Rust path.
3. **Author API version handshake.** The UI↔host boundary carries an explicit
   `authorApi` version. `RustBackend` reports `AUTHOR_API = 8`
   (`backend_info`); the legacy CLI reports the C `MUSICPACK_AUTHOR_API`. A
   mismatch is a hard, explicit error — no compatibility guessing. The
   version belongs to the host boundary, never to core.
4. **Live MusicBrainz transport stays in the host.** `LiveMusicBrainz`
   implements `musicpack_author::MusicBrainzProvider` over the existing
   `ureq` transport (`author/src-tauri/src/musicbrainz.rs`); matching,
   candidate extraction and application remain in `musicpack-author`.
   Requests are paced to ~1/s; responses are validated before use; barcode
   search returns all candidates and never auto-selects.
5. **Sonic analysis is retired from the Author runtime.** It has no Rust
   equivalent, is not required for package correctness, and pulled in a
   C+ONNX sidecar. The `sonic_analyze` command returns an explicit
   `sonic_retired` typed error; the `musicpack-sonic` sidecar is no longer
   bundled. A package's existing `analysis[]` references are preserved
   opaquely through the draft so a rebuild does not silently drop them.
6. **`--sync-tags`/APEv2 synchronization is retired.** The Rust path rebuilds
   the package deterministically from the draft; it does not project manifest
   metadata into in-place APEv2 tags. `create_package` accepts the flag for
   UI compatibility and ignores it. This is not required for package
   correctness.
7. **Encoder limitations are gated, not remapped.** The encoder's frozen
   psychoacoustic tables cover the complete **integer** quality matrix of
   the reference encoder — qualities `0..=10` at `44100`, `48000`, `37800`
   and `32000` Hz (44 configurations). Any other pair (fractional qualities
   such as `5.5`, out-of-range qualities, other rates) fails closed with a
   typed `unsupported` error. Fractional-quality parity — the C encoder's
   interpolated `--quality` values and its clipping to `[0, 10]` — remains
   the one deferred encoder-parameter gap (tracked as J.2). Extending
   coverage is encoder-crate work, not runtime work. *(Amended by the J.1
   integer-parity slice; previously the matrix was `{4,5,6,7} @ 44100 Hz`
   plus `q5 @ {48000, 37800, 32000} Hz`, with the default q6 44.1-kHz-only
   and q8 unsupported.)*
8. **Licensing.** `musicpack-author` remains BSD-3-Clause (original
   orchestration); it depends on the isolated LGPL-2.1-or-later
   `musicpack-musepack-encoder` crate. The distributed application therefore
   includes an LGPL-2.1-or-later component; because the whole repository is
   the corresponding source and the encoder stays a separable crate, the
   LGPL's source/relinking conditions are met. No crate is re-licensed and the
   encoder isolation is unchanged.

## Consequences

- A normal Author session performs no subprocess execution: encode, waveform,
  build, verify and pack run in-process. A permanent test invariant enforces
  this (a source scan plus a poisoned-`PATH` end-to-end test).
- The C CLI / `mpcenc` / `musicpack-sonic` are no longer bundled; they remain
  as optional development oracles.
- Author packages verify with the core verifier, the C verifier, and ingest
  into the Rust server.
- Known limitations (encoder matrix, >16-bit reduction, embedded-artwork
  extraction, retired sonic/APEv2) are documented, not silent.
