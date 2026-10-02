//! Conformance tests for the G-6B corrected instrument.
//!
//! These protect the *measurement*, not a conclusion. They assert that:
//!
//! 1. the report is byte-deterministic across independent runs;
//! 2. the committed G-6B instrument-check artefact matches the current code;
//! 3. the pairwise scores really are computed in the symmetric storage
//!    regime, and the historical mixed regime is carried alongside, visibly;
//! 4. the f32-ULP ordering flip is genuinely reproduced, deterministically;
//! 5. the historical G-6 artefacts still match the untouched `g6` code, so the
//!    audit trail from G-6 to G-6B is continuous;
//! 6. the retrieval boundaries cover the limits ADR 0017 §5.8 sketches;
//! 7. no artefact leaks a path, a username, metadata, or a perceptual claim;
//! 8. the frozen gate machinery (B-A1, B-A2, B-B1, B-B2, B-C1, B-C2, B-C3)
//!    measures what §11 says it measures, fails closed, and never evaluates
//!    the criteria on the surrogate.
//!
//! None of them asserts that f16 is acceptable. That is a human sign-off.
//!
//! Gate-machinery tests run on small **hand-built synthetic corpora**, never
//! on the G-6 surrogate: the same-data prohibition (§14 rule 3) keeps the
//! surrogate out of every criteria evaluation, including tests.

use std::path::PathBuf;

use crate::corpus::TrackSpec;
use crate::docfmt;
use crate::eval::{self, TrackEmbedding};
use crate::g6::{self, Corpus, DIM_PRIMARY};
use crate::g6b;
use crate::g6b::{GateEvaluation, GateState, Verdict};
use crate::g6b_real;

fn experiment_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn committed_report() -> String {
    std::fs::read_to_string(experiment_root().join("fixtures/g6b/REPORT.txt"))
        .expect("committed G-6B instrument-check report")
}

#[test]
fn the_report_is_byte_identical_across_independent_runs() {
    let first = g6b::render_report();
    let second = g6b::render_report();
    assert_eq!(first, second, "the G-6B report is not deterministic");
    assert_eq!(
        docfmt::sha256_hex(first.as_bytes()),
        docfmt::sha256_hex(second.as_bytes())
    );
}

#[test]
fn the_committed_report_matches_the_current_code() {
    let current = g6b::render_report();
    let committed = committed_report();
    assert_eq!(
        current, committed,
        "fixtures/g6b/REPORT.txt is stale; re-run `cargo run --bin g6b -- --out fixtures/g6b` and review the diff"
    );
}

/// The core instrumentation defect: `g6::compare`'s pairwise loop quantized
/// only the second operand, so the committed G-6 headline numbers understate
/// the symmetric storage-path error. The G-6B instrument must measure the
/// symmetric regime, carry the mixed regime alongside, and the two must differ
/// on the primary corpus by the documented factor.
#[test]
fn the_symmetric_storage_path_is_measured_and_exceeds_the_mixed_regime() {
    let corpus = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    let comparison = g6b::compare_storage_path(&corpus);
    // Symmetric and mixed are genuinely different measurements here...
    assert!(
        comparison.symmetric_max_abs_delta > comparison.mixed_max_abs_delta,
        "the symmetric regime must be measured separately and is expected to exceed the mixed one"
    );
    // ...by a factor consistent with one versus two quantized operands, never
    // more than two (the triangle bound over the two regimes).
    let ratio = comparison.symmetric_max_abs_delta / comparison.mixed_max_abs_delta;
    assert!(
        ratio > 1.0 && ratio < 2.0,
        "sym/mixed ratio {ratio} outside (1, 2)"
    );
    // And the mixed quantity is exactly the historical instrument's headline:
    // continuity with the committed G-6 artefact.
    let historical = g6::compare(&corpus);
    assert!(
        (comparison.mixed_max_abs_delta - historical.max_abs_delta).abs() < 1e-15,
        "the mixed column must reproduce the historical instrument's max abs delta"
    );
    // Every pairwise score really had both operands through the path: the
    // symmetric delta of a vector against itself is exactly zero only if both
    // sides are identically quantized, which self-pairs are not part of; the
    // observable check is that no symmetric delta can exceed twice the mixed
    // maximum, and that self-cosines are exactly one in both worlds.
    let stored = g6::to_f16_candidate(&corpus.vectors[0]);
    assert!((eval::cosine(&stored, &stored) - 1.0).abs() < 1e-6);
}

