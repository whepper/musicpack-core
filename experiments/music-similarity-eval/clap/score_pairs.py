"""Score the 20 blind pairs and emit clap-pairs.csv / clap-results.json.

Reads only:
  - stratified-sanity-review.csv   (review_id, query_id, candidate_id)
  - clap-embeddings.json          (per-track embeddings)

No EffNet score, rank, relation or library metadata is read here. The private
cross-model join happens in a separate step, outside this file.
"""

from __future__ import annotations

import csv
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

ALLOWED_RATINGS = {"clearly_similar", "somewhat_related", "not_similar"}


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: score_pairs.py REVIEW_CSV EMBEDDINGS_JSON OUT_DIR", file=sys.stderr)
        return 2
    review_csv = Path(sys.argv[1])
    embeddings_json = Path(sys.argv[2])
    out_dir = Path(sys.argv[3])
    out_dir.mkdir(parents=True, exist_ok=True)

    bundle = json.loads(embeddings_json.read_text())
    vectors = {key: np.asarray(value, dtype=np.float64) for key, value in bundle["_embeddings"].items()}

    rows = []
    with review_csv.open(newline="") as handle:
        for record in csv.DictReader(handle):
            query_id = record["query_id"]
            candidate_id = record["candidate_id"]
            if query_id == candidate_id:
                print(f"self pair at {record['review_id']}", file=sys.stderr)
                return 1
            if query_id not in vectors or candidate_id not in vectors:
                print(f"missing embedding for {record['review_id']}", file=sys.stderr)
                return 1
            left, right = vectors[query_id], vectors[candidate_id]
            cosine = float(np.dot(left, right))
            rows.append(
                {
                    "review_id": record["review_id"],
                    "query_id": query_id,
                    "candidate_id": candidate_id,
                    "clap_cosine": round(cosine, 6),
                }
            )

    if len(rows) != 20:
        print(f"expected 20 pairs, got {len(rows)}", file=sys.stderr)
        return 1
    if len({row["review_id"] for row in rows}) != 20:
        print("duplicate review ids", file=sys.stderr)
        return 1

    # Within-set rank: 1 = highest CLAP cosine among the 20 pairs.
    ordered = sorted(rows, key=lambda row: (-row["clap_cosine"], row["review_id"]))
    for position, row in enumerate(ordered, start=1):
        row["clap_rank_within_set"] = position

    rows.sort(key=lambda row: row["review_id"])

    pairs_csv = out_dir / "clap-pairs.csv"
    with pairs_csv.open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["review_id", "clap_cosine", "clap_rank_within_set"])
        for row in rows:
            writer.writerow(
                [row["review_id"], f"{row['clap_cosine']:.6f}", row["clap_rank_within_set"]]
            )

    public_tracks = [
        {key: value for key, value in track.items()} for track in bundle["tracks"]
    ]
    results = {
        "model_id": bundle["model_id"],
        "revision": bundle["revision"],
        "runtime": {
            "torch_version": bundle["torch_version"],
            "device": bundle["device"],
            "transformers": "4.57.1",
            "python": "3.12.13",
        },
        "determinism": {
            "deterministic": bundle["deterministic"],
            "random_seed": bundle["random_seed"],
            "note": "rand_trunc is never reached because windowing is explicit; "
            "consecutive 10 s windows are mean-pooled then L2-normalised",
        },
        "preprocessing": bundle["preprocessing"],
        "embedding_dim": bundle["embedding_dim"],
        "track_count": len(public_tracks),
        "tracks": public_tracks,
        "pair_count": len(rows),
        "pairs": [
            {
                "review_id": row["review_id"],
                "query_id": row["query_id"],
                "candidate_id": row["candidate_id"],
                "clap_cosine": row["clap_cosine"],
                "clap_rank_within_set": row["clap_rank_within_set"],
            }
            for row in rows
        ],
        "metadata_disclosure": {
            "artist_album_title_received_by_model": False,
            "filenames_visible_to_model": False,
            "filesystem_paths_visible_to_model": False,
            "effnet_information_supplied": False,
        },
        "not_computed": ["accuracy", "precision", "recall", "ground_truth", "model_winner"],
    }
    results_json = out_dir / "clap-results.json"
    results_json.write_text(json.dumps(results, indent=2) + "\n")

    csv_digest = hashlib.sha256(pairs_csv.read_bytes()).hexdigest()
    print(f"wrote {pairs_csv}")
    print(f"wrote {results_json}")
    print(f"clap-pairs.csv sha256 = {csv_digest}")
    for row in rows:
        print(
            f"  {row['review_id']}  cosine={row['clap_cosine']:.6f}"
            f"  rank={row['clap_rank_within_set']:>2}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
