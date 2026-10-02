# Sonic52 research track

A **research-only** scaffold for an independently implemented similarity
model inspired by publicly reported/reverse-engineered structural
characteristics of Plex's historical Sonic Analysis model.

> Sonic52 is an independent research model inspired by publicly
> reported/reverse-engineered structural characteristics of Plex's
> historical Sonic Analysis model. It is not "the Plex model", not Plex
> Sonic Analysis, and not a Plex reimplementation.

This phase builds the research foundation only. There is **no training,
no model weights, no inference, no audio decoding, and no production
wiring** here — and this crate must never gain any of them.

## Why this exists

MusicPack's production similarity baseline is Discogs-EffNet (1280-D
multi, 512-D release) behind the default-off `discogs-effnet` feature.
Sonic52 is a separate, alternative research direction alongside that
baseline: a compact 52-dimensional sigmoid embedding whose *output
shape* is motivated by publicly reported structural characteristics of
a historical Plex-era model. The question for a future slice is whether
an independently implemented and trained 52-D model can rival the
Discogs-EffNet baseline on the MusicPack similarity benchmark — not
whether Plex's model can be reproduced (it cannot be, from public
information, and will not be attempted).

## Placement

`experiments/sonic52-research/` is a standalone crate with its own
`[workspace]` table and **zero dependencies** — the same isolation
pattern as `experiments/music-similarity-eval/`. It is not a member of
the production Cargo workspace, takes no path dependency on any
production crate, and changes no production file. The existing
`music-similarity-eval` experiment is untouched.

The one deliberate reuse relationship is documentary and read-only: a
future 52-D `f32le` Sonic52 vector fits the frozen `.msim` container
unchanged (52 is within the `1..=4096` validation domain), so no format
change is required or proposed. This crate neither reads nor writes
`.msim` bytes.

## Research abstraction / I/O contract (`src/lib.rs`)

| Item | Value |
| --- | --- |
| Provisional profile id | `musicpack-similarity-sonic52-research-v1` |
| Embedding dimensionality | exactly 52 |
| Output element type | `f32` (`f32le` serialization, 208 bytes/vector) |
| Intended activation | sigmoid, values in `[0, 1]` (validated by `is_sigmoid_range`, not gated at construction) |
| Determinism | required of any future implementation (identical input + implementation ⇒ identical vectors) |
| Identity | profile id **plus** 32-byte fingerprint; dimension collisions never imply comparability |
| Input tensor dtype / shape | explicitly undecided (`None`; see below) |

Validation rules enforced in code: wrong dimensionality rejected,
NaN/Inf rejected, zero fingerprints rejected, empty ids rejected,
production (Discogs-EffNet) profile ids rejected as research identities,
the v1 id pinned to 52 dimensions, serialization deterministic.

## Known structural evidence vs unknowns

**Reported / publicly corroborated** (provenance in `src/lib.rs`
`PREPROCESSING_TABLE` notes):

- Network head `Conv2D → Flatten → Dense(200) → Dense(52)` (reported;
  the `Dense(200)`-then-sigmoid structure is corroborated by public
  Essentia MusiCNN-family model material).
- 52-dimensional embedding output (the reported characteristic
  motivating this track).
- Sigmoid output activation (a terminal sigmoid op is documented in
  public MusiCNN-family model material).
- Public lineage context: Essentia's public MSD-MusiCNN material
  documents 16 kHz audio input, a `[frames, 96]` mel-spectrogram
  signature, and `Dense(200)` embedding extraction; community reports
  describe Plex's historical analysis as using that model family
  (`Music.tflite`) with an Annoy cosine/angular index. None of this was
  downloaded, vendored, or reproduced here.

**Partially constrained** (bounded by lineage material, unverified for
the reported artifact): sample rate, mono/stereo handling, mel-bin
count, similarity metric.

**Unknown — must not be assumed**: FFT size, hop size, window function,
mel frequency range, logarithmic scaling, normalization, temporal window
length, input tensor shape, temporal aggregation.

**Our choices, not Plex facts**: deterministic inference is required;
the dimensionality ladder (`32 / 52 / 64 / 96 / 128`, 52 first) is our
experiment design; the profile id and fingerprint discipline follow ADR
0017. No preprocessing choices are made in this phase.

## Future experiment contract (documented, not implemented)

```
TEST CORPUS
├── Discogs-EffNet (1280-D multi, 512-D release baselines)
├── Sonic52-52 (initial target)
└── Sonic52-N (future dimensionality variants: 32 / 64 / 96 / 128)

embeddings → nearest neighbours → quantitative metrics
  → human evaluation → MusicPack similarity benchmark
```

Evidence required before Sonic52 can be evaluated against
Discogs-EffNet: an independently implemented model, a frozen
preprocessing contract with all current unknowns resolved as labelled
experimental choices, byte-deterministic inference, and a disclosed
training-data/licensing posture. Production consideration additionally
requires the ADR 0017 licensing-gate pattern resolved for the new
profile, plus benchmark results — none of which exist yet.

## Production boundary

Sonic52 must remain research-only. This scaffold does NOT, and any
future slice must not without a new ADR:

- modify or replace Discogs-EffNet, its profile IDs, or its behaviour;
- change `.msim` or `.mpak` formats;
- change production similarity ranking, server behaviour, indexing,
  playlists/radio, or the default profile set (there is none — profiles
  are explicit opt-in);
- change the web/player similarity implementation;
- add model weights, or download/vendor the Plex `Music.tflite` model;
- reproduce or redistribute proprietary Plex artifacts;
- introduce FFmpeg.

## Validation

```sh
cargo test --manifest-path experiments/sonic52-research/Cargo.toml
cargo fmt --check  # from the workspace root covers tracked files
git diff --check
```

Verification of this scaffold needs no audio, no model artifact, and no
network: the crate has zero dependencies.
