//! Deterministic regression tests for the review-set selection and reporting
//! machinery.
//!
//! These tests exist to protect the *evidence-generation* code. A silent
//! regression in selection would change which cases a human reviewer hears
//! without failing anything, so the contracts asserted here are the ones the
//! reports promise: determinism, score/rank spread, selection invariants,
//! blindness of the human-facing file, and an honest stratum partition.
//!
//! Every test drives the real `build_*_set` entry points over a synthetic run
//! directory. No algorithm is reimplemented; the only independently written code
//! is the eligible-pair recount in [`testkit`], which is deliberately a plain
//! loop so it can disagree with the production logic if that logic is wrong.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::review::{RunInput, build_review_set};
use crate::sanity::{SanityRun, build_sanity_set};
use crate::stratified::{
    STRATUM_DIFFERENT_ARTIST, STRATUM_SAME_ALBUM, STRATUM_SAME_ARTIST, StratifiedRun,
    StratifiedTargets, build_stratified_set,
};
use crate::testkit::{self, Fixture, TempDir};

/// Targets that keep the suite fast while exercising every stratum. The
/// defaults are not used here because they would demand a minimum total of 20
/// and the synthetic `same_artist` pool is small by design.
fn small_targets() -> StratifiedTargets {
    StratifiedTargets {
        same_album_cases: 4,
        same_artist_cases: 4,
        different_artist_cases: 8,
        min_total_cases: 4,
        max_total_cases: 24,
    }
}

fn stratified_run(fixture: &Fixture) -> StratifiedRun {
    StratifiedRun {
        run_directory: fixture.directory.clone(),
    }
}

/// Run the real builder into a fresh directory and return the bytes it produced.
fn build_stratified(fixture: &Fixture, out: &Path) -> usize {
    build_stratified_set(&stratified_run(fixture), &small_targets(), out)
        .expect("stratified selection succeeds")
}

fn read_manifest(out: &Path) -> Value {
    serde_json::from_str(&testkit::read(&out.join("stratified-sanity-manifest.json")))
        .expect("manifest parses as json")
}

fn manifest_strata(manifest: &Value) -> Vec<Value> {
    manifest["strata"].as_array().expect("strata array").clone()
}

fn stratum_entry<'a>(manifest: &'a Value, stratum: &str) -> &'a Value {
    manifest["strata"]
        .as_array()
        .expect("strata array")
        .iter()
        .find(|entry| entry["stratum"] == stratum)
        .unwrap_or_else(|| panic!("manifest has no {stratum} entry"))
}

// ---------------------------------------------------------------------------
// 1. Determinism
// ---------------------------------------------------------------------------

