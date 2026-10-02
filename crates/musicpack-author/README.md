# musicpack-author

The Rust **authoring pipeline** (R4.2): album directory/package → draft
JSON → validate → identify → encode → waveform → `.mpack` → optional
`.mpak`.

It replaces the functional responsibilities of the legacy C `musicpack`
authoring CLI, while the existing Author application keeps using the C
runtime until the R4.3 cutover.

```text
scan (album dir) ─► draft JSON ─► validate ─► identify ─► encode ─► waveform ─► build ─► pack
                                                        │
                             musicpack_core::authoring::build_directory
```

## What lives here

* `scan` / `inspect` — fresh-album discovery (album directory → draft
  JSON: files, tags, order, assets) and the existing-package → draft
  mapping; `inspect::open_to_draft` dispatches between the two.
* `draft` — the JSON draft surface and structural validation.
* `encode` — FLAC/integer-WAV → Musepack SV8 via the isolated
  `musicpack-musepack-encoder` crate (no `mpcenc`, no FFmpeg).
* `waveform` — canonical v1 envelopes via the core `WaveformAccumulator`.
* `identify` — MusicBrainz matching/application behind a mockable
  `MusicBrainzProvider` (transport is the host's concern).
* `pipeline` — the coherent end-to-end run that assembles the core
  `AuthoringDraft` and calls the sole package constructor.
* `similarity` — the model-neutral similarity producer boundary
  (`SimilarityProducer`, `SimilarityProfile`, `TrackSimilarity`,
  `.msim` writer, `SimilarityCache` contract): a future concrete model
  plugs in here; no model, runtime, or download lives in this crate.
* `similarity_effnet` — the Discogs-EffNet concrete profiles (multi
  1280-D, release 512-D): exact model identities, ported DSP, rten
  inference. Operator-supplied weights only; see below.
* `cli` / `main` — `validate` / `identify` / `build` / `inspect`.

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

## Similarity producer boundary (Slice 0)

The Author is the **producer** of similarity data; the Server
consumes and indexes supplied vectors and never runs inference.
`pipeline::run_with_similarity` (plus the cancellable
`pipeline::similarity_stage_with`) drives a caller-supplied
`similarity::SimilarityProducer` over staged audio and writes the
result as the package's optional `similarity` analysis document:

- **Optional.** Disabled by default; a package without similarity data is
  completely valid, and similarity failures never invalidate a package.
- **Model-neutral.** The boundary names no model family and takes no
  runtime. Concrete models are profiles plugged into `SimilarityProducer`;
  Discogs-EffNet remains unresolved and is not bundled, and model weights
  remain operator-supplied — MusicPack never silently downloads weights.
- **Profile identity.** The producer supplies the exact `profile_id` and
  `profile_fingerprint`; the fingerprint (never the display id) keys the
  cache and the server partition. Nothing here derives or invents one.
- **Cache.** `SimilarityCache` is keyed by analyzed-audio SHA-256 plus
  profile fingerprint (which already binds preprocessing, dimensions, and
  encoding); only successful vectors are stored, invalidation is explicit,
  and persistence/bounding belong to the host.
- **`f32le` only.** G-6 closed precision as KEEP F32LE; the writer has no
  `f16le` path.
- **Licensing is separate from the mechanism.** Whether a concrete profile
  may be supported — and whether generated embeddings are legally
  unrestricted — is unresolved (ADR 0017 G-1…G-4). No claim is made here
  either way.

## Discogs-EffNet profiles (experimental, licensing-restricted)

`similarity_effnet` implements two concrete profiles over the generic
boundary above — multi / 1280-D
(`musicpack-similarity-discogs-effnet-multi-v1`) and release / 512-D
(`musicpack-similarity-discogs-effnet-release-v1`) — with exact model
artifact identities, ported preprocessing, and rten 0.26.0 inference.
`EffNetProducer::{multi,release}` take an operator-supplied model path,
verify its SHA-256 before opening it, and never download, fetch, or
resolve anything over the network.

Licensing posture, stated plainly because it constrains use:

- The model source/rightsholder evidence is the Essentia Discogs-EffNet
  release (`EffnetDiscogs` v1, 2022-06-15); the research records
  conflicting license notices on MTG's public pages and an unconfirmed
  artifact-level governing license.
- Generated-embedding licensing status is unresolved. Nothing here claims
  embeddings are unrestricted, commercially usable, or open source.
- Commercial use is not cleared. Model weights are operator-supplied and
  never committed, bundled, downloaded, or redistributed by this
  repository. Users are responsible for complying with the applicable
  model license.
- G-1…G-4 remain explicitly open until qualified review or written
  clarification resolves them. This profile is optional and experimental;
  similarity stays optional end to end.
- Precision: deterministic `f32le` output (G-6 closed as KEEP F32LE).
  G-7 (human listening review) is closed as a technical release gate (ADR
  0017 §10.5): technical completeness claims nothing about musical
  usefulness, and MusicPack does not claim to have independently validated
  the model's scientific quality. Optional human listening may still be
  performed as product-quality feedback.

## Toolchain scope (G-5: PASS WITH SCOPED EXCEPTION)

MusicPack baseline remains Rust 1.85. Discogs-EffNet inference is optional
and default-off behind the `discogs-effnet` cargo feature:

- Default builds never compile `rten` and keep the 1.85 promise; profile
  metadata, DSP, writer, and cache behavior carry no higher requirement.
- Enabling the feature compiles `rten 0.26.0` and requires Rust 1.94,
  which fails loudly (a Cargo MSRV error naming the package) on older
  toolchains — never silently.
- This higher requirement does not apply to the default MusicPack build,
  and the workspace MSRV is unchanged.
