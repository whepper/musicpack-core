# Musepack fixtures

Six SV8 (`.mpc`) fixtures committed for the Phase 13 container/metadata tests
and the deferred audio-synthesis differential work.

**Provenance.** Copied verbatim from the reference repository's own test
fixtures (`tests/fixtures/` in the sibling MusicPack checkout). They are
**generated sine tones** produced by the project's `mpcenc` encoder, not
commercial music (none of the copyrighted reference album media is committed
here), and they carry the project's BSD-3-Clause licensing.

| File | Rate | Notes |
|---|---|---|
| `sine32-q8.mpc` | 32 kHz | quality 8 |
| `sine37-q4.mpc` | 37.8 kHz | quality 4 |
| `sine44-q5.mpc` | 44.1 kHz | quality 5 |
| `sine44-q7.mpc` | 44.1 kHz | quality 7 |
| `sine44-q5-48s.mpc` | 44.1 kHz | ~48 s long stream |
| `sine48-q6.mpc` | 48 kHz | quality 6 |

`tests/data/musepack_oracle.jsonl` is generated from these files by
`tools/musepack_oracle.mjs` (which runs the project's vendored libmpcdec built
to WebAssembly). The Rust decoder must reproduce the reference PCM byte-for-byte
(`tests/musepack_oracle.rs`); the oracle records each file's SHA-256 so a swap
is detected. `tools/musepack_dump_pcm.mjs` writes raw reference PCM for
sample-level diffing during development.
