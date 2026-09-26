# Music Similarity Evaluation Spike — Findings and Evidence Record

- **Date:** 2026-09-25
- **Status:** Phase-2 worksheet, patch-hop sensitivity, and cross-codec diagnostics complete; human review and product decision remain open.
- **Scope:** Isolated empirical evaluation only. This document does not authorize a production dependency, `.mpack` change, API, Server, Author, Web, Player, or Sonic change.

## 1. Repository and toolchain findings

The current checkout is `musicpack-core` at `a1dca0b` (`main`). The working tree
already contains the untracked study document `docs/adr/0016-audio-analysis-replacement.md`
from the preceding architecture task; it is preserved and is not modified by this
spike.

The root package and workspace currently contain:

- `musicpack-core` (root package): format, validation, PCM/audio, player, policy;
- `crates/musicpack`: thin CLI;
- `crates/musicpack-engine`: deterministic PCM ring/resampler/mixer;
- `crates/musicpack-host`, `crates/musicpack-wasm`;
- `crates/musicpack-author` and the separately excluded `author/src-tauri`;
- `crates/musicpack-server` (native-only SQLite exception);
- isolated encoder/tools crates;
- `fuzz/` (excluded from the workspace);
- oracle/generator scripts under `tools/`.

At inspection time there was no existing model-evaluation or experimental Cargo package. `tools/`
contains oracle/generator scripts and is not a production crate. The smallest
clean location for this work is a new standalone package at
`experiments/music-similarity-eval/`, excluded from the root workspace by giving
it its own empty Cargo workspace. It may depend on `musicpack-core` for decoding,
but no production crate will depend on it.

The package's declared MSRV is Rust `1.85` (edition 2024). The installed host
toolchain is Rust/Cargo `1.97.1` on `aarch64-apple-darwin`. Installed targets
include:

- `aarch64-apple-darwin` (host);
- `wasm32-unknown-unknown`;
- `x86_64-pc-windows-gnu`;
- `x86_64-pc-windows-msvc`;
- `x86_64-unknown-linux-musl`.

The experiment must still declare compatibility with the repository's Rust
`1.85` policy unless it is explicitly kept outside normal workspace gates and
its toolchain requirement is documented. A runtime whose current MSRV is above
1.85 cannot silently enter the experiment without an explicit decision.

## 2. Existing MusicPack seams

### Decode seam

`src/audio/mod.rs` exposes `audio::open(Box<dyn Read>)` and
`AudioDecoder::read_f32`. The current native decoders are:

- RIFF/WAVE;
- FLAC through the pure-Rust `claxon` adapter;
- Musepack SV8 through the native Rust decoder.

The decoder writes caller-owned interleaved `f32` frames and reports bounded
stream facts. This is the correct input seam for the experiment. The experiment
must not change `musicpack-core::audio`, add a production analysis API, or
introduce FFmpeg/Python audio decoding.

### Resampling seam

`crates/musicpack-engine/src/resampler.rs` provides a deterministic linear
streaming resampler, but its documented purpose is playback compatibility, not
high-quality analysis. Discogs-EffNet requires a specific 16 kHz mono frontend;
silently reusing the playback linear resampler would make the experiment's
preprocessing ambiguous and likely inaccurate. The experiment must either
implement/document a bounded analysis resampler or use a separately reviewed
runtime/helper. It must not modify the playback resampler for this spike.

### Existing analysis

The current product has streaming loudness and waveform accumulators, but no
content-embedding type, similarity query, analysis index, or model runtime. ADR
0012 retires Sonic from the default Author runtime; the current Rust verifier
only hash/structure-checks `analysis[]` references. The historical Sonic
implementation remains oracle/reference material only.

## 3. Discogs-EffNet model identity

The first candidate is the Essentia Discogs-EffNet model, using the `multi`
variant for the initial track-neighbour question. The official Essentia model
metadata was checked at:

- `https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.json`
- `https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.onnx`
- `https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_release_embeddings-effnet-bs64-1.json`

The official metadata reports:

| Field | `multi` variant |
| --- | --- |
| Model family | `EffnetDiscogs` |
| Model version | `1` |
| Release date | `2022-06-15` |
| Description | Music embeddings based on contrastive similarity; artist-level similarity prediction |
| Source framework | TensorFlow `2.8.0` |
| Model types | `frozen_model`, `onnx` |
| Dataset | Discogs-4M (unreleased), 4M full tracks / 3.3M used |
| Input name | `serving_default_melspectrogram` |
| Input type/shape | `float`, `[64, 128, 96]` |
| Embedding output | `PartitionedCall:1`, `[64, 1280]` |
| Other output | `PartitionedCall:0`, `[64, 512]` predictions |
| Inference sample rate | `16000` Hz |
| Official algorithm | `TensorflowPredictEffnetDiscogs` |

