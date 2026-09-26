# Stratified qualitative sanity check

A small, deliberately stratified listening review. It answers one question:

> Does our MusicPack integration produce plausibly useful similar-track results
> on our actual music collection?

It is **not** a re-validation of Discogs-EffNet as a published model and **not**
a model bake-off. No ranking, precision/recall or model comparison comes out of
it. It supersedes the arbitrary 20-query set in
[`SANITY_REVIEW.md`](SANITY_REVIEW.md) for this purpose; that earlier artifact is
kept so the two approaches stay comparable.

## What a stratum does and does not mean

Strata come from exact directory-derived artist/album metadata that already
exists in the run output. They are **known** relationships used to make the
sample informative, *not* evidence that the embedding is musically good:

| Stratum | Meaning | Role in the review |
| --- | --- | --- |
| `same_album` | same artist, same album | contextual / near-positive control |
| `same_artist` | same artist, different album | artist/style consistency |
| `different_artist` | different artist | the real discovery test |

The human still judges every case musically. A same-album pair that does not
sound like a sensible recommendation is a finding, not a formality.

## Generate

Reads existing run output only — no model load, no inference.

```sh
cargo run --release \
  --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin stratified_set -- \
  --run "$MULTI_HOP61_OUTPUT" \
  --out "$EXTERNAL_OUTPUT/stratified-review"
```

Stratum sizes are adjustable (`--same-album-cases`, `--same-artist-cases`,
`--different-artist-cases`, `--min-cases`, `--max-cases`); the defaults are 5 / 5
/ 12 with a 20-case floor and a 24-case ceiling.

## Selection algorithm

Deterministic, no human judgement, no expectation of musical quality:

1. Read `embeddings.json` and `neighbors.json` from the canonical run.
2. Build the eligible pool from **every** (query, candidate) pair in the run's
   neighbour lists, dropping self-pairs. Classify each pair with exact metadata
   only.
3. Sort each stratum's pool by cosine descending; ties broken by query source
   SHA-256, then candidate source SHA-256.
4. Split the sorted pool into N contiguous, near-equal **score buckets**, where
   N is the number of cases wanted from that stratum.
5. Take exactly **one** case per bucket. Inside a bucket the pick is *not* the
   best-scoring one: it is the pair with the fewest query-track uses so far,
   then fewest query-artist uses, then fewest candidate-track uses, then fewest
   candidate-artist uses, then the lowest SHA-256 of
   `stratified-v1\0<stratum>\0<query sha>\0<candidate sha>`. Use counts only
   spread identity across the collection; they are not quality signals.
6. Because every bucket yields one case, the sample spans the whole available
   cosine **and** rank range — including low-ranked candidates that are
   plausible false positives. Nothing is cherry-picked from the top.
7. If the strata fall below the minimum total, only the `different_artist`
   bucket count is raised by the shortfall, re-partitioning that pool into more,
   still even, buckets. The top-up is recorded per stratum in the manifest.
8. Present all cases in one block ordered by SHA-256 of
   `stratified-order\0<query sha>\0<candidate sha>`, then assign `R####` review
   ids and `T###` track ids. Strata are interleaved and original rank is not
   recoverable from row order.

Cosine and rank are used **only** to spread the sample, never to prefer a case.
No rating takes part in any step.

## Known limitation: `same_artist` is empty in this corpus

The canonical run's 45 tracks cover 15 artist/album groups with **one album per
artist**, so no eligible pair shares an artist across two albums. That stratum
therefore has 0 eligible pairs and contributes 0 cases. Nothing was invented or
substituted, and the shortfall was made up in `different_artist` — the stratum
that matters most. Populating it properly needs a corpus that includes several
albums per artist, which is a separate run, not a selection change.

## Files

- `stratified-sanity-review.csv` — human-facing sheet, exactly
  `review_id,query_id,query_track,candidate_id,candidate_track,rating,note`.
- `stratified-sanity-listening.csv` — `track_id,audio_file` lookup.
- `stratified-sanity-play/` — symlinks named only by track ID (`T001.flac`).
- `stratified-sanity-mapping.csv` — **private**: model, hop, rank, cosine,
  relation, bucket, source hashes, local metadata.
- `stratified-sanity-manifest.json` — **private**: counts, eligible pools,
  algorithm steps, confirmations, source file digests.
- `stratified-sanity-review.md` — generated short instructions.

Keep all generated files outside Git.

The `group` column is deliberately **omitted** from the human sheet: labels like
`context` or `discovery` would tell the reviewer what the stratum is and bias
the rating. Rows are also interleaved so strata are not visible as blocks.

Audio is exposed as ID-named symlinks rather than library paths, because a
library path spells out artist and album — which would leak the relationship the
strata are built from.

## Review

1. Find `query_id` in `stratified-sanity-listening.csv`; it gives a file name
   such as `T001.flac` in `stratified-sanity-play/`. Play it.
2. Find `candidate_id` in the same file and play that file the same way.
3. Ask: would this candidate be a reasonable similar-track recommendation for
   the query?
4. Record one rating: `clearly_similar`, `somewhat_related` or `not_similar`.
5. Optionally add a short note.

Do not open the mapping or manifest until the review is finished. Do not rank
models or compute recommendation metrics from this set.
