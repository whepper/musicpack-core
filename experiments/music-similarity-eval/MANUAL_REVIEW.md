# Manual similarity review

The review worksheet is generated outside Git because it contains local paths
and metadata. Generate one for each patch-hop configuration with:

```sh
cargo run --release --manifest-path experiments/music-similarity-eval/Cargo.toml --bin worksheet -- \
  --multi "$MULTI_OUTPUT" \
  --release "$RELEASE_OUTPUT" \
  --out "$EXTERNAL_OUTPUT/manual-review.csv"
```

The generated file has one row per query/neighbour pair for both model
variants. It includes stable `T###` IDs, source SHA-256 values, durations,
local paths, relation type, and cosine score.

Fill the `rating` column with exactly one of these values:

- `clearly_similar`
- `similar`
- `somewhat_related`
- `not_similar`
- `clearly_wrong`

Use the optional `note` column for a short explanation. Do not infer a rating
from the directory name, model score, or proxy metric.

Pay particular attention to:

- same artist and same album;
- different artists with a similar musical style;
- same broad genre but an obviously different sound;
- acoustic similarity without meaningful musical similarity;
- obvious false positives.

The worksheet generator never fills these fields. Human review is currently
incomplete, so the experiment has no human precision, recall, or musical
relevance result.
