# Music Similarity Evaluation Spike

This is an isolated, removable experiment for evaluating the Discogs-EffNet
music embedding model. It is not a MusicPack production crate and is not a
member of the root Cargo workspace.

The repository/model/corpus findings and evidence record are in
[`FINDINGS.md`](FINDINGS.md), the anonymized measured results are in
[`REPORT.md`](REPORT.md), and the review procedure is in
[`MANUAL_REVIEW.md`](MANUAL_REVIEW.md).

The **design-only** production architecture proposal that follows from this
spike is [`PRODUCTION_DESIGN.md`](PRODUCTION_DESIGN.md). It implements nothing
and changes no production code; it exists to be reviewed as the input to a
future ADR (it supersedes parts of `docs/adr/0016-audio-analysis-replacement.md`
and must say so explicitly).

## The similarity document format spike

[`FORMAT_SPEC.md`](FORMAT_SPEC.md) specifies the **MusicPack similarity document
v1**: a package-level binary file that carries, for one profile, one vector per
package track with an explicit per-track status. It is the deliverable of ADR
0017 §14 item 5a, and like the rest of this directory it is **design only**:
nothing in production reads, writes or indexes it.

It is model-independent, profile-driven, and reviewable without a model, a
library, or a server. **The format gate is closed**: four decisions were reviewed
and accepted, ADR 0017 D-3 was amended, and three further rules were made
normative. [`FORMAT_SIGNOFF.md`](FORMAT_SIGNOFF.md) is the decision record — the
accepted decisions, the evidence behind them, and an explicit list of the gates
that remain open. Alongside the specification:

- `fixtures/similarity-doc/` — 26 committed binary fixtures plus `MANIFEST.txt`,
  which records each one's size, SHA-256, parsed fields and expected validation
  result. Every fixture is synthetic: no audio, model, library path, filesystem
  path, artist, album, title or username — and the format has no string field in
  which one could hide.
- `src/docfmt.rs` — a **reference codec** for the specification, so that the
  fixtures are reproducible, determinism is a test, and the validation rules are
  executable. It is not production code, it is not in the root workspace, and a
  production reader/writer is still to be written against `FORMAT_SPEC.md`.
- `src/docfixtures.rs` — the fixture set and the manifest/dump renderers.
- `src/docfmt_tests.rs`, `src/docfmt_tlv_tests.rs` — the conformance suite
  (29 tests), including the canonical profile-TLV encoding rules.
- `src/bin/docfmt.rs` — a review tool: annotated hex dump, fixture regeneration,
  profile fingerprint recomputation.

```sh
# annotated hex dump of one fixture, with its validation result
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin docfmt -- dump experiments/music-similarity-eval/fixtures/similarity-doc/minimal-ok.msim

# the fixture profiles' fingerprints, recomputed from their field lists
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin docfmt -- profiles

# the conformance suite (29 tests)
cargo test --manifest-path experiments/music-similarity-eval/Cargo.toml docfmt
```

`docfmt emit <dir>` regenerates the fixture set from the reference encoder. It is
never run by CI: the committed bytes are the contract, and the test suite requires
the committed bytes and the encoder to agree.

## G-6: f16 versus f32 ranking equivalence

[`G6_F16.md`](G6_F16.md) is a **numerical experiment, not a decision**. It asks
one question: does storing a similarity vector as binary16 instead of the
reference binary32 change rankings beyond a predeclared tolerance?

Its acceptance criteria were written into `FORMAT_SPEC.md` §7.3 *before* the
experiment existed and are used verbatim; C7–C10 were added in `G6_F16.md` §2
before any run. The verdict is **FAIL** against two of the ten, and §9.2 shows
that both failures are threshold-formulation problems rather than evidence
against f16 — the reference f32 world fails C2 identically, and every C6 reversal
crossed a reference margin an order of magnitude below the f16 noise floor.

G-6 remains **OPEN**. Nothing here changes the reference encoding, which stays
`f32le`.

```sh
# the full report, printed
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml --bin g6

# regenerate the committed artefact
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin g6 -- --out experiments/music-similarity-eval/fixtures/g6

# the experiment's tests (determinism, instrument sanity, artefact agreement)
cargo test --manifest-path experiments/music-similarity-eval/Cargo.toml g6
```

