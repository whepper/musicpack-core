# musicpack-core

Canonical, reusable Rust implementation of the **MusicPack domain** and
the **`.mpack` v1** album-package format (including the **MPAK v1**
single-file container and the **waveform envelope v1** format).

It is a library: the same code is intended to run inside native Rust
applications, the existing MusicPack server, WebAssembly/browser code via
a binding crate, and CLI tooling.

## What it is

- The `.mpack` v1 manifest model: strict parsing, closed enumerations,
  canonical serialization, resource budgets.
- The canonical package-relative path rules and SHA-256 declaration and
  digest computation.
- Package verification: asset existence, regular-file/link-count checks,
  path containment, SHA-256, per-file/aggregate budgets, and a
  deterministic errors-vs-warnings report — through a platform-independent
  storage backend (POSIX directory-bundle and MPAK container adapters ship
  today).
- The MPAK v1 container: scan-oriented reading, recovery semantics,
  deterministic writing (byte-identical to reference packs), and
  container consistency verification (`INDX`/`TAIL`/version/duplicates).
- Audio-domain primitives: the PCM contract, the decode seam, a native
  RIFF/WAVE reader (8/16/24/32-bit PCM and 32-bit IEEE float,
  `WAVE_FORMAT_EXTENSIBLE`), a FLAC decoder adapter (pure-Rust `claxon`),
  a native Musepack SV8 decoder (see below),
  the streaming waveform-envelope accumulator, and BS.1770-5 integrated
  loudness + true-peak metering (a hand-port of the reference's vendored
  ebur128 subset).
- The platform-independent player core: queue model with repeat/shuffle and
  bounded history, a synchronous engine seam, the BS.1770 gain policy,
  session snapshots, typed events, gapless/standby orchestration, and the
  crossfade / "Sweet Fades" transition planner with EOS-jump repair and
  boundary-drift diagnostics (a behavioural port of the TypeScript
  `web/player-core`).
- A deterministic **engine adapter** (`crates/musicpack-engine`) that bridges
  the player's `Engine` seam to decoded PCM: a bounded interleaved ring, a
  streaming linear resampler, an equal-power crossfade mixer with the
  reference swap accounting, source/decoder factories, and a host-driven
  `consume()` output seam. It is `std`-only, safe Rust, and contains no
  device, browser, network, filesystem, timer, thread, or async code.
- A thin **WASM binding foundation** (`crates/musicpack-wasm`) exposing
  byte-backed decode handles and a player handle to JavaScript with
  `wasm-bindgen`; it is not the browser application, the Web Audio engine,
  the HTTP layer, persistence, or UI.
- The **representation-selection policy** (`policy`): the pure, deterministic,
  I/O-free rules that decide which of a track's audio representations is
  played, ported from the reference's Phase 4 resolver and differentially
  replayed against it.
- A **host adapter** (`crates/musicpack-host`) that owns the host concerns —
  a source backend, the shared engine handle, and the pull loop a device or
  AudioWorklet callback drives — plus the mapping from the policy decision to
  a concrete source URL.

## What it is not

- Not the MusicPack application: no HTTP server, no Svelte/UI, no
  browser or Node.js APIs, no authentication or sessions, no database
  access, no deployment tooling.
