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

