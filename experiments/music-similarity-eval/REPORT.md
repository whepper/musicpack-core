# Music Similarity Evaluation Spike — Evidence Report

- **Run date:** 2026-09-25
- **Status:** Execution, worksheet generation, hop sensitivity, and cross-codec diagnostics complete; human labels and product-quality decision remain open.
- **Scope:** Isolated research only. No `.mpack`, production API, Server,
  Author, Web, Player, Sonic, encoder, or frozen-corpus change was made.

This report is intentionally anonymized. The full external run directories
contain local paths, artist/album/track names, vectors, and a blank manual
review worksheet; those files are not committed.

## 1. Model

The first candidate is the official Essentia Discogs-EffNet ONNX model. The
model was supplied from an external temporary cache and verified before every
run. Neither model artifact is in Git.

| Variant | Official source | Version/date | File and SHA-256 | Observed embedding output |
| --- | --- | --- | --- | --- |
| `multi` | [official model JSON](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.json) / [ONNX](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.onnx) | `EffnetDiscogs` v1, 2022-06-15 | `discogs_multi_embeddings-effnet-bs64-1.onnx`, 15,998,047 bytes, `65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e` | `embeddings`, `[64,1280]` |
| `release` | [official model JSON](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_release_embeddings-effnet-bs64-1.json) / [ONNX](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_release_embeddings-effnet-bs64-1.onnx) | `EffnetDiscogs` v1, 2022-06-15 | `discogs_release_embeddings-effnet-bs64-1.onnx`, 18,621,961 bytes, `fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7` | `embeddings`, `[64,512]` |

The official JSON metadata describes a 1280-dimensional embedding output for
both historical TensorFlow artifacts. RTen inspection of the supplied official
`release` ONNX instead found a 512-dimensional output named `embeddings`; the
experiment therefore selects the embedding output by name/shape and keeps the
variants separate. It does not compare vectors across variants.

The official metadata specifies 16 kHz inference and a 96-band log-mel input.
The `multi` model is described as artist-level similarity prediction; `release`
is described as release-level similarity prediction. Those are model-document
claims, not MusicPack quality findings.

## 2. Runtime

- **ONNX runtime:** `rten 0.26.0`, MIT OR Apache-2.0, one worker thread.
- **Resampler:** `rubato 0.16.2`, MIT, fixed-input FFT path.
- **FFT:** `microfft 0.6.0`, MIT, fixed 512-point transform.
- **Decoder:** the existing BSD-3-Clause `musicpack-core::audio` seam.
- **Toolchain:** experiment MSRV 1.94; host used for the run was Rust 1.97.1
  on Apple Silicon. The root workspace MSRV remains 1.85.
- **Not used:** Python, TensorFlow, Essentia, FFmpeg, an external service, or a
  production model/runtime dependency.

The package is a standalone Cargo workspace at
`experiments/music-similarity-eval/` and is not a root workspace member.

## 3. Preprocessing

The run uses the following explicit, experiment-local contract:

1. Decode through `musicpack-core::audio::open` / `read_f32` (WAV, FLAC, and
   Musepack SV8).
2. Downmix channels by an arithmetic mean accumulated in `f64`, then cast to
   `f32`.
3. Resample to 16 kHz with `rubato::FftFixedIn`, 1024-frame chunks, and
   oversampling factor 2; trim startup delay and round the output length.
4. Compute centered 512-sample frames with hop 256, zero-padding boundaries.
5. Apply a symmetric Hann window with `normalized=false` and Essentia's
   zero-phase half rotation.
6. Compute a power spectrum and 96 Slaney mel bands with linear weighting and
   `unit_tri` theoretical-area normalization.
7. Apply `log10(max(1e-30, mel * 10000 + 1))`.
8. Create 128-frame model patches. The run uses a 61-frame patch hop, matching
   the historical sibling adapter; current upstream C++ defaults to 62, so
   this is an explicit sensitivity parameter.