- **Codec support**: FLAC decoding is delegated to the pure-Rust `claxon`
  adapter; WAV is a native reader; **Musepack SV8 is decoded natively** in safe
  Rust (`audio::musepack`, a port of the project's vendored `libmpcdec`) with
  PCM verified byte-for-byte against the reference. No codec detection or
  codec-specific branch exists outside the decoder seam.
- Not a redesign. The existing implementation
  (`core/libmusicpack` in C, `web/player-core` in TypeScript) and the
  normative v1 specs are the behavioural reference; this crate ports
  behaviour, it does not change it.

## Relationship to the existing MusicPack application

The existing repository remains the reference implementation and the
source of truth:

| Domain | Reference | Rust home |
|---|---|---|
| `.mpack` manifest, paths, checksums | `core/libmusicpack` (C) | `format::manifest`, `format::path`, `format::checksum` |
| Package verification | `musicpack_package_verify` (`package.c`) | `storage`, `validation` |
| MPAK v1 container | `core/libmusicpack/src/mpak.c` | `format::mpak` (phase 6) |
| Waveform envelope | `specs/musicpack-waveform-v1.md`, `src/waveform.c` | `format::waveform`, `audio` |
| Loudness (BS.1770-5) | `src/loudness.c` + vendored `libebur128` | `audio` |
| Player core | `web/player-core` (TypeScript) | `player` |
| PCM engine adapter (ring/resample/mix) | `web/app/.../audio-worklet.ts` (+ `ring-buffer.ts`, `streaming-resampler.ts`) | `crates/musicpack-engine` |
| WASM binding layer | `web` wasm host glue | `crates/musicpack-wasm` |
| Representation selection, playback policy | `web/app/src/lib/state/representation-selection.ts` (Phase 4) | `policy` (representation); `player::gain` (playback loudness) |
| Host pull loop (source → engine → output) | `web/app/src/lib/playback/*` + `PlayerController` | `crates/musicpack-host` |

Until a domain is ported and differentially verified, the reference
implementation remains authoritative for it.

## Architecture

```text
                ┌──────────────────────────┐
                │      musicpack-core      │
                │  format · validation ·   │
                │  audio · player · policy │
                └──────────────────────────┘
                 │            │            │
     native Rust │  server    │ musicpack- │ CLI
     apps        │  adapters  │ wasm       │ (differential harness)
                 ▼            ▼            ▼
             filesystem   browser /     corpus
                          Svelte app    runner
                             ▲
                             │
                ┌──────────────────────────┐
                │   musicpack-engine       │  ring · resampler · mixer ·
                │ (deterministic adapter)  │  decoder/source factories
                └──────────────────────────┘
```

The core performs no I/O of its own: storage backends and transports are
trait-based adapters outside the crate, which is what keeps WebAssembly a
first-class target. The engine adapter depends on the core (never the
reverse) and itself performs no I/O: a host supplies a `SourceBackend` and
drives the adapter by pumping and consuming frames. See
`docs/architecture.md` for the full boundary rules and design decisions.

## Engine adapter

`crates/musicpack-engine` is the deterministic bridge from the player's
`Engine` seam to decoded PCM:

```text
PlaybackItem → SourceBackend → Read → AudioDecoder
                                         │
                  DecodeSession (decoder + bounded ring + resampler)
                                         │
                              DecoderEngine (player::Engine)
                                  │            │
                             consume()   prepare_next / advance
                                  │       begin_crossfade (lane + mixer)
                             host output
```

- **The core stays frozen and sample-blind.** PCM, buffering, resampling and
  crossfade mixing live in the adapter; the player still sees only engine
  facts (`rendered_samples`, `is_output_drained`, capabilities, results).
- **Rendered samples** are output-rate frames actually consumed since the
  last open/seek reset (the ring read playhead), rebased at a crossfade swap
  so the album clock stays continuous.
- **Seek** is codec-neutral reopen + skip N output frames — intentionally
  simple and possibly expensive. `AudioDecoder` is unchanged.
- **Sources** are abstract readable bytes. `MemorySourceBackend` (tests,
  embedded) and `PackageSourceBackend` (an existing core `PackageBackend`,
  e.g. MPAK members) ship; filesystem, HTTP and OPFS belong to the host.
- **Musepack** decodes natively behind the same `DecoderFactory`: the SV8
  container/metadata parser and the DSP path (bitstream, Huffman/CAN,
  requantisation, synthesis filterbank) are implemented in `audio::musepack`
  and registered in `audio::open`. PCM is byte-identical to the reference.
- The **ring, resampler and equal-power mixer are byte-exact ports** of the
  reference TypeScript; `crates/musicpack-engine/tests/xfade_oracle.rs`
  replays `tests/data/xfade_oracle.jsonl`, generated by
  `tools/xfade_oracle.ts` from the real worklet processor.

## Host integration and representation policy

`crates/musicpack-host` is the thinnest practical host adapter. It owns a
source backend, the shared engine handle, and the pull loop a device or
AudioWorklet callback drives:

```text
host source (bytes / MPAK member / range)   ← SourceBackend
      │
      ▼
musicpack-engine DecoderEngine (decode · ring · resample · mix)
      │
      ▼  Host::render(frames)   (the device/worklet pull)
  host output
```

`Host::render` calls `DecoderEngine::consume()` and feeds every engine fact
back into the player (`on_crossfade_complete`, `on_engine_error`, `on_eos`,
`on_tick`). The engine never sees a clock, device, or browser API; a paused
host simply stops pulling.

`policy::representation` is the ported Phase 4 resolver: given a track's
candidates (primary + representations in manifest order), a preference
(`default` / `representation` / `codec` / `lossless`), and an injected
playability predicate, it returns the chosen representation deterministically
(manifest order is the only tie-break; a playable primary always wins; an
unplayable primary may be rescued by a playable alternate). It is pure,
total, I/O-free, and does not know about the player.
`tools/policy_oracle.ts` regenerates `tests/data/policy_oracle.jsonl` from the
reference resolver, and `tests/policy_oracle.rs` requires identical
selections (32 records).

## Compatibility

Compatibility with the existing implementation is a **core requirement**,
not a nice-to-have:

- Behavioural parity is verified by **differential testing**: the same
  corpus is run through the reference `musicpack` CLI and the Rust
  implementation, and accept/reject decisions are compared (the 72-case
  v1 conformance corpus, canonical manifests rewritten by the reference
  tooling, and a committed table of 3394 `%.8g` number vectors generated
  from the real C `snprintf`).
- Canonical serialization is **byte-identical** (demonstrated by tests);
  the reference's own undefined behaviours in this area are documented in
  `docs/architecture.md` §8 rather than approximated.
- Compatibility expectations are never weakened to make Rust tests pass.
  Where a behavioural difference is unavoidable (e.g. rejecting
  non-UTF-8 JSON bytes), it is documented and corpus-checked.

## Migration strategy

Ported in dependency order — the format must be proven compatible before
player/audio work begins:

1. repository/toolchain foundation *(this repository's current state)*
2. domain types
3. `.mpack` format model + strict JSON parser
4. canonical writer + round-trip tests
5. validation + SHA-256
6. MPAK v1 parser/writer + byte-compat tests
7. **compatibility gate** (differential corpus harness)
8. audio primitives *(PCM contract, decode seam, native WAV reader,
   `claxon` FLAC adapter, differential/hostile tests — done)*
9. waveform + BS.1770-5 metering *(streaming accumulator, ebur128 subset
   port, reference-golden differential — done)*
10. player core (queue/engine/gain/snapshot/events) *(ported and
    differentially replayed against `web/player-core` — done)*
11. crossfade + Sweet Fades transitions *(the pure planner and crossfade/EOS
    orchestration are included in phase 10; the deterministic PCM engine
    adapter with ring, resampler, equal-power mixer and swap accounting is
    `crates/musicpack-engine` — done)*
12. representation-selection policy *(ported in `policy` and differentially
    replayed against the reference Phase 4 resolver — done)*
13. `musicpack-wasm` bindings *(a thin byte-backed binding foundation is
    `crates/musicpack-wasm` — done; the production browser cutover is not)*
14. integration with the existing application
15. performance optimization

## Repository layout

The root is both the **core package** and a Cargo workspace (a deliberate
adaptation of the "everything under `crates/`" layout — see
`docs/architecture.md` O8):

```text
Cargo.toml            # package musicpack-core + [workspace]
src/ tests/ fixtures/ tools/ docs/
crates/musicpack/         # the thin `musicpack` CLI (info | verify | pack | unpack)
crates/musicpack-engine/  # deterministic platform-neutral PCM engine adapter
crates/musicpack-host/    # thinnest host adapter (pull loop + source selection)
crates/musicpack-wasm/    # thin wasm-bindgen binding foundation
crates/musicpack-musepack-encoder/  # Rust Musepack SV8 encoder (LGPL-2.1-or-later; see its README)
fuzz/                 # cargo-fuzz targets (excluded from the workspace; nightly)
```

The core is independently usable as a library; the CLI, the engine adapter,
the host adapter and the WASM binding depend on it, never the reverse. The
engine adapter depends only on the core and the standard library; the host
adapter depends on the core and the engine.

`crates/musicpack-musepack-encoder` is the safe-Rust replacement for the
legacy C Musepack encoder. It is **not** depended on by the core or by any
other crate; it is licensed **LGPL-2.1-or-later** (a source-derived
compatibility implementation of the legacy LGPL encoder, treated
conservatively). Parity is reached (Phase 15L): the legacy C repository is
the immutable historical reference (never modified), and the frozen
compatibility corpus in the encoder crate is the permanent compatibility
boundary. See its README for the licensing boundary and the frozen corpus.

### CLI

```sh
cargo build -p musicpack
musicpack info   <package-dir|file.mpak>
musicpack verify <package-dir|file.mpak> [-q|--quiet] [--json]
musicpack pack   <package-dir> <output.mpak>
musicpack unpack <input.mpak> <directory>
```

`verify` prints `error:`/`warning:` findings and a summary, exits non-zero
exactly when the core report contains an error (warnings never fail), and
`--json` emits the reference's `{ "ok", "errors", "warnings" }` shape.
`pack` only packs verified bundles and publishes atomically after
re-verifying the container. `unpack` is the application-layer safe
extractor (canonical path re-validation, fresh staging tree, `create_new`
writes, regular files only, aggregate budget) — see the unpack safety model
in `docs/architecture.md`.

## Building and testing

Requires stable Rust (edition 2024, MSRV 1.85):

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo check -p musicpack-core  --target wasm32-unknown-unknown   # Wasm targets stay green
cargo check -p musicpack-engine --target wasm32-unknown-unknown
cargo check -p musicpack-host --target wasm32-unknown-unknown
cargo doc --no-deps -p musicpack-core -p musicpack-engine -p musicpack-wasm
```

The WASM binding has a Node smoke test (no browser, no `wasm-pack`):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128
tools/wasm_smoke.sh
```

Conventions: `#![forbid(unsafe_code)]`, documented public API
(`#![warn(missing_docs)]`), rustfmt defaults, clippy deny-by-default in
CI (Linux/macOS/Windows plus a wasm32 check job). Integration tests live
in `tests/`, the compatibility corpus plan in `fixtures/README.md`.

For the differential harness against the reference CLI, build the
reference once and point the tests at it (optional — the corpus gate
runs without it):

```sh
cmake -S ../musicpack -B ../musicpack/build -DMPC_BUILD_TESTS=OFF \
      -DMPC_BUILD_SERVER=OFF -DMPC_BUILD_MPCGAIN=OFF -DMPC_BUILD_MPCCHAP=OFF
cmake --build ../musicpack/build --target musicpack_cmd -j
cargo test   # conformance_differential now compares against the CLI
```

## Fuzzing

The `fuzz/` directory holds `cargo-fuzz` targets for the untrusted-input
boundaries (strict JSON/manifest parsing, the MPAK scanner,
the audio decode seam, the analysis feed paths, the player core, the
engine adapter's ring/resampler/mixer, the representation policy, and the
Musepack SV8 parser). They
are excluded from the workspace and are not part of `cargo test`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run json_manifest
cargo +nightly fuzz run mpak_scan
cargo +nightly fuzz run audio_decode
cargo +nightly fuzz run analysis_feed
cargo +nightly fuzz run player_state
cargo +nightly fuzz run engine_adapter
cargo +nightly fuzz run policy
cargo +nightly fuzz run musepack
```

The deterministic counterpart runs in CI: `tests/fuzz_lite.rs` replays 83
seeded mutations (truncations, CPython-identical bit flips, path
injections) through the parser/verifier, and through the reference CLI when
one is built. See `fuzz/README.md`.

## Dependencies

`sha2` (RustCrypto) — required by the `.mpack` integrity model and chosen
for its maturity, wasm32 support, MSRV, and permissive licence. `claxon`
(pure-Rust FLAC decoder, Apache-2.0, zero runtime dependencies) — required
by the audio decode seam; it is hidden behind an adapter and never appears
in the public API. Phase 9 adds **no** dependency: the BS.1770 subset is a
hand-port of the reference's vendored libebur128 1.2.6 (MIT, attribution in
`src/audio/loudness.rs`). The engine adapter adds **no** dependency either:
the ring, resampler and equal-power mixer are hand-ports of the reference
TypeScript and use only `std`. Only the WASM binding crate adds a
dependency (`wasm-bindgen` + `js-sys`, the binding mechanism itself). See
`docs/architecture.md` §6 for the full policy,
the `claxon` unsafe audit, and the assessment of future candidates
(`proptest`, `cargo-fuzz`, `criterion`).

## License

BSD-3-Clause — see [LICENSE](LICENSE).
