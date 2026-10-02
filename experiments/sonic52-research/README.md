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
0017. Slice 1 additionally freezes the H0 frontend as our experimental
configuration (see above) — every value in it stays labelled
lineage-derived or experimental-choice per `FORENSICS.md`.

## Slice 1: deterministic H0 log-mel frontend (`src/frontend.rs`)

Implemented exactly the H0 configuration from `FORENSICS.md` §11 as
**our experimental configuration** — lineage-derived hypotheses plus
explicit experimental choices, never Plex facts:

```text
caller-provided mono 16 kHz f32 PCM
→ centered 512-sample frames, 256 hop, zero-padded edges
→ symmetric Hann, zero-phase, unnormalized (area 255.5)
→ 512-point power spectrum, bins 0..=256, no normalization
→ 96 Slaney mel bands, linear weighting, unit-triangle, 0–8000 Hz
→ log10(10000 * x + 1) (no clamp: the +1 shift floors at exactly 0)
→ 187-frame patches, stride 93, partial final patch discarded
```

Key properties, all tested (26 tests, zero dependencies):

- `Sonic52MelPatch`: exactly 187 × 96 f32, row-major, deterministic
  71,808-byte `f32le` serialization. A frontend representation — not a
  model input tensor, not an embedding, not `.msim`.
- `MelFrontend`: streaming (`push_chunk`/`finish`) with output
  byte-identical across deliberately awkward chunkings (1, 7, 511,
  512, 513, primes, empties) vs contiguous input.
- Input contract `FrontendInput::mono_16k` rejects non-16 kHz / non-mono
  instead of silently converting; resampling/downmix stay caller-side
  future choices. No decode, no resampler, no FFT/mel third-party
  dependency (local auditable radix-2 FFT; Slaney/unit-tri/Hann pinned
  by hand-computed goldens; Parseval cross-check on the FFT).
- Same-platform/toolchain determinism; cross-platform bit-identity is
  not claimed (platform libm). Exact equality for serialization and
  chunk tests; documented tolerances for DSP goldens.
- `[187, 96]` is the H0 *patch shape*, not a proven Plex tensor
  contract. Still absent: inference, weights, embeddings, ranking —
  those are future slices.

## Slice 2: real-audio ingestion (`src/ingest.rs`)

Explicit experimental pipeline — decode → sanitize → downmix →
resample → `FrontendInput` — over the **unmodified**
`musicpack-core::audio` decoder (WAV/FLAC/Musepack SV8), consumed as a
read-only path dependency (the established
`music-similarity-eval` pattern; no production code changed):

```text
input:
  WAV / FLAC / Musepack SV8 bytes (existing decoder, unchanged)

output:
  16 kHz mono f32  (+ IngestFacts: rates, channels, frame counts)

resampling:
  rubato 0.16.2 FftFixedIn, chunk 1024, 2 subchunks, 1 channel —
  same version/feature set as the in-tree Discogs-EffNet DSP path.
  Single stream state per signal (StreamResampler buffers input and
  releases whole blocks at absolute stream offsets); delay drained,
  tail truncated to round(in * 16000/src). Bypassed bit-identically
  when already 16 kHz. No FFmpeg, no tools, no Python.

downmix:
  arithmetic f64 mean (mono passes through byte-identical).
  No loudness/gain/peak normalization.

sanitization:
  NaN -> 0, +Inf -> +1, -Inf -> -1 — the exact
  musicpack-core analysis-feed mapping (mirrored; that function is
  crate-private), applied post-decode before downmix so hostile
  values cannot poison the mean or the resampler. Frontend keeps its
  own idempotent boundary check; there is one policy, not two.

frontend:
  Sonic52 H0 (Slice 1, unchanged)
```

> Resampling and downmixing are experimental Sonic52 choices. They are
> not established Plex preprocessing facts.