The corpus is a deterministic **surrogate** — 45 tracks in 15 clusters at 1280
dimensions, generated from a SHA-256 counter-mode stream — because the recorded
Discogs-EffNet run outputs are not in this repository. That limitation is
recorded in `G6_F16.md` §3 before the run, and §10 states the mechanical step
that would resolve it. No model, audio, CLAP or inference is involved.

### G-6A — review of the acceptance criteria

[`G6A_CRITERIA_REVIEW.md`](G6A_CRITERIA_REVIEW.md) assesses whether C1–C10 were
the right instruments. The G-6 verdict is unchanged — **FAIL, keep f32** — because
the experiment found that **C2 and C6 failed for reasons unconnected to f16**:
C2 is a statement about the corpus that the *unmodified f32 reference* also
fails, and every one of C6's reversals sat inside the quantization noise floor,
below a resolvability bound derived from binary16's significand width.

The proposed revised set (`R-A*`, `R-B*`, `R-C*`) separates numerical fidelity,
retrieval fidelity and representational safety, derives every threshold from
binary16's structure rather than from the observed result, and is **stricter**
than the original wherever the reference can resolve a difference. One property —
whether quantization demotes similar tracks below a relevance threshold — is
**not definable yet**, because no relevance threshold exists; that is recorded as
an open product decision rather than filled with an invented number.

G-6A changes no code, re-runs nothing, and leaves the closed format gate
untouched. The proposed criteria require human approval and a real-corpus run
before any G-6 re-run.

### G-6B — reconciled methodology and corrected instrument

[`G6B_METHODOLOGY.md`](G6B_METHODOLOGY.md) reconciles the G-6A criteria with
the independent review ([`G6A_GLM_REVIEW.md`](G6A_GLM_REVIEW.md)) and defines
the methodology for the final real-corpus re-run. The G-6 verdict is unchanged
— **FAIL, keep f32** — and the historical artefacts are untouched.

Two corrections carry the document. First, the **instrument**: the historical
pairwise metrics quantized only one operand of each pair, while the declared
experiment (and the architecture: a similarity query is itself a stored track
vector, ADR 0017 §5.8) puts **both** operands through the f16 storage path.
`src/g6b.rs` measures the symmetric regime and carries the historical mixed
regime alongside; the correction is 1.52×/1.53×/1.29× at 1280/512/16
dimensions. `src/g6.rs` is deliberately untouched so the committed G-6
artefact keeps matching the code that produced it. Second, the **methodology**:
a numerical error bound is never used to decide that an ordering change does
not matter. Retrieval events are recorded unconditionally — the verified
f32-ULP flip (f32 distinguishes two candidates, f16 reverses them, the
presented top-1 changes, 712× inside the old floor) is committed as a
deterministic reproduction in `g6b::ulp_flip_case`.

Seven frozen gates result (B-A1/A2 numerical, B-B1/B2 retrieval identity at
k ∈ {1,5,10,12,20}, B-C1/2/3 profile safety), each with a threshold that is a
theorem or a definition; six product decisions are recorded rather than
invented. `fixtures/g6b/REPORT.txt` is an **instrument check only** — the
same-data prohibition (§14) forbids evaluating the criteria against the
surrogate that motivated the G-6A repair.

Status: **`G-6B: READY FOR REAL-CORPUS RUN`**. The instrument is complete:
the B-C1/B-C2/B-C3 profile measurements, the album-level k=12 comparison over
per-world recomputed aggregates, the frozen seven-gate evaluator, and the
real-corpus runner (`--bin g6b_real`) with per-track digest verification and
the §17.5 criteria-freeze check (see §18 of the methodology and
`G6B_IMPLEMENTATION.md`). The real-corpus experiment (§13: four corpora —
{multi, release} × {hop 61, 62} — read from recorded `embeddings.json`, pure
post-processing, no inference) has now been run as G-6C (below).

### G-6C — real-corpus evaluation: FAIL, keep `f32le`