The sibling research adapter records the preprocessing and patch contract:

- mono 16 kHz input;
- internal 96-band log-mel frontend;
- frame size 512 and hop 256;
- Slaney mel scale and Essentia log/scaling behavior;
- 128 model input frames per patch (the official graph shape is `[64,128,96]`);
- the sibling adapter's historical profile uses a 61-frame patch hop
  (0.976 seconds), while the current upstream C++ default is 62 frames; the
  distinction is an explicit experiment parameter and sensitivity check;
- the historical report describes 131 mel frames/2.096 seconds as the frontend
  context/window, not as the graph's 128-frame input dimension;
- 1280-dimensional output;
- output rows are not L2-normalized by the model.

The experiment implements the upstream `TensorflowInputMusiCNN` constants
(frame 512, hop 256, 96 Slaney bands, linear weighting, `unit_tri`, shift 1,
scale 10000, `log10`) and records the chosen patch policy. The Rust frontend
has not been numerically cross-checked against an Essentia run, so its output
is an experiment approximation/preprocessing port rather than a claim of
Essentia bit-parity. The fixed model batch behavior is verified by RTen: the
input is `[64,128,96]`, and only the final incomplete batch is zero-padded.

The historical research adapter used TensorFlow `.pb` files rather than the
official `.onnx` files:

- `discogs_multi_embeddings-effnet-bs64-1.pb` — SHA-256
  `2c964064951217e1e345461cf88884086a21f4bca2ae0d48187ee75edc263cd7`;
- `discogs_release_embeddings-effnet-bs64-1.pb` — SHA-256
  `bd044fe53b5d874d52374e023fa7befa1d1afdfd601ff5fe10824cacadaf4dc6`.

Those hashes are historical `.pb` identities, not hashes for the `.onnx` file
that a Rust ONNX runtime would load. The official `multi` ONNX artifact was
fetched only into an external temporary cache for this evaluation (never into
the repository); its verified identity is:

- file: `discogs_multi_embeddings-effnet-bs64-1.onnx`;
- size: `15,998,047` bytes;
- SHA-256: `65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e`.

The corresponding official `release` ONNX artifact was also fetched only into
the external cache for comparison:

- file: `discogs_release_embeddings-effnet-bs64-1.onnx`;
- size: `18,621,961` bytes;
- SHA-256: `fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7`.

The checked-in experiment must still require the caller to provide this exact
path/digest (or another explicitly reviewed artifact) and must never download a
model during normal execution. No model file was found in the current
repository or the sibling research tree.

## 4. Licensing and artifact boundary

The Essentia library is AGPL-3.0-oriented with separate commercial licensing
terms. The Discogs-EffNet weights are documented by the project as
**CC BY-NC-SA 4.0**. These terms do not permit silently treating the model as a
redistributable production dependency or committing its weights to this
repository.

The experiment will therefore:

- accept a model path supplied outside the repository;
- verify a caller-provided expected SHA-256 before loading it;
- never download a model during normal build, test, or CI;
- never commit model weights, model caches, or copyrighted audio;
- keep any downloaded evaluation artifact in an external, ignored cache;
- report code/runtime/model licenses separately;
- fail closed if the artifact or license provenance is not supplied.

The official Essentia model index exposes `.onnx` files, but the experiment will
not assume that their presence grants redistribution rights. The model URL is
an acquisition reference, not an automatic download instruction.

## 5. Rust runtime investigation

### Direct ONNX Runtime

The Rust `ort` crate is a high-level wrapper around ONNX Runtime's C API. It is
not a clean fit for the current repository boundary: `ort-sys` is unsafe FFI,
the normal build obtains/links a native runtime, and the native runtime is not
part of the WASM-clean core. A standalone experiment may evaluate it only if
its native-only status, license, and build/runtime requirements are explicitly
accepted; it must not become a production dependency.

### Pure-Rust alternatives

- `rten` is an end-to-end Rust ONNX runtime with CPU/WASM support. Its current
  0.26.x metadata reports MSRV 1.94, above the repository's 1.85; the isolated
  experiment declares that higher MSRV and has now loaded both official ONNX
  artifacts without Python, TensorFlow, Essentia, or an external service.
- `tract-onnx` is active and has browser support, but the full native dependency
  path includes build/toolchain and upstream `unsafe` concerns. It was not
  needed for this spike.