Corpus (all read-only reuse, no new binary fixtures, no music):
FLAC 44.1 kHz mono / FLAC 96 kHz stereo / WAV 44.1 kHz stereo /
Musepack cut fragments (11 ms and 2.1 s paths) — facts, patch counts,
and SHA-256 patch digests pinned per file — plus a spliced 3×FLAC
patch-bearing case (375 frames → 3 patches, digest pinned) and
hand-rolled synthetic WAVs (PCM16/float32 incl. NaN/Inf) for
boundary/downmix/resampler goldens. `flac-long-48k.flac` is excluded
on purpose: it is the repo's 30-minute stress fixture and the repo's
own audio test checks it format-only without full decode.
All pinned digests agree in debug and release profiles.

## Slice 3: patch-level behavioural harness (`src/experiment.rs`)

Research instrumentation for characterizing candidate conventions
before any model exists — still no inference, weights, training, or
production wiring:

- **Configurable layout over frozen H0 DSP**: `PatchConfig`
  (187-frame windows, stride, `BoundaryPolicy`) with H0 as one explicit
  configuration (187/93/Discard). Proven byte-identical to the Slice 1
  path wherever H0 is defined; one canonical DSP implementation remains.
- **Stride ablations**: 46 (heavy overlap), 93 (H0), 94 (off-by-one),
  187 (non-overlapping). Exact start tables pinned (187, 187+S−1,
  187+S, 2·187, 2·187+S); overlap derived as 187−S and proven, not
  eyeballed. No stride is claimed for any external implementation.
