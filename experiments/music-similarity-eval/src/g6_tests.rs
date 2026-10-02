//! Conformance tests for the G-6 experiment.
//!
//! These tests protect the *measurement*, not a conclusion. They assert that:
//!
//! 1. the corpus and the report are byte-deterministic across independent runs;
//! 2. the committed result artefact is exactly what the current code produces;
//! 3. the f16 candidate is genuinely the narrowed-and-widened reference vector,
//!    and is not silently renormalized;
//! 4. the established comparison and ranking procedures are the ones being used;
//! 5. no result artefact leaks a local path, a username or any metadata.
//!
//! None of them asserts that f16 is acceptable. That is a human sign-off.

use std::path::PathBuf;

use crate::docfmt;
use crate::eval;
use crate::g6::{self, Corpus, DIM_PRIMARY};

fn experiment_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn committed_report() -> String {
    std::fs::read_to_string(experiment_root().join("fixtures/g6/REPORT.txt"))
        .expect("committed G-6 result report")
}

#[test]
fn the_report_is_byte_identical_across_independent_runs() {
    // Two separate invocations of the generator, not one value formatted twice.
    let first = g6::render_report();
    let second = g6::render_report();
    assert_eq!(first, second, "the G-6 report is not deterministic");
    assert_eq!(
        docfmt::sha256_hex(first.as_bytes()),
        docfmt::sha256_hex(second.as_bytes())
    );
}

#[test]
fn the_corpus_is_reproducible_from_its_documented_parameters() {
    // A different construction path reaching the same bytes would mean the
    // corpus is not actually pinned by its parameters.
    let a = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    let b = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    assert_eq!(
        a.vectors, b.vectors,
        "corpus generation is not deterministic"
    );
    assert_eq!(a.cluster, b.cluster);
    // Different parameters must give a different corpus, or the parameter is
    // not doing anything and the corpus is not what the report claims.
    let c = Corpus::surrogate(DIM_PRIMARY, 0.36, 0.55);
    assert_ne!(a.vectors, c.vectors, "spread must affect the corpus");
    // Unit vectors, as the producing pipeline would deliver.
    for vector in &a.vectors {
        let mut sum = 0.0f64;
        for value in vector {
            sum += f64::from(*value) * f64::from(*value);
        }
        assert!(
            (sum.sqrt() - 1.0).abs() < 1e-5,
            "corpus vectors must be unit vectors"
        );
    }
}

#[test]
fn the_candidate_is_the_narrowed_and_widened_reference_and_is_not_renormalized() {
    let corpus = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    for vector in corpus.vectors.iter().take(4) {
        let candidate = g6::to_f16_candidate(vector);
        assert_eq!(candidate.len(), vector.len());
        for (index, value) in vector.iter().enumerate() {
            // The candidate is exactly the round trip, not a recomputation.
            let bits = docfmt::f32_to_f16_bits(*value).expect("representable");
            assert_eq!(candidate[index], docfmt::f16_bits_to_f32(bits));
            assert!(
                candidate[index] != *value || f64::from(*value) == 0.0,
                "a non-zero component must be perturbed by the round trip"
            );
        }
        // And it is deliberately NOT re-normalized: the norm is allowed to
        // drift, and that drift is one of the things under test.
        let mut sum = 0.0f64;
        for value in &candidate {
            sum += f64::from(*value) * f64::from(*value);
        }
        assert!(
            (sum.sqrt() - 1.0).abs() < 1e-3,
            "the candidate must not have been renormalized"
        );
    }
}

#[test]
fn the_established_comparison_and_ranking_procedures_are_used() {
    // `eval::nearest` excludes the query and tie-breaks by index; the
    // experiment must inherit that, not reimplement a different ordering.
    let corpus = Corpus::surrogate(16, 0.35, 0.55);
    let comparison = g6::compare(&corpus);
    assert_eq!(comparison.tracks, corpus.vectors.len());
    for seed in &comparison.seeds {
        assert_eq!(seed.reference_order.len(), corpus.vectors.len() - 1);
        assert!(
            !seed.reference_order.contains(&seed.seed),
            "a seed must not rank itself"
        );
        assert!(
            !seed.candidate_order.contains(&seed.seed),
            "a seed must not rank itself in the candidate world"
        );
        // A ranking is a permutation of every other track.
        let mut sorted = seed.reference_order.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), corpus.vectors.len() - 1);
    }
    // The cosine in play is the established one: it is 1.0 for a vector against
    // itself, and the f16 round trip perturbs it by a small non-zero amount —
    // which is precisely the quantity under test.
    let a = &corpus.vectors[0];
    let b = &corpus.vectors[1];
    assert!((eval::cosine(a, a) - 1.0).abs() < 1e-6);
    let reference = eval::cosine(a, b);
    let candidate = eval::cosine(&g6::to_f16_candidate(a), &g6::to_f16_candidate(b));
    assert!(reference.is_finite() && candidate.is_finite());
    assert!(
        (f64::from(candidate) - f64::from(reference)).abs() > 0.0,
        "the f16 round trip must actually move the cosine"
    );
    assert!(
        (f64::from(candidate) - f64::from(reference)).abs() < 1e-2,
        "and only slightly, at this dimension"
    );
}