9. Run fixed batches of 64; zero-pad only the final batch and discard padded
   outputs.
10. L2-normalize each window embedding, average, and L2-normalize the track.
    Album vectors are the equal mean of unit track vectors followed by L2.

The Rust mel/resampling implementation follows the documented Essentia constants
but has not been numerically cross-checked against an Essentia run. It is an
experiment port, not a bit-exact compatibility claim.

## 4. Corpus

The local music collection (caller-supplied path, not recorded here) was
inventoried without committing filenames: approximately 5,550 audio files
(2,763 FLAC, 2,785 Musepack, and 2 WAV). The deterministic scanner selected 15
album groups and three evenly spaced tracks per group, for 45 tracks. Both model
runs embedded all 45 tracks with zero skips.

- **Root identity SHA-256:** `0968613caf1cab2f7e222a170cc4edcae788338b68d2bdbbe5ec2d1ab35e94ed`
- **Selected-content identity SHA-256:** `6d213f1952338e5561688a721b867a81140790301886aa039e2b7c1836cdcf45`
- **Selection:** sorted album groups, evenly spaced album selection, evenly
  spaced track limit within each album.

Album and artist names in the proxy metrics come from directory structure. They
are useful diagnostics, not verified human labels.

## 5. Embedding characteristics

| Variant | Dimensions | Runtime | Elapsed | Result |
| --- | ---: | --- | ---: | --- |
| `multi` | 1280 | `rten 0.26.0` | 102.924 s | 45 track vectors, 15 album vectors, 0 skipped |
| `release` | 512 | `rten 0.26.0` | 103.314 s | 45 track vectors, 15 album vectors, 0 skipped |

All generated vectors were finite and unit-normalized. The external
`embeddings.json` records a SHA-256 for each vector over its canonical
little-endian `f32` representation. A separate three-track repeat of the
`multi` run on the same host/toolchain/input produced an identical serialized
embedding hash:

```text
a0b5d8413b0f3b92783333bca701b1f652603d3f14551df4534d2a8d2fc1806b
```

This is evidence of repeatability for that host and run, not a cross-platform
numeric guarantee. For the full 45-track evidence sets, the SHA-256 of the
newline-joined per-track embedding digests (selected order) is:

- `multi`: `7c50fb5679b03c5656d35437a6e19c9ac69c2da4bea96ca1c56e5c02bd7f184d`
- `release`: `12afda79ada6a3ebab3224aa61ba407819d7e8960b5f3d6225f446159d48f8c4`

## 6. Track nearest-neighbour examples

IDs are anonymized positions in the deterministic selected-track order. The
external `neighbors.json` and `manual-review.csv` retain the local metadata for
review. Scores are cosine similarities.

### `multi`

| Query | Top neighbours |
| --- | --- |
| T1 | T3 (0.7467), T2 (0.7105), T33 (0.6433) |
| T2 | T3 (0.8311), T35 (0.8252), T38 (0.8092) |
| T4 | T6 (0.7702), T39 (0.7583), T38 (0.6878) |
| T5 | T23 (0.8685), T38 (0.7968), T35 (0.7671) |

### `release`

| Query | Top neighbours |
| --- | --- |
| T1 | T2 (0.9904), T3 (0.9893), T40 (0.9848) |
| T2 | T3 (0.9933), T35 (0.9916), T33 (0.9910) |
| T4 | T6 (0.9955), T39 (0.9921), T38 (0.9892) |
| T5 | T23 (0.9934), T35 (0.9907), T38 (0.9895) |

The examples show same-group neighbours in some positions and cross-group
neighbours in others. Without human labels, they cannot be called musically
correct or incorrect.

## 7. Album nearest-neighbour examples

Album IDs are anonymized positions in the selected-album order.

