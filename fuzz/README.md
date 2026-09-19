# Fuzzing

`cargo-fuzz` targets for the untrusted-input boundaries of
`musicpack-core` and `musicpack-engine`. This directory is **excluded from
the workspace** (root
`Cargo.toml`, `exclude = ["fuzz"]`) and is never built by `cargo test` or
CI: libFuzzer linking needs a nightly toolchain plus the `libfuzzer-sys`
runtime, which must not enter the production dependency graph.

## Targets

| Target | Boundary | Invariants |
|--------|----------|------------|
| `json_manifest` | strict JSON + manifest parser | no panic, termination, bounded resource use, accepted input round-trips |
| `mpak_scan` | MPAK container scanner (normal + recovery) | no panic, no infinite scan, no out-of-bounds read, no allocation from untrusted lengths, CRC-before-length, checked arithmetic |
| `audio_decode` | audio seam (WAV reader + FLAC adapter) | no panic (third-party FLAC panics are caught), termination, buffers sized only from the caller, checked WAV chunk arithmetic |
| `analysis_feed` | Phase 9 analysis (waveform accumulator + loudness meter) | no panic on arbitrary f32 bit patterns (NaN/±Inf/huge), termination across arbitrary chunk boundaries, bounded memory, no overflow into indexing/allocation |
| `player_state` | Phase 10 player core (snapshot codec, queue ops, command sequences) | no panic on hostile snapshots / queue sequences / command orders, termination, bounded history/queue growth, adversarial shuffle RNG (`1.0`) |
| `engine_adapter` | Phase 11 PCM adapter (ring, streaming resampler, equal-power mixer) | no panic on arbitrary chunk sizes / rates / channel counts, termination, bounded fixed-capacity rings, no frame-arithmetic overflow into indexing |
| `policy` | Phase 12 representation-selection policy | no panic on arbitrary byte-derived candidates/preferences, termination, the selected index is always in range, inputs are never mutated |
| `musepack` | Phase 13B/14C Musepack SV8 decoder (streaming demux, bitstream, requantisation, synthesis) | no panic/hang on arbitrary bytes (block keys, variable-length sizes, CRC, Huffman/bitstream, truncation, short reads), termination, bounded resource use (≤ one `MAX_BLOCK_BYTES` input block), bounded decode loop |

## Running

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

Useful options: `-max_total_time=60`, `-runs=N`, `-seed=<n>`,
`-artifact_prefix=artifacts/`. Add inputs to `corpus/<target>/` to grow the
seed set; `artifacts/` holds crash reproducers.

A crash is a bug: `artifacts/<target>/crash-*` is a reproducer, and the
deterministic regression corpus in `tests/fuzz_lite.rs` should be extended
with a minimal form of it.

## Deterministic replay in normal CI

Statistical fuzzing is intentionally **not** part of `cargo test`. The
deterministic fuzz-lite replay (`tests/fuzz_lite.rs`) reproduces the
reference repository's mutation corpus (83 cases: truncations, seeded
bit-flips via a CPython-identical PRNG, path injections) and runs it
through both the Rust parser/verifier and — when a reference CLI is
available — the reference tool, asserting no panics/crashes.