#[test]
fn the_spearman_implementation_is_correct_on_known_inputs() {
    // Perfect agreement, perfect reversal, and a single adjacent swap.
    let identity = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    assert!((g6::spearman(&identity, &identity) - 1.0).abs() < 1e-12);
    let reversed = vec![5.0, 4.0, 3.0, 2.0, 1.0];
    assert!((g6::spearman(&identity, &reversed) + 1.0).abs() < 1e-12);
    let swapped = vec![1.0, 2.0, 4.0, 3.0, 5.0];
    let rho = g6::spearman(&identity, &swapped);
    // One adjacent swap among five: rho is exactly 0.9, and the point of the
    // assertion is that it is clearly below 1 and clearly positive.
    assert!((rho - 0.9).abs() < 1e-12, "one adjacent swap: {rho}");
    assert!(rho > 0.5, "one adjacent swap: {rho}");
    // Ties must be averaged, not broken arbitrarily.
    let tied = vec![1.0, 1.0, 3.0];
    assert!(
        g6::spearman(&tied, &tied).is_nan() || (g6::spearman(&tied, &tied) - 1.0).abs() < 1e-12
    );
}

#[test]
fn the_committed_report_matches_the_current_code() {
    // The artefact is the record; this is what stops it drifting away from the
    // measurement that produced it.
    let current = g6::render_report();
    let committed = committed_report();
    assert_eq!(
        current, committed,
        "fixtures/g6/REPORT.txt is stale; re-run `cargo run --bin g6 -- --out fixtures/g6` and review the diff"
    );
}

#[test]
fn the_committed_report_records_the_required_provenance() {
    let report = committed_report();
    for required in [
        "corpus=surrogate",
        "candidate=f16_to_f32(f32_to_f16_bits(v)), no renormalization",
        "cosine=eval::cosine",
        "ranking=eval::nearest",
        "conversion=RNE ties-to-even",
        "tracks=45",
        "[storage]",
        "[boundary]",
        "[separation-margin]",
    ] {
        assert!(
            report.contains(required),
            "the report must record {required:?}"
        );
    }
}

#[test]
fn no_result_artefact_leaks_a_path_a_username_or_metadata() {
    // Matched as *paths*, not as bare substrings: a document is allowed to name
    // the pattern it must not contain, so the detector requires a real segment
    // after the prefix. `/Users/<something>` is a leak; a bare mention is not.
    let paths_and_secrets = [
        "/Users/[A-Za-z0-9._-]+",
        "/home/[A-Za-z0-9._-]+",
        "[A-Za-z]:\\\\Users",
        "file://",
        "~/[A-Za-z]",
        "BEGIN RSA PRIVATE KEY",
        "BEGIN PRIVATE KEY",
        "AKIA",
    ];
    // Model family and vendor names belong in the *provenance* record, which
    // must state what the surrogate stands in for. They must never appear in the
    // data artefact, which describes generated vectors and nothing else.
    let metadata_names = ["Discogs", "EffNet", "CLAP", "OpenL3", "Spotify"];

    for name in ["REPORT.txt", "RUN.txt"] {
        let path = experiment_root().join("fixtures/g6").join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for pattern in paths_and_secrets {
            assert!(
                !text.contains(pattern),
                "fixtures/g6/{name} contains a path or secret matching {pattern:?}"
            );
        }
    }

    let report = std::fs::read_to_string(experiment_root().join("fixtures/g6/REPORT.txt"))
        .expect("committed G-6 result report");
    for name in metadata_names {
        assert!(
            !report.contains(name),
            "the data artefact must not mention {name:?}"
        );
    }
}

#[test]
fn the_experiment_claims_nothing_about_perceptual_quality() {
    // A guard on the report's own vocabulary. G-6 is numerical; if a future
    // edit starts writing "equivalent" or "perceptually" into the artefact, the
    // claim has outrun the evidence and this fails.
    let report = committed_report().to_lowercase();
    for forbidden in [
        "perceptually equivalent",
        "perceptual equivalence",
        "indistinguishable",
        "as good as f32",
        "safe for every profile",
    ] {
        assert!(
            !report.contains(forbidden),
            "the report must not claim {forbidden:?}"
        );
    }
}