- **Boundary policies**: Discard (H0) plus explicitly defined
  ZeroPad/Repeat/Reflect alternatives (ours — not Plex's, not
  Essentia's). Partial-fill rules pinned slot-by-slot on traceable
  frames; full-window content identical across policies.
- **Generic aggregation** over `Vec<f32>` of any width (model-agnostic;
  reusable for future 52/128-D embeddings): mean, standalone
  normalize, mean→normalize, normalize→mean — orders named, computed,
  and proven distinct on asymmetric vectors; empty/dim-mismatch/
  non-finite/zero-norm rejected.
- **Independent formula checks** (test-only references, deliberately
  re-transcribed): exact Slaney linear-branch goldens
  (mel 0/7.5/15 ↔ 0/500/1000 Hz) plus round-trip property; Hann via
  reordered `0.5·(1−cos)` expression; log compression via independent
  `ln(x)/ln(10)` path; DC power chained to the Slice 1 area golden
  (255.5² = 65280.25) with Hann's three-line signature (bin1 ≈ 1/4 DC).
  No published numeric tables exist in the lineage sources, so goldens
  are hand-derived exact consequences of the published formulas — each
  labelled as such. They guard transcription, not the formulas.
- **Deterministic experiments**: `ExperimentConfig` (stable `Display`
  text) + `ExperimentReport` (config, input/audio digests, counts,
  patch starts, patch/aggregate digests) via `run_experiment`
  (ingest → H0 frontend → layout → aggregate). Corpus: Slice 2
  fixtures × strides {46, 93, 187} plus spliced 3×FLAC multi-patch rows
  (5/3/2 patches, digests pinned, debug≡release). Chunk invariance
  re-proven through the configurable layer (4 strides × 4 policies).

## Slice 4A: frozen Research v1 experiment contract

`RESEARCH_V1_CONTRACT` (`src/experiment.rs`) freezes the configuration
the first neural-network experiments run under. Every value is an
experimental default unless marked otherwise — none is inferred as Plex
behaviour:

| Parameter   | Research v1          | Basis |
| ----------- | -------------------- | ----- |
| Input       | 16 kHz mono f32      | Established implementation fact (Slice 2) |
| Window      | 512, symmetric Hann  | H0 (lineage hypothesis) |
| Hop         | 256                  | H0 (lineage hypothesis) |
| FFT         | 512-point power      | H0 (lineage hypothesis) |
| Mel bands   | 96, Slaney, 0–8 kHz  | H0 (lineage hypothesis) |
| Compression | `log10(10000*x + 1)` | Lineage, two-sided (training ≡ inference) |
| Patch       | 187 frames           | H0 (lineage hypothesis) |
| Stride      | 93                   | Experimental default (H0; 46/94/187 stay ablation axes) |
| Boundary    | Discard              | Experimental default (lineage inference default; simplest) |
| Aggregation | Mean                 | Experimental default (matches lineage validation averaging; order ablations stay open) |
| Downmix     | arithmetic mean      | Experimental choice (Slice 2) |
| Resampling  | rubato 0.16.2 FftFixedIn | Experimental choice (Slice 2) |

Still unknown (see `FORENSICS.md` §10): actual Plex tensor shape/layout
and dtype, actual frontend preprocessing, actual stride/aggregation,
actual trained weights, and all Conv2D details beyond the reported
`Conv2D → Flatten → Dense(200) → Dense(52, sigmoid)` chain.

## Slice 4B: tiny 52-D reference network (`src/network.rs`)

Plumbing validation only — **not a model, not trained, not Plex**.
Deterministic pure-Rust reference through which v1 `[187,96]` patches
flow end to end:

```text
[187,96] → Tensor3 [1,187,96] (tested no-transpose conversion)
→ Conv2D 3×3 stride-1 valid, 1→4 ch (RESEARCH placeholder, linear —
   the reported chain states no activation here, so none is invented)
→ Flatten channel-major (pinned by sequential-tensor test)
→ Dense(200) + ReLU (ReLU is S1-reported for this layer)
→ Dense(52, linear) → sigmoid → [52] f32 in [0,1]
```

- f32 throughout, fixed loop order, explicit biases, no normalization.
- Reference weights from a transparent integer formula
  (`reference_weight`, range ±0.1) — reproducible, musically meaningless.
- `Sonic52ReferenceNetwork::forward` (+ `forward_many`); shapes enforced
  by type plus explicit rejection of wrong dims / non-finite input.
- 208-byte `f32le` serialization; bridge into the scaffold's
  `Sonic52Embedding` under caller-supplied identity (no coupling, no
  invented fingerprint).
- Full-shape digest pinned (debug ≡ release ≡ 1.85) only after
  independent verification: exact tiny conv/dense/flatten goldens,
  scatter-vs-gather conv agreement, ReLU placement proof, f64 sigmoid
  reference. Multi-patch → Mean aggregation digest pinned.

## Slice 5: corpus-scale runner (`src/corpus.rs`)

One track at a time through the full v1 pipeline — ingest → H0
frontend → v1 layout → reference network → Mean aggregation — over a
pinned fixture corpus plus synthetic sanity tracks, yielding a
deterministic manifest per run:

- **Corpus**: pinned `(id, repo-relative path)` rows sorted by stable
  id (never filesystem order): FLAC 44.1 kHz mono, FLAC 96 kHz stereo,
  WAV 44.1 kHz stereo, two Musepack SV8 cut fragments — all read-only
  reuse, no music added — plus hand-rolled synthetic WAVs (silence, DC,
  impulse, sine, known-L/R stereo, 6 s multi-patch sine).
- **Per-track record**: format, byte/frame counts, patch/vector counts,
  patch + aggregate digests. Zero-patch tracks record an explicit
  aggregate-`none` ("insufficient audio"), never a zero vector.
  Failures (unsupported extension, undecodable bytes) are explicit
  per-track records; neighbours still run.
- **Determinism**: canonical record strings (fixed field order, no
  paths/timings/host facts) → collection SHA-256. Reference network
  pinned by id (`...-reference-v1`) **and** weight digest over every
  synthetic weight/bias. Duplicate runs byte-identical; all digests
  agree across debug/release/1.85.
- **Stride ablation** (46/93/187, all else constant): patch-count
  statistics per arm; digest differences between arms reflect layout
  coverage only — synthetic weights say nothing about model quality.
- **Timings** (`Instant`, per stage + total) and materialization counts
  (peak patches/vectors, patch/vector bytes) are diagnostic only and
  never digested. No RSS metering (not portable without OS deps —
  explicitly omitted). No ANN, no ranking, no metrics, no EffNet
  comparison anywhere.
- The runner reuses the Slice 3/4 APIs directly (proven by a
  cross-check test); one canonical patch/aggregation implementation.

**Reference-vector digests demonstrate reproducible plumbing, not
embedding quality.**

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