| Variant/query | Top neighbours |
| --- | --- |
| `multi` A1 | A11 (0.7772), A12 (0.7331), A5 (0.7286) |
| `multi` A2 | A13 (0.8856), A8 (0.8094), A5 (0.7464) |
| `multi` A5 | A13 (0.8538), A8 (0.8171), A11 (0.7629) |
| `release` A1 | A11 (0.9924), A12 (0.9904), A7 (0.9903) |
| `release` A2 | A13 (0.9952), A8 (0.9921), A6 (0.9888) |

Equal-mean album aggregation is only a first experiment choice. It does not
establish a final `.mpack` album representation.

## 8. Quantitative evaluation

No reliable human labels were supplied, so precision, recall, and rank-based
human metrics are intentionally not calculated. The following are mechanical
directory-group diagnostics over all 45 track queries:

| Variant | Same-album/artist proxy @1 | @5 | @10 | Neighbour score range | Mean neighbour score |
| --- | ---: | ---: | ---: | ---: | ---: |
| `multi` | 0.6889 | 0.3067 | 0.1778 | 0.2827–0.9263 | 0.6917 |
| `release` | 0.8000 | 0.3022 | 0.1800 | 0.9665–0.9961 | 0.9858 |

The selected directory groups have one artist per album, so the same-artist and
same-album columns coincide. The narrow, very high `release` score distribution
is a measured warning against treating its neighbour ordering as automatically
meaningful; it may reflect the artifact's representation or preprocessing
contract and requires human review.

## 9. Manual observations

- The generated lists contain repeated directory-group pairings as well as
  cross-group pairings; this is a structural observation about the output, not
  a human relevance label.
- The external worksheets have a blank `rating` column accepting
  `clearly_similar`, `similar`, `somewhat_related`, `not_similar`, or
  `clearly_wrong`, plus an optional note. All label cells are blank.
- No labels were inferred from genres, directory names, or model metadata.

## 10. Known limitations

- The corpus is 15 albums and 45 tracks, not a representative population-level
  benchmark.
- Directory-derived album/artist groups are proxy labels, not human judgements.
- The Rust frontend has not been compared numerically with Essentia's reference
  implementation.
- The historical 61-frame patch hop may differ from the current upstream 62;
  the measured sensitivity results are in §13.
- FLAC/Musepack decode support was exercised through the local corpus; the
  measured five-pair stability diagnostics are in §13.
- RTen/SIMD/libm behavior may differ across hosts and CPUs.
- The `multi` and `release` vectors have different dimensions and must never be
  compared with one another.
- The model is non-commercial-licensed and unsuitable as an assumed production
  dependency.

## 11. Licensing findings

The project material documents the Discogs-EffNet weights as CC BY-NC-SA 4.0;
Essentia itself has AGPL and separate commercial-licensing implications. The
model files were not committed or downloaded by normal build/test/CI code.
This report is an isolated research record, not legal advice or a grant of
redistribution rights. Confirm licensing with the appropriate licensor before
any broader use.

## 12. Initial recommendation (superseded by §13)

**Measured:** the Rust/ONNX path runs both supplied artifacts without Python,
TensorFlow, Essentia, FFmpeg, or an external service; the selected corpus is
fully embedded; repeated `multi` vectors are identical on this host; and the
`release` artifact has a materially different output dimension and score range.

**Not established:** that either model produces musically useful neighbours for
MusicPack. The proxy metrics are not human relevance labels, and the release
score saturation is a quality warning.

The next step is deliberately still research:

1. Have a reviewer fill the blank manual-review worksheet for a small, explicit
   set of queries.
2. Compare the reviewer judgments with the proxy metrics, without changing
   production code.
3. Use the phase-2 results in §13 to interpret the reviewer judgments.
4. Only if the reviewed results are useful, define a Music Similarity product
   contract and then evaluate package representation separately.

Do not change `.mpack`, Server, Author, Web, Player, or existing workflows based
on this run alone.

## 13. Phase 2: human review and robustness

### Established