#[test]
fn stratified_output_is_byte_identical_across_runs() {
    let source = TempDir::new("det-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let first = TempDir::new("det-a");
    let second = TempDir::new("det-b");

    let first_count = build_stratified(&fixture, &first.path);
    let second_count = build_stratified(&fixture, &second.path);
    assert_eq!(first_count, second_count, "case counts must agree");

    for name in [
        "stratified-sanity-review.csv",
        "stratified-sanity-mapping.csv",
        "stratified-sanity-listening.csv",
        "stratified-sanity-manifest.json",
        "stratified-sanity-review.md",
    ] {
        let a = testkit::read(&first.path.join(name));
        let b = testkit::read(&second.path.join(name));
        assert_eq!(a, b, "{name} is not byte-identical between runs");
    }
}

#[test]
fn sanity_output_is_byte_identical_across_runs() {
    let source = TempDir::new("det-sanity-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let run = || SanityRun {
        directory: fixture.directory.clone(),
        library_root: testkit::LIBRARY_ROOT.into(),
    };
    let first = TempDir::new("det-sanity-a");
    let second = TempDir::new("det-sanity-b");

    let a = build_sanity_set(&run(), 6, 4, &first.path).expect("sanity selection succeeds");
    let b = build_sanity_set(&run(), 6, 4, &second.path).expect("sanity selection succeeds");
    assert_eq!(a, b);

    for name in [
        "sanity-review.csv",
        "sanity-mapping.csv",
        "sanity-manifest.json",
    ] {
        assert_eq!(
            testkit::read(&first.path.join(name)),
            testkit::read(&second.path.join(name)),
            "{name} is not byte-identical between runs"
        );
    }
}

#[test]
fn blind_review_set_is_byte_identical_across_runs() {
    let source = TempDir::new("det-blind-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let runs = vec![RunInput {
        label: "condition-a".into(),
        model_variant: "multi".into(),
        patch_hop: testkit::PATCH_HOP,
        directory: fixture.directory.clone(),
    }];
    let first = TempDir::new("det-blind-a");
    let second = TempDir::new("det-blind-b");

    let a = build_review_set(&runs, 4, 3, &first.path).expect("review set succeeds");
    let b = build_review_set(&runs, 4, 3, &second.path).expect("review set succeeds");
    assert_eq!(a, b);

    for name in [
        "review-set.csv",
        "review-set-mapping.csv",
        "review-set-manifest.json",
    ] {
        assert_eq!(
            testkit::read(&first.path.join(name)),
            testkit::read(&second.path.join(name)),
            "{name} is not byte-identical between runs"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Bucket and rank spread
// ---------------------------------------------------------------------------

#[test]
fn stratified_discovery_spans_the_available_score_range() {
    let source = TempDir::new("spread-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("spread-out");
    build_stratified(&fixture, &out.path);
    let manifest = read_manifest(&out.path);
    let discovery = stratum_entry(&manifest, STRATUM_DIFFERENT_ARTIST);

    let eligible_low = discovery["eligible_score_min"].as_f64().expect("min");
    let eligible_high = discovery["eligible_score_max"].as_f64().expect("max");
    let selected_low = discovery["selected_score_min"].as_f64().expect("min");
    let selected_high = discovery["selected_score_max"].as_f64().expect("max");
    let eligible_span = eligible_high - eligible_low;
    let selected_span = selected_high - selected_low;

    assert!(
        eligible_span > 0.2,
        "fixture must offer a usable score range, got {eligible_span}"
    );
    assert!(
        selected_span / eligible_span > 0.8,
        "selected discovery cases must span the eligible range \
         (eligible {eligible_low}..{eligible_high}, selected {selected_low}..{selected_high})"
    );
}

#[test]
fn stratified_discovery_spans_multiple_ranks() {
    let source = TempDir::new("rank-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("rank-out");
    build_stratified(&fixture, &out.path);

    let (header, rows) = testkit::parse_csv(&out.path.join("stratified-sanity-mapping.csv"));
    let stratum = testkit::column(&header, "stratum");
    let rank = testkit::column(&header, "original_rank");

    let ranks: BTreeSet<usize> = rows
        .iter()
        .filter(|row| row[stratum] == STRATUM_DIFFERENT_ARTIST)
        .map(|row| row[rank].parse().expect("rank parses"))
        .collect();
    assert!(
        ranks.len() >= 4,
        "discovery selection must cover several original ranks, got {ranks:?}"
    );
    let low = *ranks.iter().min().expect("non-empty");
    let high = *ranks.iter().max().expect("non-empty");
    assert!(
        high - low >= 3,
        "discovery selection must span a real rank range, got {low}..{high}"
    );
}

#[test]
fn stratified_takes_at_most_one_case_per_score_bucket() {
    let source = TempDir::new("bucket-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("bucket-out");
    build_stratified(&fixture, &out.path);

    let (header, rows) = testkit::parse_csv(&out.path.join("stratified-sanity-mapping.csv"));
    let stratum = testkit::column(&header, "stratum");
    let bucket = testkit::column(&header, "score_bucket");

    let mut per_stratum: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &rows {
        per_stratum
            .entry(row[stratum].clone())
            .or_default()
            .insert(row[bucket].clone());
    }
    for (name, buckets) in per_stratum {
        assert_eq!(
            buckets.len(),
            rows.iter().filter(|row| row[stratum] == name).count(),
            "stratum {name} selected more than one case from a score bucket: {buckets:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Selection invariants
// ---------------------------------------------------------------------------

#[test]
fn stratified_selection_invariants_hold() {
    let source = TempDir::new("inv-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("inv-out");
    build_stratified(&fixture, &out.path);

    let (header, rows) = testkit::parse_csv(&out.path.join("stratified-sanity-mapping.csv"));
    let stratum = testkit::column(&header, "stratum");
    let pair = testkit::column(&header, "query_source_sha256");
    let candidate_pair = testkit::column(&header, "candidate_source_sha256");
    let rank = testkit::column(&header, "original_rank");
    let index_of = fixture.source_index();

    let mut seen_pairs = BTreeSet::new();
    let mut queries_per_stratum: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for row in &rows {
        let (query_sha, candidate_sha) = (&row[pair], &row[candidate_pair]);

        // No duplicate (query, candidate) pair anywhere in the set.
        assert!(
            seen_pairs.insert((query_sha.clone(), candidate_sha.clone())),
            "duplicate pair selected: {query_sha} -> {candidate_sha}"
        );

        let query_index = *index_of
            .get(query_sha.as_str())
            .expect("query sha is a fixture track");
        let candidate_index = *index_of
            .get(candidate_sha.as_str())
            .expect("candidate sha is a fixture track");

        // Every selected candidate exists in that query's neighbour list, at the
        // rank the mapping recorded.
        let recorded = fixture
            .recorded_rank(query_index, candidate_index)
            .unwrap_or_else(|| {
                panic!("candidate {candidate_index} is not a neighbour of {query_index}")
            });
        assert_eq!(
            recorded,
            row[rank].parse::<usize>().expect("rank parses"),
            "recorded rank disagrees with the fixture for {query_index}->{candidate_index}"
        );

        // No query track is reused within a single stratum.
        assert!(
            queries_per_stratum
                .entry(row[stratum].clone())
                .or_default()
                .insert(query_index),
            "stratum {} reuses query track {query_index}",
            row[stratum]
        );
    }
}

#[test]
fn sanity_cases_are_unique_and_never_self_referential() {
    let source = TempDir::new("sanity-inv-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("sanity-inv-out");
    build_sanity_set(
        &SanityRun {
            directory: fixture.directory.clone(),
            library_root: testkit::LIBRARY_ROOT.into(),
        },
        6,
        4,
        &out.path,
    )
    .expect("sanity selection succeeds");

    let (header, rows) = testkit::parse_csv(&out.path.join("sanity-mapping.csv"));
    let query = testkit::column(&header, "query_id");
    let candidate = testkit::column(&header, "candidate_id");
    let pair = testkit::column(&header, "query_source_sha256");
    let candidate_pair = testkit::column(&header, "candidate_source_sha256");

    let mut seen = BTreeSet::new();
    for row in &rows {
        assert_ne!(row[query], row[candidate], "self pair selected");
        assert!(
            seen.insert((row[pair].clone(), row[candidate_pair].clone())),
            "duplicate pair selected"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Blindness
// ---------------------------------------------------------------------------

/// Assert that generated text carries none of the metadata the human-facing
/// contract forbids. The forbidden literals are derived from the fixture's own
/// metadata, so the check follows the data rather than a hand-typed list.
fn assert_no_metadata_leak(text: &str, label: &str, extra_forbidden: &[&str]) {
    let mut forbidden: Vec<String> = vec![
        testkit::MODEL_NAME.to_string(),
        testkit::MODEL_SHA256.to_string(),
        testkit::PATCH_HOP.to_string(),
        STRATUM_SAME_ALBUM.to_string(),
        STRATUM_SAME_ARTIST.to_string(),
        STRATUM_DIFFERENT_ARTIST.to_string(),
        "same_album".to_string(),
        "same_artist".to_string(),
        "different_artist".to_string(),
        "model_name".to_string(),
        "model_sha256".to_string(),
        "patch_hop".to_string(),
        "cosine".to_string(),
        "score".to_string(),
        "rank".to_string(),
        "relation".to_string(),
        "source_sha256".to_string(),
        "Synthetic Artist".to_string(),
        "Synthetic Album".to_string(),
        testkit::LIBRARY_ROOT.to_string(),
    ];
    forbidden.extend(extra_forbidden.iter().map(|value| (*value).to_string()));
    for needle in forbidden {
        assert!(
            !text.contains(&needle),
            "{label} leaks {needle:?} into the human-facing output"
        );
    }
}

#[test]
fn stratified_review_csv_is_blind() {
    let source = TempDir::new("blind-strat-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("blind-strat-out");
    build_stratified(&fixture, &out.path);

    let review = testkit::read(&out.path.join("stratified-sanity-review.csv"));
    assert_no_metadata_leak(&review, "stratified-sanity-review.csv", &["/"]);
    // The listening list is likewise opaque.
    let listening = testkit::read(&out.path.join("stratified-sanity-listening.csv"));
    assert_no_metadata_leak(&listening, "stratified-sanity-listening.csv", &["/"]);
}

#[test]
fn blind_review_set_csv_is_blind() {
    let source = TempDir::new("blind-set-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("blind-set-out");
    build_review_set(
        &[RunInput {
            label: "condition-a".into(),
            model_variant: "multi".into(),
            patch_hop: testkit::PATCH_HOP,
            directory: fixture.directory.clone(),
        }],
        4,
        3,
        &out.path,
    )
    .expect("review set succeeds");

    let review = testkit::read(&out.path.join("review-set.csv"));
    assert_no_metadata_leak(&review, "review-set.csv", &["/", "multi"]);
}

#[test]
fn sanity_review_csv_carries_no_model_metadata() {
    let source = TempDir::new("blind-sanity-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("blind-sanity-out");
    build_sanity_set(
        &SanityRun {
            directory: fixture.directory.clone(),
            library_root: testkit::LIBRARY_ROOT.into(),
        },
        6,
        4,
        &out.path,
    )
    .expect("sanity selection succeeds");

    // The sanity sheet does carry an opaque object path by design, so only the
    // model/evaluation metadata is forbidden here; the path must still be free
    // of artist and album text, which the fixture paths guarantee.
    let review = testkit::read(&out.path.join("sanity-review.csv"));
    assert_no_metadata_leak(&review, "sanity-review.csv", &[]);
}

// ---------------------------------------------------------------------------
// 5. Stratum partition and eligible-pool honesty
// ---------------------------------------------------------------------------

#[test]
fn stratified_relations_match_the_fixture_metadata() {
    let source = TempDir::new("rel-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("rel-out");
    build_stratified(&fixture, &out.path);

    let (header, rows) = testkit::parse_csv(&out.path.join("stratified-sanity-mapping.csv"));
    let stratum = testkit::column(&header, "stratum");
    let relation = testkit::column(&header, "relation");
    let pair = testkit::column(&header, "query_source_sha256");
    let candidate_pair = testkit::column(&header, "candidate_source_sha256");
    let index_of = fixture.source_index();

    for row in &rows {
        let query = &fixture.tracks[index_of[row[pair].as_str()]];
        let candidate = &fixture.tracks[index_of[row[candidate_pair].as_str()]];
        assert_eq!(
            row[relation],
            testkit::relation_of(query, candidate),
            "recorded relation disagrees with the fixture metadata"
        );
        // The stratum is a relabelling of the relation, not an independent claim.
        let expected_stratum = match row[relation].as_str() {
            "same_album" => STRATUM_SAME_ALBUM,
            "same_artist" => STRATUM_SAME_ARTIST,
            _ => STRATUM_DIFFERENT_ARTIST,
        };
        assert_eq!(row[stratum], expected_stratum);
    }
}

#[test]
fn stratified_manifest_eligible_counts_match_an_independent_recount() {
    let source = TempDir::new("count-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("count-out");
    build_stratified(&fixture, &out.path);
    let manifest = read_manifest(&out.path);
    let expected = fixture.recount_eligible();

    for entry in manifest_strata(&manifest) {
        let stratum = entry["stratum"].as_str().expect("stratum name");
        let reported = entry["eligible_pairs"].as_u64().expect("eligible count");
        let recounted = *expected.get(stratum).unwrap_or(&0) as u64;
        assert_eq!(
            reported, recounted,
            "manifest over-reports eligible pairs for {stratum}"
        );
    }

    // The synthetic corpus is built to populate all three strata, which the real
    // corpus could not do. If this fails, the fixture stopped exercising the
    // same-artist path and the other tests would be silently weaker.
    assert!(
        expected.contains_key(STRATUM_SAME_ARTIST) && expected[STRATUM_SAME_ARTIST] > 0,
        "fixture must populate the same_artist stratum, got {expected:?}"
    );
    assert!(expected[STRATUM_SAME_ALBUM] > 0);
    assert!(expected[STRATUM_DIFFERENT_ARTIST] > 0);
}

#[test]
fn stratified_manifest_case_count_matches_the_review_csv() {
    let source = TempDir::new("casecount-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("casecount-out");
    let reported = build_stratified(&fixture, &out.path);
    let manifest = read_manifest(&out.path);
    assert_eq!(manifest["case_count"].as_u64(), Some(reported as u64));

    let review = testkit::read(&out.path.join("stratified-sanity-review.csv"));
    let data_rows = review.lines().filter(|line| !line.is_empty()).count() - 1;
    assert_eq!(
        data_rows, reported,
        "review csv row count must equal case count"
    );
}

#[test]
fn stratified_play_directory_exposes_only_track_ids() {
    let source = TempDir::new("play-src");
    let fixture = testkit::materialise(&source.path.join("run"));
    let out = TempDir::new("play-out");
    build_stratified(&fixture, &out.path);

    let play = out.path.join("stratified-sanity-play");
    let entries: Vec<_> = std::fs::read_dir(&play)
        .expect("play directory exists")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(!entries.is_empty(), "play directory must not be empty");
    for name in entries {
        let stem = name.split('.').next().unwrap_or_default();
        assert!(
            stem.starts_with('T') && stem[1..].chars().all(|c| c.is_ascii_digit()),
            "play entry {name} is not an opaque track id"
        );
    }
}