- `candle` is a broad ML framework with optional native backends and no
  turnkey Discogs audio model path.
- `essentia-rs` wraps C++ Essentia and is not a Rust-native replacement.

At the findings pass no runtime was selected. The implementation then chose
`rten 0.26.0` for the experiment only, with a one-thread run configuration.
This does not change the production runtime decision or the root MSRV.

## 6. Corpus availability

The repository contains only synthetic/reference fixtures: short WAV/FLAC/MPC
files and small test albums. They are useful for decoder smoke tests but are not
a representative music-similarity corpus and will not be used to claim quality.

A local music collection is present at a caller-supplied path. A
filename-suppressing inventory found approximately:

- 5,550 audio files total;
- 2,763 FLAC files;
- 2,785 Musepack files;
- 2 WAV files;
- 5 top-level directories, 16 second-level directories, and 232 third-level
  directories.

The first evidence run selected 15 album groups and three evenly spaced
tracks per group (45 tracks) from this collection. The scanner uses the parent
directory as the album and grandparent as the artist; these are proxy labels,
not verified human annotations. No private filenames, paths, or tags are
committed. The full external output contains the minimum local metadata needed
for manual inspection, while the checked-in report uses anonymized IDs.

## 7. Initial implementation decision (historical)

The repository and model evidence were sufficient to create an isolated
experiment harness, but not sufficient at that point to claim an embedding
result. The implementation boundary was:

1. add a standalone, removable `experiments/music-similarity-eval` package;
2. implement corpus discovery/manifest and metadata-redacted reporting;
3. implement a model-artifact preflight that requires an external path and
   expected SHA-256;
4. implement the Rust PCM decode/preparation path against
   `musicpack-core::audio`;
5. add the model runtime only behind an explicit experiment-only choice;
6. refuse to run embeddings until the model artifact, exact preprocessing
   contract, and corpus selection are recorded.

No `.mpack`, Server, Author, Web, Player, production analysis API, frozen
corpus, encoder, or legacy repository change is part of this milestone.

## 8. Implementation and evidence status

The standalone package is now implemented under
`experiments/music-similarity-eval/`. It uses:

- `musicpack-core::audio::open` / `read_f32` for WAV, FLAC, and Musepack SV8;
- `rubato 0.16.2` fixed-input FFT resampling to 16 kHz;
- `microfft 0.6.0` fixed-size FFTs and an experiment-local Essentia-style
  96-band Slaney mel frontend;
- `rten 0.26.0` as a pure-Rust ONNX runtime, with one worker thread;
- model-specific embedding-output selection by output name and shape;
- per-window L2 pooling, cosine similarity, and equal-mean album aggregation;
- per-track SHA-256 identity over canonical little-endian `f32` vectors;
- an external JSON/CSV/Markdown report with blank human-label columns.

The experiment declares Rust `1.94` because `rten 0.26.0` requires it; the
root workspace remains at Rust `1.85`. It is deliberately outside the root
workspace and has its own lockfile. No root crate, production manifest, `.mpack`
format, Server API, Author workflow, Web/Player code, frozen corpus, encoder
crate, or sibling repository was changed.

The model smoke tests confirmed:

- `multi`: input `[64,128,96]`, embedding output `embeddings`, 1280 dimensions;
- `release`: input `[64,128,96]`, embedding output `embeddings`, 512 dimensions.

The official JSON metadata describes 1280 dimensions for both historical
TensorFlow artifacts, but the supplied official `release` ONNX artifact
exposes a 512-dimensional `embeddings` output when inspected by RTen. This
artifact-specific difference is retained in the reports; vectors from the two
variants are never compared.

The first 15-album, 45-track evidence run uses three evenly spaced tracks per
album. It completed without skipped tracks. The measured proxy metrics,
representative anonymized neighbours, runtime, and licensing conclusion are
in [`REPORT.md`](REPORT.md); full paths and human-review worksheets remain in
the external output directory and are not committed.

The central quality question remains open: the run demonstrates that the
model and Rust pipeline execute and produce stable-shaped embeddings, not that
the neighbours are musically useful for MusicPack. No human labels were
fabricated. The next gate is completion of the human review worksheet; the
phase-2 patch-hop and cross-codec diagnostics are measured below but remain
research evidence rather than a product decision.

## 9. Phase-2 human-review and robustness findings

The experiment now provides three additional utilities, all outside the
production workspace:

- `worksheet` combines the `multi` and `release` top-10 results into a blank-label
  CSV/Markdown review worksheet. It uses stable `T###` IDs and source hashes;
  local paths and names are written only to the external output.
