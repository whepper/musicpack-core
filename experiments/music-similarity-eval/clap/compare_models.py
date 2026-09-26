"""Private cross-model join: CLAP vs EffNet, plus the blind follow-up subset.

This is the only step that reads the private EffNet mapping. Its outputs are:
  - out/clap-vs-effnet.json  (private diagnostics, contains EffNet values)
  - out/clap-human-followup.csv  (BLIND: ids only, no scores/ranks/relations)

Methodological rules enforced here:
  * raw cosines from the two models are never compared numerically; only
    within-model ranks and within-model z-scores are used
  * no accuracy/precision/recall/ground-truth/winner is computed
  * the follow-up rule is a documented deterministic quantile threshold
"""

from __future__ import annotations

import csv
import json
import statistics
import sys
from pathlib import Path

# Deterministic follow-up rule, fixed before looking at any value:
# a case qualifies when its combined within-model standardised rank
# disagreement exceeds this z threshold in either direction, OR when it sits in
# the top decile of absolute disagreement. Both are pure rank/z transforms.
Z_THRESHOLD = 1.0
ABS_DISAGREEMENT_QUANTILE = 0.90


def zscores(values: list[float]) -> list[float]:
    mean = statistics.fmean(values)
    spread = statistics.pstdev(values)
    if spread == 0:
        return [0.0 for _ in values]
    return [(value - mean) / spread for value in values]


def ranks_desc(values: list[float]) -> list[int]:
    """1 = highest value, ties broken by original index for determinism."""
    order = sorted(range(len(values)), key=lambda i: (-values[i], i))
    result = [0] * len(values)
    for position, index in enumerate(order, start=1):
        result[index] = position
    return result