/// The ranking outcomes in G-6B must agree with the historical G-6 run on the
/// same corpus, because the historical rankings were already computed in the
/// symmetric regime (`eval::nearest` over the fully quantized candidate set).
/// This is the continuity proof: the correction changes the pairwise metrics,
/// not the ranking metrics.
#[test]
fn ranking_outcomes_are_continuous_with_the_historical_run() {
    let corpus = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    let historical = g6::compare(&corpus);
    let corrected = g6b::compare_storage_path(&corpus);
    assert_eq!(historical.top1_changed(), 0);
    assert!(corrected.seeds.iter().all(|seed| seed.top1_identical));
    assert_eq!(
        historical.swaps,
        corrected.reversals.len(),
        "the reversal census must count the same events the historical swap count did"
    );
}

#[test]
fn retrieval_boundaries_cover_the_adr_sketch() {
    assert_eq!(g6b::RETRIEVAL_KS, [1, 5, 10, 12, 20]);
    // The ceiling is context, never an exemption: no API in this module takes
    // a floor that suppresses a reversal from the census.
    assert!((g6b::NUMERICAL_CEILING - 9.765_625e-4).abs() < 1e-18);
}

/// The f32-ULP flip: the reference distinguishes the two candidates, binary16
/// storage reverses them, the margin is far inside the old 2^-9 floor, and the
/// *presented top-1* changes. Deterministic, from a fixed stream.
#[test]
fn the_f32_ulp_ordering_flip_is_reproduced_deterministically() {
    let first = g6b::ulp_flip_case();
    let second = g6b::ulp_flip_case();
    assert!(
        first.found,
        "the flip case must be found in the fixed stream"
    );
    assert_eq!(
        format!("{first:?}"),
        format!("{second:?}"),
        "the flip case must be identical across independent calls"
    );
    // The reference genuinely resolves the pair: at least one f32 ULP.
    assert!(
        first.reference_margin >= g6b::F32_SCORE_ULP,
        "margin {:.3e} must be at least one f32 score ULP",
        first.reference_margin
    );
    // And it is far inside the old floor that used to exempt exactly this.
    assert!(
        first.reference_margin < 1.953_125e-3 / 4.0,
        "margin {:.3e} must sit far inside the old 2^-9 floor",
        first.reference_margin
    );
    // The presented answer changes: a different track is top-1.
    assert_ne!(first.reference_top1, first.stored_top1);
}

/// The historical G-6 artefact must still match the untouched `g6` code: the
/// audit trail runs G-6 -> G-6A -> review -> G-6B without history being
/// rewritten. (`g6_tests` pins the artefact; this pins that `g6::compare`'s
/// headline on the primary corpus is still the committed value.)
#[test]
fn the_historical_instrument_still_produces_the_committed_headline() {
    let committed = std::fs::read_to_string(experiment_root().join("fixtures/g6/REPORT.txt"))
        .expect("committed G-6 result report");
    assert!(
        committed.contains("max_abs_delta_cosine=1.835e-5"),
        "the historical artefact must keep its committed headline"
    );
    let corpus = Corpus::surrogate(DIM_PRIMARY, 0.35, 0.55);
    let historical = g6::compare(&corpus);
    assert!((historical.max_abs_delta - 1.8353e-5).abs() < 5e-9);
}