- `compare` compares two same-model, same-corpus runs and reports embedding
  hashes/cosine, top-K overlap, top-1 changes, rank displacement, and score
  deltas.
- `cross_codec` finds artist/album/stem FLAC/Musepack candidates and evaluates
  a bounded sample through the existing decoder seam.
- `review_set` builds a blind 15-query × 4-condition × 5-neighbour set without
  using scores, proxy metrics, relations, or ratings for selection.

### Established

- 900 worksheet rows were generated for each hop setting (45 queries × 10
  neighbours × 2 variants). No rating cells were filled.
- The blind set contains 300 rows: 15 hash-selected album-group queries ×
  5 neighbours × 4 model/hop conditions. It has 15 query artists, 15
  candidate artists, 206 different-artist rows, and 94 same-album rows; these
  are post-selection audit counts, not selection criteria.
- Hop-62 runs completed with zero skipped tracks for both variants.
- Both variants changed every serialized embedding hash between hop 61 and 62,
  while embedding cosine remained at least 0.999106 (`multi`) and 0.999982
  (`release`) per track.
- Top-10 overlap was 0.9844 mean for `multi` and 0.9867 mean for `release`;
  top-1 changed for 9/45 and 10/45 queries respectively. Maximum common-neighbour
  rank displacement was 2.
- The cross-codec candidate finder found 2,735 pairs using the artist/album
  directory proxy plus filename stem; the first five in deterministic path
  order were evaluated per variant/hop. FLAC/Musepack embedding hashes
  differed, but
  mean cosine was 0.999960 (`multi`, hop 61), 0.999958 (`multi`, hop 62),
  0.999998 (`release`, hop 61), and 0.999998 (`release`, hop 62). The
  counterpart was rank 1 in the small ten-track diagnostic set for every
  sampled pair.
- Repeated identical `multi` inference still produced the previously recorded
  identical serialized embedding hash.

### Observed

Human review is **not complete**. No human ratings were collected, so the
experiment still has no human similarity labels, precision/recall, or
musical-relevance conclusion. The `release` score saturation observed in the
first run remains unexplained. The hop change causes a small but non-zero
ordering change; the cross-codec sample is highly stable in embedding space.

### Not established

- Whether either variant produces intuitively useful cross-artist or
  cross-genre neighbours.
- Whether hop 61 or 62 is preferable for a future product contract.
- Whether the five-pair cross-codec result generalizes to a larger collection.
- Whether a different, permissively licensed model would be materially better.

The evidence supports continuing the research phase, not productization. A
human must complete the 300-row blind set before a model winner or
product-contract phase can be proposed.

## 10. Small qualitative sanity set

A separate 100-row sanity set was generated from the pre-existing `multi`/hop-61
run for quick listening feedback. It contains 20 deterministic queries and
five candidates per query. Query selection uses only directory-derived
artist/album grouping and source hashes; candidate selection uses the existing
top-five rule from the pre-declared canonical run. Presentation order is
shuffled so original rank is hidden.

The human-facing sheet exposes only anonymous IDs, local relative paths,
blank `clearly_similar`/`somewhat_related`/`not_similar` ratings, and notes.
The private mapping retains model, hop, rank, cosine, relation, source hashes,
and local metadata. The set has 15 query artists/albums, 15 candidate artists,
and 70 different-artist rows; these are audit counts, not selection criteria.

No ratings were entered, no model was ranked, and no recommendation metric was
calculated. See [`SANITY_REVIEW.md`](SANITY_REVIEW.md).

## 11. Relationship-stratified sanity set

A 20-case stratified set was generated from the same canonical `multi`/hop-61
run, replacing the arbitrary query selection of §10 for integration-sanity
purposes. Eligible pools: `same_album` 80, `same_artist` 0, `different_artist`
370. Selected: 5 / 0 / 15.

Selection takes one case per contiguous cosine bucket rather than the best case
per bucket, so the sample spans the full rank range (ranks 1–10 all occur) and
both plausible successes and likely false positives are exposed. The `group`
column is omitted from the human sheet and rows are interleaved, so the reviewer
cannot read the stratum off the file. Audio is reached through ID-named symlinks
so a library path cannot reveal the artist/album relationship either.

Open uncertainty: the `same_artist` stratum is empty by corpus construction — the
45-track run has one album per artist — so the artist/style-consistency
hypothesis is untested. It needs a corpus with several albums per artist, which is
a separate run rather than a selection change. No ratings exist yet, so nothing
about integration quality is established; only the review instrument is ready.
See [`STRATIFIED_SANITY.md`](STRATIFIED_SANITY.md).

