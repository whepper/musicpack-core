# Qualitative music-similarity sanity check

This is a small listening sanity check, not a scientific model comparison.
It is intended only to catch obvious integration problems and answer whether
the neighbours feel musically sensible on this collection.

## Generate

The canonical candidate source is the pre-existing `multi`/hop-61 run. It is
selected by configuration before reviewing candidates, not by comparing model
quality.

```sh
cargo run --release \
  --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin sanity_set -- \
  --run "$MULTI_HOP61_OUTPUT" \
  --library "$MUSIC_LIBRARY" \
  --queries 20 \
  --neighbors 5 \
  --out "$EXTERNAL_OUTPUT/sanity-review"
```

The command reads existing run output only. It does not download a model or
run new inference.

## Selection

- Query tracks are selected deterministically from directory-derived
  artist/album groups using a SHA-256-derived group order.
- One lowest-source-hash track is taken per group, with remaining slots filled
  by source-hash order.
- The first five neighbours from the canonical run are used for each query.
- Candidate presentation order is shuffled with a source-hash key, so original
  rank is not visible.
- No score, rank, relation, model comparison, or rating is used to select
  queries or individual candidates. The set is intentionally not curated for
  obvious or ambiguous musical outcomes; the reviewer is meant to discover
  those qualitatively.

The current set contains 20 queries, 100 candidate recommendations, 15
distinct query artists/albums, 15 candidate artists, and 70 different-artist
rows. Those counts are an audit after selection, not selection criteria.

## Files

Keep generated files outside Git:

- `sanity-review.csv`: the human-facing sheet with exactly
  `review_id,query_id,query_track,candidate_id,candidate_track,rating,note`.
- `sanity-mapping.csv`: private provenance including model, hop, original
  rank, cosine, relation, source hashes, and local metadata.
- `sanity-manifest.json`: counts and selection method.
- `sanity-review.md`: generated short instructions.

The human-facing sheet does not expose model identity, patch hop, score,
original rank, or relation. Track fields contain anonymous IDs plus local
relative paths so the reviewer can play the audio.

## Review

1. Play or listen to the query track.
2. Play or listen to the candidate track.
3. Decide whether the candidate would make sense as a similar-track
   recommendation.
4. Record one rating: `clearly_similar`, `somewhat_related`, or
   `not_similar`.
5. Optionally add a short note.

Do not perform the review using the mapping file. No ratings are filled by the
tool. Do not rank the models or calculate precision/recall from this sanity
check; the purpose is only qualitative integration validation.