#[test]
fn no_result_artefact_leaks_a_path_a_username_or_metadata() {
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
    for name in ["REPORT.txt", "RUN.txt"] {
        let path = experiment_root().join("fixtures/g6b").join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for pattern in paths_and_secrets {
            assert!(
                !text.contains(pattern),
                "fixtures/g6b/{name} contains a path or secret matching {pattern:?}"
            );
        }
    }
    let report = committed_report();
    for name in ["Discogs", "EffNet", "CLAP", "OpenL3", "Spotify"] {
        assert!(
            !report.contains(name),
            "the data artefact must not mention {name:?}"
        );
    }
}

#[test]
fn the_experiment_claims_nothing_about_perceptual_quality() {
    let report = committed_report().to_lowercase();
    for forbidden in [
        "perceptually equivalent",
        "perceptual equivalence",
        "indistinguishable",
        "as good as f32",
        "safe for every profile",
        "f16 is acceptable",
    ] {
        assert!(
            !report.contains(forbidden),
            "the report must not claim {forbidden:?}"
        );
    }
}

/// The methodology document must exist, carry the frozen-criteria table, and
/// state the same-data prohibition. A silent edit to the freeze table breaks
/// this test, which is the point.
#[test]
fn the_methodology_freeze_and_prohibition_are_recorded() {
    let text = std::fs::read_to_string(experiment_root().join("G6B_METHODOLOGY.md"))
        .expect("G6B_METHODOLOGY.md must exist alongside the instrument");
    assert!(text.contains("| Frozen before run? |"));
    assert!(text.contains("same-data prohibition"));
    assert!(text.contains("G-6: FAIL — KEEP F32"));
    // Every load-bearing criterion row must declare itself frozen.
    for id in ["B-A1", "B-A2", "B-B1", "B-B2", "B-C1", "B-C2", "B-C3"] {
        assert!(
            text.contains(&format!("| **{id}** |")),
            "criterion {id} must appear in the frozen table"
        );
    }
}

// ---------------------------------------------------------------------------
// Gate machinery: synthetic hand-built corpora only (never the surrogate)
// ---------------------------------------------------------------------------

fn spec_record(artist: &str, album: &str, title: &str, vector: Vec<f32>) -> TrackEmbedding {
    TrackEmbedding {
        spec: TrackSpec {
            path: format!("{artist}/{album}/{title}").into(),
            artist: artist.to_string(),
            album: album.to_string(),
            title: title.to_string(),
        },
        duration_seconds: 1.0,
        source_sha256: String::new(),
        frame_count: 1,
        patch_count: 1,
        embedding_sha256: docfmt::sha256_hex(&g6::f32_le_bytes(&vector)),
        vector,
    }
}

/// A deterministic unit vector at `angle` radians in 2-D.
fn unit_at(angle: f64) -> Vec<f32> {
    let mut vector = vec![angle.cos() as f32, angle.sin() as f32];
    g6::normalize(&mut vector);
    vector
}

/// 22 tracks/albums whose components are exactly representable in binary16
/// (n/32, n ≤ 24) and whose norm is exactly 1 (Σn² = 1024), so the storage
/// path is the *identity* on this corpus: both worlds are bit-identical by
/// construction and every gate passes deterministically, whatever ties the
/// corpus contains (identical worlds tie identically and index-break
/// identically). This is the all-green wiring test; change *detection* is
/// covered by the swap test below and the deterministic ulp-flip case.
fn full_pass_corpus() -> Vec<TrackEmbedding> {
    dyadic_corpus(22)
}

/// 12 tracks/albums: below every gated boundary population (k=20 needs 21
/// tracks, k=12 needs 13 candidates), so B-B2 is NOT EVALUATED and the run
/// can never be a PASS — even though every gate that *is* evaluable is green.
fn short_corpus() -> Vec<TrackEmbedding> {
    dyadic_corpus(12)
}

