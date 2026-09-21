# musicpack-author

The Rust **authoring pipeline** (R4.2): draft JSON → validate → identify →
encode → waveform → `.mpack` → optional `.mpak`.

It replaces the functional responsibilities of the legacy C `musicpack`
authoring CLI, while the existing Author application keeps using the C
runtime until the R4.3 cutover.

```text
draft JSON ─► validate ─► identify ─► encode ─► waveform ─► build ─► pack
                                                        │
                             musicpack_core::authoring::build_directory
```

## What lives here

* `draft` — the JSON draft surface and structural validation.
* `encode` — FLAC/integer-WAV → Musepack SV8 via the isolated
  `musicpack-musepack-encoder` crate (no `mpcenc`, no FFmpeg).
* `waveform` — canonical v1 envelopes via the core `WaveformAccumulator`.
* `identify` — MusicBrainz matching/application behind a mockable
  `MusicBrainzProvider` (transport is the host's concern).
* `pipeline` — the coherent end-to-end run that assembles the core
  `AuthoringDraft` and calls the sole package constructor.
* `cli` / `main` — `validate` / `identify` / `build`.

Package semantics (path validation, hashing, canonical manifest,
verification, identity, `.mpak` framing, lyric model, loudness) stay in
`musicpack-core`. This crate owns orchestration only: it never writes
`manifest.json`, never re-implements a verifier, and never spawns a legacy
binary.

## Commands

```sh
cargo test -p musicpack-author
cargo clippy -p musicpack-author --all-targets -- -D warnings
cargo run -p musicpack-author -- build draft.json -o Album.mpack --mpak Album.mpak
```

See `docs/author-pipeline.md` for the contract and
`docs/adr/0011-authoring-pipeline.md` for the decisions.