- A combined worksheet generator now produces 900 rows for each hop setting:
  45 query tracks × top-10 neighbours × `multi`/`release`. The external
  `manual-review-hop61.csv` and `manual-review-hop62.csv` include stable
  `T###` IDs, source hashes, durations, local paths, relation type, cosine,
  a blank `rating` column, and a blank note column. The checked-in procedure
  is [`MANUAL_REVIEW.md`](MANUAL_REVIEW.md).
- A blind review-set builder produces 15 anonymous queries, five candidates
  per query for each of the four model/hop conditions, and 300 rows. Its
  private mapping and selection audit are documented in
  [`BLIND_REVIEW.md`](BLIND_REVIEW.md).
- The hop-62 runs completed with 45 tracks and zero skips for both variants.
- The comparison tool reports embedding hashes, embedding cosine, top-K
  overlap, top-1 changes, rank displacement, and score deltas.
- The cross-codec finder found 2,735 artist/album/stem candidate pairs. Five
  pairs were evaluated for each variant at each hop; none were skipped.
- The prior deterministic repeat remains valid: identical repeated `multi`
  input produced the identical serialized embedding hash.

### Observed

Human review status: **incomplete**. No human ratings were entered. Therefore
there are no human precision, recall, or musical-relevance results, and no
model winner is declared. The worksheet's relation column and instructions
are prompts for a reviewer, not inferred labels.

The blind set contains 15 query artists, 15 candidate artists, 206
`different_artist` rows, and 94 `same_album` rows. This distribution was
measured after selection and was not used to choose the sample.

#### Patch-hop 61 versus 62

| Variant | Hash equal | Embedding cosine min/mean/max | Top-K overlap mean/min | Top-1 changed | Mean/max rank delta | Mean score delta |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| `multi` | 0/45 | 0.999106 / 0.999867 / 0.999977 | 0.9844 / 0.9000 | 9/45 | 0.123 / 2 | 0.002092 |
| `release` | 0/45 | 0.999982 / 0.999996 / 0.999999 | 0.9867 / 0.9000 | 10/45 | 0.147 / 2 | 0.000105 |

A one-frame patch-hop change changes every serialized embedding hash, but the
resulting vectors and neighbour lists are highly similar. The top-1 result
changes for roughly one fifth of queries; this is a material ordering change
worth reviewing, not evidence of a quality verdict.

#### FLAC versus Musepack

The candidate matcher uses the existing directory-derived artist/album proxy
plus filename stem. Across five evaluated pairs per variant:

| Variant | Hop | Pairs | Hash equal | Cosine min/mean/max | Counterpart rank |
| --- | ---: | ---: | ---: | --- | --- |
| `multi` | 61 | 5 | 0/5 | 0.999914 / 0.999960 / 0.999976 | 1 for all five |
| `multi` | 62 | 5 | 0/5 | 0.999908 / 0.999958 / 0.999975 | 1 for all five |
| `release` | 61 | 5 | 0/5 | 0.999995 / 0.999998 / 0.999999 | 1 for all five |
| `release` | 62 | 5 | 0/5 | 0.999995 / 0.999998 / 0.999999 | 1 for all five |

Durations matched to the displayed precision for all sampled pairs. The
embeddings are not byte-identical, but practical cross-codec stability is high
for this sample. The ten-track pair set is only a diagnostic neighbourhood, not
a full-corpus rank-stability result.

### Not established

- Whether a human listener would call the neighbours clearly similar, similar,
  somewhat related, not similar, or clearly wrong.
- Whether either variant handles cross-artist style, same-genre false positives,
  or acoustic-only similarity better than the other.
- Whether the high cross-codec cosine generalizes beyond five candidate pairs or
  beyond this library's decoding/resampling path.
- Whether the hop-61/62 ordering changes are acceptable for a product contract.
- Any recommendation-quality, precision, recall, or user-value claim.

### Phase-2 recommendation