The real-corpus run is complete
([`G6C_EVALUATION_RESULTS.md`](G6C_EVALUATION_RESULTS.md)). The two hop-62
`embeddings.json` files were recovered exact; the two hop-61 vector sets were
recovered digest-verified (45/45) from historical `neighbors.json` and
re-containerized per the admissibility review
([`G6C_ADMISSIBILITY_REVIEW.md`](G6C_ADMISSIBILITY_REVIEW.md)); both
reconstructions were independently verified before any evaluator run
(forensics: [`G6C_ARTIFACT_FORENSICS.md`](G6C_ARTIFACT_FORENSICS.md)).

Result under the frozen semantics: **FAIL**. Multi/1280-D passed 7/7 on both
hops; release/512-D failed B-B2 on both hops — f16 storage changes presented
ordered sequences the f32 reference resolves (a top-5 order change at 15
ULPs, a top-20 boundary crossing at 57 ULPs, an album top-12 order change at
11 ULPs). All numerical gates pass everywhere (worst `|Δcos|` 3.27e-5, ~30×
inside the ceiling); top-1 holds 45/45 on all corpora. Two independent runs
were byte-identical (exit 1 both, fail-closed).

Three things, kept distinct: numerical deviations were small; they were still
sufficient to change ordered retrieval results for the release profile; and
because the frozen semantics require exact ordered retrieval identity,
**similarity vectors remain stored as `f32le`**. This is scoped to the tested
release/512-D profile under exact-identity semantics — not a claim that f16
is inaccurate in general, and the multi/1280-D PASS is evidence within a
failed run, not an approval. G-6C is closed; no G-6D is planned. G-7 is
**closed as a technical release gate** (see below); licensing gates G-1…G-4
remain open, as do the ADR 0017 product decisions (D-1…D-6); the similarity
feature is not production-ready.

## G-7: human listening review — closed as a technical release gate

**G-7 is CLOSED (2026-10-01) as a technical release gate.** Human listening
validation is not required for technical acceptance. The full disposition is
recorded in ADR 0017 §10.5:

- Discogs-EffNet's established model evaluation (the creators' own, ISMIR 2022)
  is accepted as evidence about the underlying model.
- MusicPack's own validation (determinism, dimensions, reproducibility,
  retrieval, cross-hop and cross-codec stability, top-1 stability, numerics,
  f32 precision, server index/query, `.mpak` round-trip) covers the
  implementation and integration risks.
- Optional human listening may still be performed later as product-quality
  feedback; it must not block the technical implementation.
- MusicPack does **not** claim to have independently validated the model's
  scientific quality. No perceptual or quality claim is made anywhere in this
  repository.

The listening infrastructure in this directory (`MANUAL_REVIEW.md`,
`BLIND_REVIEW.md`, `SANITY_REVIEW.md`, `STRATIFIED_SANITY.md`, and the
gitignored `g7-set1/`) is **preserved as optional, reusable experiment
infrastructure**. It is not a release gate, and no ratings exist or are
required.

## Scope

The experiment may:

- read a caller-supplied local audio library;
- read a caller-supplied, externally stored model artifact;
- use the existing `musicpack-core::audio` decoder;
- write an external, gitignored evaluation report/cache.

It must not modify `.mpack`, production analysis APIs, Server, Author, Web,
Player, frozen corpora, or the legacy repository. It must not download a model
or audio during a normal build/test/CI invocation.

## Model input

The first profile targets the official Essentia Discogs-EffNet `multi` ONNX
model. The caller must provide both `--model` and `--model-sha256`; the tool
verifies the digest before loading the file. The model weights are documented
as CC BY-NC-SA 4.0 and are never redistributed by this repository.

## Invocation

The model is always supplied by the caller; the command never downloads it.
For the first evidence run, the model was kept in an external temporary cache:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml -- \
  --model "$MODEL_DIR/discogs_multi_embeddings-effnet-bs64-1.onnx" \
  --model-sha256 65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e \
  --model-variant multi \
  --library "$MUSIC_LIBRARY" \
  --albums 15 \
  --tracks-per-album 3 \
  --top-k 10 \
  --patch-hop 61 \
  --out "$EXTERNAL_OUTPUT"