def main() -> int:
    if len(sys.argv) != 5:
        print(
            "usage: compare_models.py CLAP_PAIRS_CSV EFFNET_MAPPING_CSV REVIEW_CSV OUT_DIR",
            file=sys.stderr,
        )
        return 2
    clap_csv, mapping_csv, review_csv, out_dir = (Path(a) for a in sys.argv[1:5])
    out_dir.mkdir(parents=True, exist_ok=True)

    clap = {row["review_id"]: row for row in csv.DictReader(clap_csv.open(newline=""))}
    mapping = {row["review_id"]: row for row in csv.DictReader(mapping_csv.open(newline=""))}
    order = [row["review_id"] for row in csv.DictReader(review_csv.open(newline=""))]

    if set(clap) != set(mapping) or set(clap) != set(order):
        print("review id sets do not match across inputs", file=sys.stderr)
        return 1
    if len(order) != 20:
        print(f"expected 20 review ids, got {len(order)}", file=sys.stderr)
        return 1

    effnet_cos = [float(mapping[rid]["cosine"]) for rid in order]
    clap_cos = [float(clap[rid]["clap_cosine"]) for rid in order]
    effnet_rank = ranks_desc(effnet_cos)
    clap_rank = ranks_desc(clap_cos)
    effnet_z = zscores(effnet_cos)
    clap_z = zscores(clap_cos)

    rows = []
    for index, review_id in enumerate(order):
        eff_rank = int(mapping[review_id]["original_rank"])
        rank_gap = effnet_rank[index] - clap_rank[index]
        rows.append(
            {
                "review_id": review_id,
                "query_id": mapping[review_id]["query_id"],
                "candidate_id": mapping[review_id]["candidate_id"],
                "relation": mapping[review_id]["relation"],
                "effnet_cosine": effnet_cos[index],
                "clap_cosine": clap_cos[index],
                "effnet_rank_within_set": effnet_rank[index],
                "clap_rank_within_set": clap_rank[index],
                "effnet_original_rank": eff_rank,
                "effnet_z_within_set": round(effnet_z[index], 6),
                "clap_z_within_set": round(clap_z[index], 6),
                "rank_gap_effnet_minus_clap": rank_gap,
                "z_gap_effnet_minus_clap": round(effnet_z[index] - clap_z[index], 6),
            }
        )

    # Spearman on within-set ranks (no external dependency).
    n = len(rows)
    d_squared = sum((row["effnet_rank_within_set"] - row["clap_rank_within_set"]) ** 2 for row in rows)
    spearman = 1 - (6 * d_squared) / (n * (n * n - 1))

    def stratum_stats(relation: str) -> dict:
        subset = [row for row in rows if row["relation"] == relation]
        if not subset:
            return {"count": 0}
        return {
            "count": len(subset),
            "effnet_cosine_mean": round(statistics.fmean(row["effnet_cosine"] for row in subset), 6),
            "clap_cosine_mean": round(statistics.fmean(row["clap_cosine"] for row in subset), 6),
            "effnet_rank_mean": round(statistics.fmean(row["effnet_rank_within_set"] for row in subset), 3),
            "clap_rank_mean": round(statistics.fmean(row["clap_rank_within_set"] for row in subset), 3),
        }

    # Deterministic follow-up selection on |z gap|.
    gaps = sorted(abs(row["z_gap_effnet_minus_clap"]) for row in rows)
    quantile_index = max(0, min(len(gaps) - 1, int(ABS_DISAGREEMENT_QUANTILE * (len(gaps) - 1))))
    abs_threshold = gaps[quantile_index]
    followup_ids = [
        row["review_id"]
        for row in rows
        if abs(row["z_gap_effnet_minus_clap"]) >= abs_threshold
        and abs(row["z_gap_effnet_minus_clap"]) > 0
    ]
    followup_ids.sort()

    with (out_dir / "clap-human-followup.csv").open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(
            [
                "review_id",
                "query_id",
                "query_track",
                "candidate_id",
                "candidate_track",
                "rating",
                "note",
            ]
        )
        by_id = {row["review_id"]: row for row in csv.DictReader(review_csv.open(newline=""))}
        for review_id in followup_ids:
            record = by_id[review_id]
            writer.writerow(
                [
                    review_id,
                    record["query_id"],
                    record["query_id"],
                    record["candidate_id"],
                    record["candidate_id"],
                    "",
                    "",
                ]
            )

    diagnostics = {
        "pair_count": n,
        "method": {
            "comparison_basis": "within-model rank and within-model z-score only; "
            "raw cross-model cosine values are never compared numerically",
            "effnet_within_set_rank": "1 = highest EffNet cosine among the 20 pairs",
            "clap_within_set_rank": "1 = highest CLAP cosine among the 20 pairs",
            "z_score": "population z-score within each model's 20 values, independently",
            "spearman": "Spearman rho on the two within-set rank vectors, computed "
            "from the rank-gap sum of squares (no ties present)",
            "not_computed": [
                "accuracy",
                "precision",
                "recall",
                "ground_truth",
                "model_winner",
            ],
        },
        "effnet_cosine_range": [min(effnet_cos), max(effnet_cos)],
        "clap_cosine_range": [min(clap_cos), max(clap_cos)],
        "note_on_ranges": "The two cosine ranges are not on a common scale and must not "
        "be compared directly; only ranks and z-scores are used.",
        "spearman_rho_within_set_ranks": round(spearman, 6),
        "strata": {
            "same_album": stratum_stats("same_album"),
            "same_artist": stratum_stats("same_artist"),
            "different_artist": stratum_stats("different_artist"),
        },
        "agreement": {
            "strong_agreement_top5_both": [
                row["review_id"]
                for row in rows
                if row["effnet_rank_within_set"] <= 5 and row["clap_rank_within_set"] <= 5
            ],
            "strong_disagreement_gap_ge_8": [
                row["review_id"]
                for row in rows
                if abs(row["rank_gap_effnet_minus_clap"]) >= 8
            ],
            "effnet_high_clap_low": [
                row["review_id"]
                for row in rows
                if row["effnet_z_within_set"] > 0 and row["clap_z_within_set"] < 0
            ],
            "effnet_low_clap_high": [
                row["review_id"]
                for row in rows
                if row["effnet_z_within_set"] < 0 and row["clap_z_within_set"] > 0
            ],
        },
        "followup_rule": {
            "statistic": "absolute difference of within-model z-scores",
            "z_threshold": Z_THRESHOLD,
            "abs_disagreement_quantile": ABS_DISAGREEMENT_QUANTILE,
            "effective_abs_threshold": round(abs_threshold, 6),
            "rule": "select cases whose |z gap| is at or above the "
            f"{ABS_DISAGREEMENT_QUANTILE:.0%} quantile of |z gap| across the 20 pairs "
            "and strictly greater than zero",
            "selected_count": len(followup_ids),
            "selected": followup_ids,
        },
        "pairs": rows,
    }
    (out_dir / "clap-vs-effnet.json").write_text(json.dumps(diagnostics, indent=2) + "\n")

    print(f"spearman rho (within-set ranks) = {spearman:.4f}")
    print(f"follow-up |z gap| threshold      = {abs_threshold:.4f}")
    print(f"follow-up cases ({len(followup_ids)}): {', '.join(followup_ids)}")
    for row in sorted(rows, key=lambda r: -abs(r["z_gap_effnet_minus_clap"])):
        print(
            f"  {row['review_id']}  {row['relation']:17}"
            f" effnet_r={row['effnet_rank_within_set']:>2}"
            f" clap_r={row['clap_rank_within_set']:>2}"
            f" gap={row['rank_gap_effnet_minus_clap']:>+3}"
            f" z_gap={row['z_gap_effnet_minus_clap']:>+7.3f}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