Do not productize yet, and do not declare a model winner. First complete the
300-row blind review set, recording one of the five rating values and optional
notes for each anonymous candidate. Then compare the human judgments with the
hop sensitivity and cross-codec diagnostics. If the reviewed neighbours are
consistently useful, the next step is a separate Music Similarity
product-contract/design phase; that phase must define its own representation,
indexing, and licensing decisions before any production change.

### Verification

- Experiment formatting, all-target checks, unit tests, and Clippy pass.
- Workspace Clippy, workspace build, workspace tests, wasm32 checks, and the
  fuzz manifest check pass.
- Workspace `cargo fmt --all -- --check` currently reports unrelated
  working-tree formatting differences in
  `crates/musicpack-wasm/src/lib.rs`; those production files were not changed
  or reformatted by this spike.
- `git diff --check` passes.
- No model weights, local audio, or generated external reports are tracked by
  Git. The experiment remains outside the production workspace.

## 14. Small qualitative sanity set

A separate 100-row sanity set was generated for quick human listening rather
than a 300-row scientific review:

- 20 deterministic query tracks;
- 5 candidates per query;
- 100 candidate recommendations total;
- canonical source: the pre-existing `multi`/hop-61 run;
- 15 distinct query artists/albums and 15 candidate artists;
- 70 different-artist rows, recorded only as a post-selection audit.

The human-facing CSV contains only anonymous IDs, local relative track paths,
blank `rating`, and blank `note`. Model identity, hop, numerical score,
original rank, and relation are hidden. The private mapping retains those
fields and source hashes.

Queries were selected from directory-derived artist/album groups using a
pre-declared SHA-256 ordering, with one lowest-source-hash track per group and
source-hash filling for remaining slots. Candidates are the first five from the
canonical run, with presentation order shuffled independently of rank. No
score, relation, model comparison, or rating was used to select a query or
candidate.

No human ratings were entered, no model ranking was performed, and no
recommendation-quality metric was calculated. The procedure is documented in
[`SANITY_REVIEW.md`](SANITY_REVIEW.md).

## 15. Relationship-stratified sanity set

The arbitrary 20-query set of §14 is superseded, for this purpose, by a
deliberately stratified set built from the same canonical `multi`/hop-61 run.
It answers integration sanity only, not model quality.

Eligible pools in the canonical run, before any sampling (45 queries × top-10
neighbours = 450 pairs, no self-pairs):

| Stratum | Eligible pairs | Cosine range | Selected |
| --- | --- | --- | --- |
| `same_album` | 80 | 0.517 – 0.926 | 5 |
| `same_artist` | 0 | — | 0 |
| `different_artist` | 370 | 0.283 – 0.908 | 15 |

Selection sorts each stratum by cosine, splits it into near-equal contiguous
score buckets, and takes exactly one case per bucket using identity-use counts
and a SHA-256 key — never the best-scoring pair in a bucket. The result spans
the full available rank range: ranks 1, 2, 3, 4, 5, 6, 7 and 10 all appear, and
cosine runs from 0.437 to 0.908. Five strata were unreachable by construction
and the shortfall was made up in `different_artist` (12 → 15 buckets), keeping
the total at 20.

The `same_artist` stratum is empty because the run's 45 tracks cover 15
artist/album groups with exactly one album per artist; no eligible pair shares an
artist across two albums. Nothing was invented or substituted. Filling it needs a
corpus with several albums per artist, which is a separate run rather than a
selection change.

Presentation: 20 cases interleaved by a source-hash order, `R####` review ids and
`T###` track ids, no group column (labels would bias the reviewer), and audio
exposed as ID-named symlinks so a library path cannot leak artist/album. The
private mapping retains model, hop, rank, cosine, relation, bucket and hashes.

No ratings were entered, no model was ranked, and no metric was computed.
Procedure: [`STRATIFIED_SANITY.md`](STRATIFIED_SANITY.md).