```

The release comparator uses the same corpus and settings with:

```sh
--model "$MODEL_DIR/discogs_release_embeddings-effnet-bs64-1.onnx" \
--model-sha256 fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7 \
--model-variant release
```

The selected output directory contains `embeddings.json`, `neighbors.json`,
`report.md`, and `manual-review.csv`. The JSON/CSV files may contain local
paths and metadata; keep the output outside Git. `manual-review.csv` has blank
label columns rather than fabricated labels.

## Review and robustness utilities

The package keeps the evaluation binary as the default run. The additional
binaries are experiment-only and read existing output directories; they do not
download models or touch production state.

Generate a combined worksheet for both variants (900 rows for 45 queries × 10
neighbours):

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin worksheet -- \
  --multi "$MULTI_OUTPUT" \
  --release "$RELEASE_OUTPUT" \
  --out "$EXTERNAL_OUTPUT/manual-review.csv"
```

The worksheet includes stable `T###` IDs, source SHA-256 values, local paths,
model variant, patch hop, relation (`same_album`, `same_artist`, or
`different_artist`), cosine score, a blank `rating` column accepting
`clearly_similar`/`similar`/`somewhat_related`/`not_similar`/`clearly_wrong`,
and an optional note. A Markdown instruction sheet is written beside it. A
human reviewer must fill the labels; the tools never infer them.

For the blind, metadata-free review set, see [`BLIND_REVIEW.md`](BLIND_REVIEW.md).
For the smaller qualitative sanity check, see [`SANITY_REVIEW.md`](SANITY_REVIEW.md).
For the relationship-stratified sanity check, see [`STRATIFIED_SANITY.md`](STRATIFIED_SANITY.md).

Generate it with:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin review_set -- \
  --multi61 "$MULTI_HOP61_OUTPUT" \
  --release61 "$RELEASE_HOP61_OUTPUT" \
  --multi62 "$MULTI_HOP62_OUTPUT" \
  --release62 "$RELEASE_HOP62_OUTPUT" \
  --queries 15 \
  --neighbors 5 \
  --out "$EXTERNAL_OUTPUT/blind-review"
```

Create the smaller 20-query qualitative sanity set with:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin sanity_set -- \
  --run "$MULTI_HOP61_OUTPUT" \
  --library "$MUSIC_LIBRARY" \
  --queries 20 \
  --neighbors 5 \
  --out "$EXTERNAL_OUTPUT/sanity-review"
```

Create the relationship-stratified sanity set (context / artist-consistency /
discovery) from an existing run:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin stratified_set -- \
  --run "$MULTI_HOP61_OUTPUT" \
  --out "$EXTERNAL_OUTPUT/stratified-review"
```

Compare two runs of the same model and corpus:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin compare -- \
  --a "$HOP61_OUTPUT" \
  --b "$HOP62_OUTPUT" \
  --out "$EXTERNAL_OUTPUT/comparison"
```

Find and evaluate likely equivalent FLAC/Musepack candidates:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin cross_codec -- \
  --model "$MODEL_DIR/discogs_multi_embeddings-effnet-bs64-1.onnx" \
  --model-sha256 65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e \
  --model-variant multi \
  --library "$MUSIC_LIBRARY" \
  --pairs 5 \
  --out "$EXTERNAL_OUTPUT/cross-codec"
```

Use `--list-only` first if the local collection may not contain equivalent
codec candidates. The cross-codec output is a diagnostic; it does not assert
that similarly named files are the same recording.

## Verification

The package is a standalone Cargo workspace with its own lockfile. Relevant
checks are:

```sh
cargo fmt --manifest-path experiments/music-similarity-eval/Cargo.toml -- --check
cargo check --manifest-path experiments/music-similarity-eval/Cargo.toml
cargo test --manifest-path experiments/music-similarity-eval/Cargo.toml
cargo clippy --manifest-path experiments/music-similarity-eval/Cargo.toml --all-targets -- -D warnings
```

The model and local audio are deliberately absent from normal CI. The model
weights are documented as CC BY-NC-SA 4.0; Essentia itself has AGPL/commercial
licensing implications. This experiment does not make either a production
MusicPack dependency.

