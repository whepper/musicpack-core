# Blind human review set

The technical evaluation produced two patch-hop runs for each model variant.
This procedure creates a small blind review set without inspecting scores,
proxy metrics, or musical results.

## Generate

```sh
cargo run --release \
  --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin review_set -- \
  --multi61 "$MULTI_HOP61_OUTPUT" \
  --release61 "$RELEASE_HOP61_OUTPUT" \
  --multi62 "$MULTI_HOP62_OUTPUT" \
  --release62 "$RELEASE_HOP62_OUTPUT" \
  --queries 15 \
  --neighbors 5 \
  --out "$EXTERNAL_OUTPUT/blind-review"
```

The command requires the four existing run directories and does not download
models or read new audio.

## Selection method

Selection is deterministic and deliberately independent of the neighbour
results:

1. Validate that all four runs describe the same 45-track corpus in the same
   order using source SHA-256 values.
2. Group tracks by the existing directory-derived artist/album proxy.
3. Sort album groups by the SHA-256 of a NUL-separated
   `album-group`, artist, and album key, then take the first 15 groups. If
   fewer than 15 groups exist, fill the remaining query slots from unused
   tracks ordered by source SHA-256.
4. Within each selected group, choose the track with the lowest source SHA-256.
5. Assign anonymous `Q01`–`Q15` query IDs by source-hash order.
6. Take the first five neighbours from each of the four conditions:
   `multi`/hop 61, `release`/hop 61, `multi`/hop 62, and `release`/hop 62.
7. Deterministically shuffle rows within each query using a hash key, then
   assign anonymous `R####` review IDs and row-level `C####` candidate IDs.

No neighbour score, rank, relation, proxy metric, or human rating is used for
selection. The current run contains 15 distinct query artists, 15 candidate
artists, 206 different-artist rows, and 94 same-album rows. These are recorded
as an audit of the resulting set, not as selection criteria.

## Files and blindness

Keep all generated files outside Git:

- `review-set.csv`: the primary blind sheet. It contains only
  `review_id,query_id,candidate_id,rating,note`.
- `review-set-mapping.csv`: the private mapping. It adds source hashes,
  artist/album/title/path metadata, model variant, patch hop, rank, cosine, and
  relation.
- `review-set-manifest.json`: counts, conditions, and the selection method.
- `review-set.md`: generated instructions.

The primary sheet does not expose model identity, hop, paths, names, scores,
or same-artist/same-album relations. Candidate IDs are row-level, so the same
source track can appear more than once across conditions without revealing
that duplication in the blind sheet.

## Perform the review

1. Open only `review-set.csv`.
2. For every `review_id`, enter exactly one value in `rating`:
   `clearly_similar`, `similar`, `somewhat_related`, `not_similar`, or
   `clearly_wrong`.
3. Add a short optional note when it explains an important judgment.
4. Do not inspect the mapping until the review is complete.
5. Preserve the completed sheet; no rating is currently filled by the tools.

## Later interpretation

After ratings are supplied, join the sheet to the private mapping by
`review_id`. The first analysis may report:

- completion and missing-row counts;
- rating distributions overall and by model/hop condition;
- the same-artist/same-album/different-artist breakdown as a post-hoc
  comparison, not as a selection criterion;
- recurring notes and false-positive examples.

Do not calculate precision, recall, recommendation quality, or a model winner
until human ratings exist and the analysis rule is agreed. If the review is
incomplete, report it as incomplete and do not extrapolate ratings.