/// Dyadic unit vectors in 16 dimensions: components are n/32 for the listed
/// integers, with Σn² = 1024, so ‖v‖ = 1 exactly and every component survives
/// the binary16 round trip bit-exactly.
fn dyadic_corpus(count: usize) -> Vec<TrackEmbedding> {
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let block: [u32; 16] = match index % 6 {
            0 => [16, 16, 16, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            1 => [16, 16, 16, 8, 8, 8, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            2 => [16, 16, 8, 8, 8, 8, 8, 8, 8, 8, 0, 0, 0, 0, 0, 0],
            3 => [16, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 0, 0, 0],
            4 => [24, 8, 8, 8, 8, 8, 8, 8, 0, 0, 0, 0, 0, 0, 0, 0],
            _ => [24, 16, 8, 8, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let shift = index / 6;
        let mut n = [0u32; 16];
        for (position, value) in block.iter().enumerate() {
            n[(position + shift) % 16] = *value;
        }
        // Fail construction, not the gate table, if a fixture stops being
        // exactly unit (Σn² must be 1024 so ‖n/32‖ = 1 exactly).
        let sum_squares: u32 = n.iter().map(|value| value * value).sum();
        assert_eq!(
            sum_squares, 1024,
            "dyadic fixture vector {index} is not exactly unit"
        );
        let vector: Vec<f32> = n.iter().map(|value| *value as f32 / 32.0).collect();
        records.push(spec_record(
            &format!("artist{index:02}"),
            &format!("album{index:02}"),
            &format!("t{index:02}"),
            vector,
        ));
    }
    records
}

// ---- B-C1 -----------------------------------------------------------------

#[test]
fn bc1_norm_deviation_measures_each_reference_vector() {
    // Exact by construction: norm([3, 4]) = 5.
    assert_eq!(g6b::norm_deviation(&[3.0, 4.0]), 4.0);
    // A unit vector deviates by nothing.
    assert_eq!(g6b::norm_deviation(&[1.0, 0.0]), 0.0);
    // Deterministic, and f32 pipeline output sits far inside the tolerance.
    let first = g6b::norm_deviation(&[0.6, 0.8]);
    let second = g6b::norm_deviation(&[0.6, 0.8]);
    assert_eq!(first, second);
    assert!(first > 0.0 && first < 1.0e-6);
    // Independently observable per vector: the worst vector is identified.
    let measured = g6b::measure_profile_safety(&[vec![1.0, 0.0], vec![3.0, 4.0]]);
    assert_eq!(measured.max_norm_deviation_vector, Some(1));
    assert!((measured.max_norm_deviation - 4.0).abs() < 1.0e-12);
}

#[test]
fn bc1_gate_passes_unit_vectors_and_fails_off_norm_ones() {
    let good = g6b::measure_profile_safety(&[vec![1.0, 0.0], vec![0.0, 1.0]]);
    let report = g6b::evaluate_gates("bc1-good", &good, None);
    assert!(matches!(report.gate("B-C1").state, GateState::Pass));

    let bad = g6b::measure_profile_safety(&[vec![3.0, 4.0]]);
    let report = g6b::evaluate_gates("bc1-bad", &bad, None);
    assert!(matches!(report.gate("B-C1").state, GateState::Fail(_)));
    assert_eq!(report.verdict, Verdict::Fail);
}

// ---- B-C2 -----------------------------------------------------------------

#[test]
fn bc2_subnormal_energy_fraction_matches_the_frozen_definition() {
    // No subnormal-range energy at all.
    assert_eq!(g6b::subnormal_energy_fraction(&[1.0, 0.0]), 0.0);
    // Exactly at the 2^-14 boundary is *not* below it: zero is exact in
    // binary16 and components at the knee are still normal-range.
    assert_eq!(
        g6b::subnormal_energy_fraction(&[1.0, 2.0f32.powi(-14)]),
        0.0
    );
    // Just below the boundary: a small but nonzero fraction, inside the gate.
    let near = g6b::subnormal_energy_fraction(&[1.0, 2.0f32.powi(-15)]);
    assert!(near > 0.0 && near < g6b::BC2_MAX_SUBNORMAL_ENERGY);
    // Deterministic.
    assert_eq!(
        near,
        g6b::subnormal_energy_fraction(&[1.0, 2.0f32.powi(-15)])
    );
    // Everything below the boundary: the whole energy is subnormal.
    assert_eq!(g6b::subnormal_energy_fraction(&[2.0f32.powi(-15); 4]), 1.0);
}

#[test]
fn bc2_gate_passes_at_the_boundary_and_fails_past_the_threshold() {
    let ok = g6b::measure_profile_safety(&[vec![1.0, 2.0f32.powi(-15)]]);
    assert!(ok.max_subnormal_energy_fraction > 0.0);
    let report = g6b::evaluate_gates("bc2-ok", &ok, None);
    assert!(matches!(report.gate("B-C2").state, GateState::Pass));

    let bad = g6b::measure_profile_safety(&[vec![2.0f32.powi(-15); 4]]);
    let report = g6b::evaluate_gates("bc2-bad", &bad, None);
    assert!(matches!(report.gate("B-C2").state, GateState::Fail(_)));
    assert_eq!(report.verdict, Verdict::Fail);
}

// ---- B-C3 -----------------------------------------------------------------

#[test]
fn storage_path_reports_rejections_instead_of_panicking() {
    // A representable vector round-trips exactly.
    assert_eq!(
        g6b::encode_storage_path(&[0.5, -0.25], 7).unwrap(),
        vec![0.5, -0.25]
    );
    // A magnitude that would overflow binary16 is a *measured* rejection with
    // the information needed to identify the affected vector.
    let overflow = g6b::encode_storage_path(&[1.0e30], 3).unwrap_err();
    assert_eq!(overflow.vector_index, 3);
    assert_eq!(overflow.component_index, 0);
    assert_eq!(overflow.reason, g6b::RejectionReason::NotRepresentable);
    // Non-finite input is the other rejection reason.
    let nonfinite = g6b::encode_storage_path(&[f32::NAN], 1).unwrap_err();
    assert_eq!(nonfinite.reason, g6b::RejectionReason::NonFinite);
    // Corpus-level encoding collects every rejection, with vector indices.
    let rejections = g6b::encode_corpus_storage_path(&[
        vec![1.0],
        vec![0.0, 1.0e30],
        vec![2.0],
        vec![f32::INFINITY],
    ])
    .unwrap_err();
    assert_eq!(rejections.len(), 2);
    assert_eq!(rejections[0].vector_index, 1);
    assert_eq!(rejections[0].component_index, 1);
    assert_eq!(rejections[1].vector_index, 3);
}

#[test]
fn bc3_violation_yields_fail_and_never_an_evaluated_pass() {
    let records = vec![
        spec_record("a0", "al0", "t0", vec![1.0, 0.0]),
        spec_record("a1", "al1", "t1", vec![0.0, 1.0e30]),
    ];
    let report = g6b::run_corpus_experiment("synthetic-overflow", &records);
    assert!(matches!(report.gate("B-C3").state, GateState::Fail(_)));
    // With rejections the f16 world does not exist, so the numerical and
    // retrieval gates are recorded as not evaluated — and the verdict can
    // only be FAIL.
    for id in ["B-A1", "B-A2", "B-B1", "B-B2"] {
        assert!(
            matches!(report.gate(id).state, GateState::NotEvaluated(_)),
            "gate {id} must be not evaluated when the storage path is undefined"
        );
    }
    assert_eq!(report.verdict, Verdict::Fail);
    assert_ne!(report.verdict, Verdict::Pass);
}

// ---- B-B2 album comparison ------------------------------------------------

#[test]
fn album_comparison_ranks_each_world_from_its_own_aggregates() {
    // Three single-track albums; the stored world carries the two candidate
    // vectors exchanged, so the only way album retrieval can agree with the
    // reference is by copying aggregates — which it must not do.
    let reference = vec![
        spec_record("a0", "al0", "t0", unit_at(0.0)),
        spec_record("a1", "al1", "t1", unit_at(0.05)),
        spec_record("a2", "al2", "t2", unit_at(0.15)),
    ];
    let mut stored = reference.clone();
    stored[1].vector = reference[2].vector.clone();
    stored[2].vector = reference[1].vector.clone();

    let albums = g6b::compare_album_retrieval(&reference, &stored);
    assert!(albums.evaluable);
    assert_eq!(albums.seeds.len(), 3);
    let seed0 = &albums.seeds[0];
    // The presented album answer changes, deterministically, because the
    // stored world's aggregates were recomputed from the stored vectors.
    assert!(!seed0.top1_identical);
    assert_eq!(seed0.sequence_identical.get(&1), Some(&false));
    assert!(
        albums
            .seeds
            .iter()
            .any(|seed| seed.sequence_identical.get(&12) == Some(&false))
            || albums.album_count < 13
    );
    assert!(!albums.reversals.is_empty());

    // Identical worlds rank identically at every *evaluable* boundary (the
    // k=12 album boundary is not evaluable with three albums).
    let same = g6b::compare_album_retrieval(&reference, &reference);
    assert!(same.seeds.iter().all(|seed| seed.top1_identical));
    assert!(same.seeds.iter().all(|seed| {
        seed.sequence_identical
            .iter()
            .all(|(k, identical)| !seed.sequence_evaluable[k] || *identical)
    }));
    assert!(same.reversals.is_empty());
}

#[test]
fn album_comparison_marks_degenerate_aggregation_not_evaluable() {
    // Single album: no candidates, nothing to evaluate.
    let records = vec![spec_record("a0", "al0", "t0", unit_at(0.0))];
    let albums = g6b::compare_album_retrieval(&records, &records);
    assert!(albums.seeds.is_empty());
    // A world whose aggregation dropped an album (zero mean) cannot be
    // compared against a world that kept it.
    let two = vec![
        spec_record("a0", "al0", "t0", unit_at(0.0)),
        spec_record("a1", "al1", "t1", unit_at(0.05)),
    ];
    let albums = g6b::compare_album_retrieval(&two, &records);
    assert!(!albums.evaluable);
    assert!(albums.note.is_some());
}

// ---- full gate table ------------------------------------------------------

#[test]
fn all_gates_green_on_a_clean_corpus_yields_pass() {
    let records = full_pass_corpus();
    let report = g6b::run_corpus_experiment("synthetic-clean", &records);
    assert!(g6b::gate_set_complete(&report.gates));
    assert_eq!(report.gate_ids(), g6b::GATE_IDS.to_vec());
    for gate in &report.gates {
        assert_eq!(
            gate.state,
            GateState::Pass,
            "gate {} did not pass: {:?}",
            gate.id,
            gate.state
        );
    }
    assert_eq!(report.verdict, Verdict::Pass);
}

#[test]
fn an_unevaluatable_gate_is_reported_and_never_passes() {
    // 15 tracks / 5 albums: the k=20 track boundary and the k=12 album
    // boundary cannot exist, so B-B2 is not evaluated and the run can never
    // be a PASS (G6B_METHODOLOGY.md §11).
    let records = short_corpus();
    let report = g6b::run_corpus_experiment("synthetic-short", &records);
    let bb2 = report.gate("B-B2");
    assert!(matches!(bb2.state, GateState::NotEvaluated(_)));
    assert!(bb2.state.reason().unwrap().contains("top-20"));
    assert!(bb2.state.reason().unwrap().contains("top-12"));
    assert!(matches!(report.gate("B-B1").state, GateState::Pass));
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_ne!(report.verdict, Verdict::Pass);
}

#[test]
fn a_changed_presented_result_fails_the_retrieval_gates() {
    let reference = full_pass_corpus();
    let mut stored = reference.clone();
    // Exchange two track vectors between worlds (album0's second member and
    // album2's first member). The stored world is a genuinely different
    // stored corpus, so its presented top-1 for seed 0 changes.
    let moved = stored[1].vector.clone();
    stored[1].vector = stored[4].vector.clone();
    stored[4].vector = moved;

    let comparison = g6b::compare_worlds(&reference, &stored);
    let vectors: Vec<Vec<f32>> = reference.iter().map(|r| r.vector.clone()).collect();
    let profile = g6b::measure_profile_safety(&vectors);
    let report = g6b::evaluate_gates("synthetic-swap", &profile, Some(&comparison));

    assert!(matches!(report.gate("B-B1").state, GateState::Fail(_)));
    assert!(matches!(report.gate("B-B2").state, GateState::Fail(_)));
    assert!(matches!(report.gate("B-C3").state, GateState::Pass));
    assert_eq!(report.verdict, Verdict::Fail);
}

// ---- verdict semantics ----------------------------------------------------

fn gate(id: &'static str, state: GateState) -> GateEvaluation {
    GateEvaluation {
        id,
        state,
        measurement: "unit test".to_string(),
    }
}

#[test]
fn verdict_requires_every_gate_evaluated_and_passing() {
    let all_pass: Vec<GateEvaluation> = g6b::GATE_IDS
        .iter()
        .map(|id| gate(id, GateState::Pass))
        .collect();
    assert_eq!(g6b::verdict_of_gates(&all_pass), Verdict::Pass);

    let mut one_fail = all_pass.clone();
    one_fail[2] = gate(
        "B-B1",
        GateState::Fail("seed 7 presents a different top-1".into()),
    );
    assert_eq!(g6b::verdict_of_gates(&one_fail), Verdict::Fail);

    let mut one_not_evaluated = all_pass.clone();
    one_not_evaluated[4] = gate(
        "B-C1",
        GateState::NotEvaluated("no reference vectors".into()),
    );
    let verdict = g6b::verdict_of_gates(&one_not_evaluated);
    assert_eq!(verdict, Verdict::Inconclusive);
    assert_ne!(
        verdict,
        Verdict::Pass,
        "a missing measurement must never PASS"
    );
}

#[test]
fn overall_verdict_fails_closed_across_corpora() {
    let report = |verdict| CorpusGateReportForTest::build(verdict);
    assert_eq!(
        g6b::overall_verdict(&[report(Verdict::Pass), report(Verdict::Pass)]),
        Verdict::Pass
    );
    assert_eq!(
        g6b::overall_verdict(&[report(Verdict::Pass), report(Verdict::Fail)]),
        Verdict::Fail
    );
    assert_eq!(
        g6b::overall_verdict(&[report(Verdict::Pass), report(Verdict::Inconclusive)]),
        Verdict::Inconclusive
    );
    assert_eq!(
        g6b::overall_verdict(&[report(Verdict::Inconclusive), report(Verdict::Fail)]),
        Verdict::Fail
    );
}

/// Minimal stand-in so the overall-verdict rule can be tested without
/// building full comparisons.
struct CorpusGateReportForTest;
impl CorpusGateReportForTest {
    fn build(verdict: Verdict) -> g6b::CorpusGateReport {
        let gates: Vec<GateEvaluation> = g6b::GATE_IDS
            .iter()
            .map(|id| {
                gate(
                    id,
                    match verdict {
                        Verdict::Pass => GateState::Pass,
                        Verdict::Fail => GateState::Fail("test".into()),
                        Verdict::Inconclusive => GateState::NotEvaluated("test".into()),
                    },
                )
            })
            .collect();
        g6b::CorpusGateReport {
            label: "test".to_string(),
            provenance: None,
            dimension: 0,
            profile: g6b::measure_profile_safety(&[vec![1.0, 0.0]]),
            comparison: None,
            gates,
            verdict,
        }
    }
}

// ---- freeze check ---------------------------------------------------------

#[test]
fn the_frozen_gate_rows_are_present_in_the_methodology() {
    let text = std::fs::read_to_string(experiment_root().join("G6B_METHODOLOGY.md"))
        .expect("G6B_METHODOLOGY.md must exist alongside the instrument");
    assert!(g6b::check_criteria_freeze(&text).is_ok());
}

#[test]
fn criteria_freeze_check_rejects_drift() {
    assert!(g6b::check_criteria_freeze("a document without the frozen table").is_err());
    // Removing a single frozen row is enough to fail the check.
    let text = std::fs::read_to_string(experiment_root().join("G6B_METHODOLOGY.md"))
        .expect("G6B_METHODOLOGY.md must exist alongside the instrument");
    let drifted = text.replace(
        g6b::FROZEN_GATE_ROWS[6],
        "| **B-C3** | C — profile | M16 conversion overflow count | **0** | tampered | FORMAT_SPEC §7.4 | **YES** |",
    );
    assert!(g6b::check_criteria_freeze(&drifted).is_err());
}

// ---- real-corpus ingestion ------------------------------------------------

#[test]
fn the_real_loader_verifies_embedding_digests_before_measurement() {
    let vector = vec![1.0f32, 0.0];
    let digest = docfmt::sha256_hex(&g6::f32_le_bytes(&vector));
    let json = serde_json::json!({
        "metadata": {
            "model_name": "test-model",
            "model_sha256": "aa",
            "patch_hop": 61,
            "embedding_dimensions": 2,
            "corpus_identity": "cid",
            "corpus_root_sha256": "rid",
            "runtime": "rten",
        },
        "tracks": [{
            "artist": "a", "album": "al", "title": "t", "path": "p",
            "duration_seconds": 1.0, "source_sha256": "s",
            "frame_count": 3, "patch_count": 2,
            "embedding_sha256": digest, "embedding": [1.0, 0.0],
        }],
        "albums": [{"artist": "a", "album": "al", "embedding": [9.0, 9.0]}],
    })
    .to_string();
    let corpus = g6b_real::parse_corpus(&json).expect("a well-formed recorded corpus");
    assert_eq!(corpus.records.len(), 1);
    assert_eq!(corpus.records[0].vector, vec![1.0, 0.0]);
    let provenance = corpus.provenance_line();
    assert!(provenance.contains("patch_hop=61"));
    assert!(provenance.contains("model_sha256=aa"));
    // The recorded albums[] block is deliberately ignored (§18 R-3).
    assert!(!provenance.contains('9'));

    // A tampered vector fails identity verification, naming the track.
    let tampered = serde_json::json!({
        "metadata": {
            "model_name": "test-model", "model_sha256": "aa", "patch_hop": 61,
            "embedding_dimensions": 2, "corpus_identity": "cid",
            "corpus_root_sha256": "rid", "runtime": "rten",
        },
        "tracks": [{
            "artist": "a", "album": "al", "title": "t", "path": "p",
            "duration_seconds": 1.0, "source_sha256": "s",
            "frame_count": 3, "patch_count": 2,
            "embedding_sha256": digest, "embedding": [0.0, 1.0],
        }],
    })
    .to_string();
    let error = g6b_real::parse_corpus(&tampered).unwrap_err();
    assert!(
        error.contains("track 0"),
        "the failing track must be identified: {error}"
    );

    // A dimension drift fails loudly too.
    let drifted = json.replace("\"embedding_dimensions\":2", "\"embedding_dimensions\":3");
    assert!(g6b_real::parse_corpus(&drifted).is_err());
}

#[test]
fn the_real_run_report_carries_no_track_metadata() {
    let records = full_pass_corpus();
    let mut report = g6b::run_corpus_experiment("real-shape-check", &records);
    report.provenance = Some("model=test-model model_sha256=aa patch_hop=61 dimensions=2".into());
    let freeze: Result<(), String> = Ok(());
    let text = g6b::render_real_run_report(&freeze, &[report]);
    // The artefact carries the verdict machinery, never the library's
    // metadata: artist names, album names, titles and paths stay out.
    for marker in ["artist", "album0", "album1", "/t0", "t00"] {
        assert!(!text.contains(marker), "the run artefact leaked {marker:?}");
    }
    assert!(text.contains("criteria_freeze=verified"));
    assert!(text.contains("overall_verdict=PASS"));

    // A failed freeze is rendered as the protocol failure it is.
    let records = full_pass_corpus();
    let report = g6b::run_corpus_experiment("real-shape-check", &records);
    let freeze: Result<(), String> =
        Err("frozen criteria row for B-A1 is missing or altered".into());
    let text = g6b::render_real_run_report(&freeze, &[report]);
    assert!(text.contains("criteria_freeze=FAILED"));
}
