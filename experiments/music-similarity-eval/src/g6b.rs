//! G-6B: corrected instrumentation for the f16 storage-path experiment.
//!
//! **Status: EXPERIMENT. Not a decision, and not production code.** The
//! methodology and the frozen acceptance criteria live in
//! [`G6B_METHODOLOGY.md`](../G6B_METHODOLOGY.md). The historical G-6 verdict
//! stands unchanged: **`G-6: FAIL — KEEP F32`**.
//!
//! ## Why this module exists
//!
//! The independent review found that `g6::compare`'s pairwise loop quantized
//! only the *second* operand of each pair (`src/g6.rs:489-491`), while the
//! declared experiment — and the ranking loop inside the very same function —
//! compares two **stored** vectors. In the MusicPack architecture a similarity
//! query is `GET /api/v1/tracks/{id}/similar` (ADR 0017 §5.8): the query is
//! itself a stored track vector, so in an f16 world **both** operands travel
//! through the storage path.
//!
//! The storage path under test is therefore:
//!
//! ```text
//! f32 reference vector -> binary16 serialization -> binary16 decode -> cosine
//! ```
//!
//! for **both** vectors participating in every comparison. This module measures
//! that path. The historical `g6::compare` is deliberately left untouched so
//! that the committed `fixtures/g6/REPORT.txt` keeps matching the code that
//! produced it; the G-6B artefact carries the mixed regime alongside the
//! symmetric one so the correction is visible rather than silent.
//!
//! ## What is deliberately absent
//!
//! No resolvability floor exempts any ordering change here. Retrieval events
//! are recorded unconditionally; numerical bounds are reported as context, not
//! as permission. See `G6B_METHODOLOGY.md` §8.
//!
//! ## The frozen gate machinery
//!
//! The seven gates of `G6B_METHODOLOGY.md` §11 (B-A1, B-A2, B-B1, B-B2, B-C1,
//! B-C2, B-C3) are evaluated only by [`evaluate_gates`] and
//! [`run_corpus_experiment`], and only ever on the **real** corpus: the
//! same-data prohibition (§14 rule 3) forbids evaluating them against the
//! surrogate, so [`render_report`] prints measurements only. Gate thresholds
//! are the frozen §11 constants; they appear nowhere else.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::docfmt;
use crate::eval::{self, TrackEmbedding};
use crate::g6::{self, Corpus};

/// Retrieval boundaries evaluated. `1`, `5` and `10` continue the historical
/// series; `12` and `20` are the result limits ADR 0017 §5.8 sketches for
/// albums and tracks. The final limit set is a product decision (D-3).
pub const RETRIEVAL_KS: [usize; 5] = [1, 5, 10, 12, 20];

/// The derived numerical ceiling `2^-10`, kept **only** as context. It bounds
/// how far a cosine can move under the stated preconditions; it is never used
/// to decide that an ordering change does not matter.
pub const NUMERICAL_CEILING: f64 = 9.765_625e-4;

/// The f32 spacing of similarity scores in `[0.5, 1)`: the finest difference
/// the reference representation itself can express at retrieval-relevant
/// scores. Context for the reversal census, not a threshold.
pub const F32_SCORE_ULP: f64 = 5.960_464_477_539_063e-8;

// ---------------------------------------------------------------------------
// Frozen gate constants (G6B_METHODOLOGY.md §11 — never edited here)
// ---------------------------------------------------------------------------

/// B-A1: `max abs(Δcos)` over the symmetric storage path, all ordered pairs,
/// every corpus. Theorem value `2^-10` given the §9 preconditions.
pub const BA1_LIMIT: f64 = 9.765_625e-4;

/// B-A2: `max abs(‖v'‖ − 1)` over stored vectors. Theorem value `2^-10`.
pub const BA2_LIMIT: f64 = 9.765_625e-4;

/// B-C1: `|‖v‖ − 1|` for every reference vector. Load-bearing §9 requirement.
pub const BC1_TOLERANCE: f64 = 9.765_625e-4;

/// B-C2: per-vector subnormal energy fraction φ (energy in components below
/// 2^-14). Frozen §9/§11 value; see §18 R-2 for the corrected composition
/// argument that supports the B-A1 ceiling.
pub const BC2_MAX_SUBNORMAL_ENERGY: f64 = 5.960_464_477_539_063e-8;

/// B-C3: the specified conversion rejects unrepresentable components; the
/// profile is eligible only with zero rejections.
pub const BC3_MAX_REJECTIONS: usize = 0;

/// The album retrieval boundary gated by B-B2 (ADR 0017 §5.8 sketches
/// `GET /api/v1/albums/{id}/similar?limit=12`).
pub const ALBUM_GATE_K: usize = 12;

/// Components strictly below this magnitude are quantized on the binary16
/// subnormal grid (`2^-14` is binary16's smallest normal).
pub const F16_SUBNORMAL_BOUND: f64 = 6.103_515_625e-5;

/// The frozen gate identifiers, in §11 table order.
pub const GATE_IDS: [&str; 7] = ["B-A1", "B-A2", "B-B1", "B-B2", "B-C1", "B-C2", "B-C3"];

// ---------------------------------------------------------------------------
// Profile-safety measurements (B-C1, B-C2 and the §9 support record)
// ---------------------------------------------------------------------------

/// B-C1 measurement for one reference vector: `|‖v‖ − 1|`.
pub fn norm_deviation(vector: &[f32]) -> f64 {
    let mut sum = 0.0f64;
    for value in vector {
        sum += f64::from(*value) * f64::from(*value);
    }
    (sum.sqrt() - 1.0).abs()
}

/// B-C2 measurement for one reference vector: the fraction φ of the vector's
/// energy carried by components with magnitude below `2^-14` (the binary16
/// subnormal range). Exact zeros carry no energy and are harmless (zero is
/// exact in binary16), which is why the requirement is an energy fraction and
/// not a per-component floor (G6B_METHODOLOGY.md §9).
pub fn subnormal_energy_fraction(vector: &[f32]) -> f64 {
    let mut total = 0.0f64;
    let mut subnormal = 0.0f64;
    for value in vector {
        let square = f64::from(*value) * f64::from(*value);
        total += square;
        if f64::from(*value).abs() < F16_SUBNORMAL_BOUND {
            subnormal += square;
        }
    }
    if total == 0.0 {
        // A zero vector carries no energy anywhere; it fails B-C1 instead.
        return 0.0;
    }
    subnormal / total
}

/// The §9 support record (report only, never gated): the number of nonzero
/// components and the participation ratio `(Σv²)²/Σv⁴` — the effective number
/// of energy-carrying components (`d` for a flat vector, `1` for a single
/// spike). Concentrated vectors move more error than dense ones at the same
/// dimension; the profile review must see the number (G6B_METHODOLOGY.md §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SupportRecord {
    pub nonzero: usize,
    pub participation_ratio: f64,
}

pub fn support_record(vector: &[f32]) -> SupportRecord {
    let mut sum_squares = 0.0f64;
    let mut sum_fourth = 0.0f64;
    let mut nonzero = 0usize;
    for value in vector {
        let square = f64::from(*value) * f64::from(*value);
        sum_squares += square;
        sum_fourth += square * square;
        if *value != 0.0 {
            nonzero += 1;
        }
    }
    let participation_ratio = if sum_fourth == 0.0 {
        0.0
    } else {
        sum_squares * sum_squares / sum_fourth
    };
    SupportRecord {
        nonzero,
        participation_ratio,
    }
}

/// The category-C measurement over one profile's reference vectors.
#[derive(Debug, Clone)]
pub struct ProfileSafetyMeasurement {
    pub vectors: usize,
    /// B-C1 quantity: `max |‖v‖ − 1|` over reference vectors.
    pub max_norm_deviation: f64,
    pub max_norm_deviation_vector: Option<usize>,
    /// B-C2 quantity: `max φ` over reference vectors.
    pub max_subnormal_energy_fraction: f64,
    pub max_subnormal_energy_vector: Option<usize>,
    /// §9 support record, report only.
    pub min_effective_support: f64,
    pub min_nonzero_support: usize,
    /// B-C3 quantity: every component the specified conversion rejects.
    pub rejections: Vec<ComponentRejection>,
    /// Distinct vectors carrying at least one rejection.
    pub affected_vectors: usize,
}

/// Measure B-C1/B-C2 (and the support record) over reference vectors, and
/// B-C3 by attempting the storage-path encode of every vector. Rejections are
/// *recorded*, never panicked (G6B_METHODOLOGY.md §9, §13).
pub fn measure_profile_safety(vectors: &[Vec<f32>]) -> ProfileSafetyMeasurement {
    let mut max_norm_deviation = 0.0f64;
    let mut max_norm_deviation_vector = None;
    let mut max_subnormal_energy_fraction = 0.0f64;
    let mut max_subnormal_energy_vector = None;
    let mut min_effective_support = f64::INFINITY;
    let mut min_nonzero_support = usize::MAX;
    for (index, vector) in vectors.iter().enumerate() {
        let deviation = norm_deviation(vector);
        if deviation > max_norm_deviation {
            max_norm_deviation = deviation;
            max_norm_deviation_vector = Some(index);
        }
        let fraction = subnormal_energy_fraction(vector);
        if fraction > max_subnormal_energy_fraction {
            max_subnormal_energy_fraction = fraction;
            max_subnormal_energy_vector = Some(index);
        }
        let support = support_record(vector);
        if support.participation_ratio < min_effective_support {
            min_effective_support = support.participation_ratio;
        }
        if support.nonzero < min_nonzero_support {
            min_nonzero_support = support.nonzero;
        }
    }
    if vectors.is_empty() {
        min_effective_support = 0.0;
        min_nonzero_support = 0;
    }
    let rejections = match encode_corpus_storage_path(vectors) {
        Ok(_) => Vec::new(),
        Err(rejections) => rejections,
    };
    let affected: BTreeSet<usize> = rejections.iter().map(|r| r.vector_index).collect();
    ProfileSafetyMeasurement {
        vectors: vectors.len(),
        max_norm_deviation,
        max_norm_deviation_vector,
        max_subnormal_energy_fraction,
        max_subnormal_energy_vector,
        min_effective_support,
        min_nonzero_support,
        affected_vectors: affected.len(),
        rejections,
    }
}

// ---------------------------------------------------------------------------
// The storage path, with representability recorded instead of panicked (B-C3)
// ---------------------------------------------------------------------------

/// Why the specified conversion rejected a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    /// NaN or infinite input (`docfmt::f32_to_f16_bits` rejects non-finite).
    NonFinite,
    /// Finite but not representable: the magnitude would overflow binary16.
    NotRepresentable,
}

/// One component the storage path cannot represent. A vector with any
/// rejection has **no** f16 storage representation, so the symmetric
/// comparison is undefined for it — this is measured gate failure material
/// (B-C3), never something to saturate or skip.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentRejection {
    pub vector_index: usize,
    pub component_index: usize,
    pub value: f32,
    pub reason: RejectionReason,
}

/// Apply the storage path (`f32 -> binary16 serialize -> binary16 decode`) to
/// one vector, recording representability rejections instead of panicking.
pub fn encode_storage_path(
    vector: &[f32],
    vector_index: usize,
) -> Result<Vec<f32>, ComponentRejection> {
    let mut out = Vec::with_capacity(vector.len());
    for (component_index, value) in vector.iter().enumerate() {
        let bits = match docfmt::f32_to_f16_bits(*value) {
            Some(bits) => bits,
            None => {
                let reason = if value.is_finite() {
                    RejectionReason::NotRepresentable
                } else {
                    RejectionReason::NonFinite
                };
                return Err(ComponentRejection {
                    vector_index,
                    component_index,
                    value: *value,
                    reason,
                });
            }
        };
        out.push(docfmt::f16_bits_to_f32(bits));
    }
    Ok(out)
}

/// Apply the storage path to a whole corpus. All-or-nothing by design: a
/// vector with any rejected component cannot exist in the f16 world, so a
/// corpus containing one has no defined symmetric comparison. The error
/// carries every rejection with the information needed to identify the
/// affected vector (B-C3 evidence).
pub fn encode_corpus_storage_path(
    vectors: &[Vec<f32>],
) -> Result<Vec<Vec<f32>>, Vec<ComponentRejection>> {
    let mut stored = Vec::with_capacity(vectors.len());
    let mut rejections = Vec::new();
    for (index, vector) in vectors.iter().enumerate() {
        match encode_storage_path(vector, index) {
            Ok(encoded) => stored.push(encoded),
            Err(rejection) => rejections.push(rejection),
        }
    }
    if rejections.is_empty() {
        Ok(stored)
    } else {
        Err(rejections)
    }
}

// ---------------------------------------------------------------------------
// Retrieval measurement (tracks and albums), symmetric storage path
// ---------------------------------------------------------------------------

/// One seed's retrieval outcome, measured directly with no exemption floor.
#[derive(Debug, Clone)]
pub struct SeedRetrieval {
    pub seed: usize,
    /// Whether the presented top-1 neighbour is the same track in both worlds.
    pub top1_identical: bool,
    /// Whether the ordered top-k sequence is identical, per evaluated k.
    pub sequence_identical: BTreeMap<usize, bool>,
    /// Reference score gap across each k-th boundary: `score[k-1] - score[k]`.
    /// `None` when the corpus is too small for the boundary to exist.
    pub boundary_margin: BTreeMap<usize, Option<f64>>,
    /// How many candidates sit within `NUMERICAL_CEILING` of the k-th reference
    /// score. Context only: it describes the neighbourhood, it never exempts.
    pub boundary_population: BTreeMap<usize, usize>,
}

/// One ordering reversal, recorded unconditionally. For album comparisons the
/// fields are indexed by album (`seed` is the querying album).
#[derive(Debug, Clone)]
pub struct ReversalEvent {
    pub seed: usize,
    pub a: usize,
    pub b: usize,
    /// `|cos_f32(q,a) - cos_f32(q,b)|` — the reference margin that flipped.
    pub reference_margin: f64,
    /// The margin expressed in units of the f32 score spacing near 1.0.
    pub margin_in_f32_ulps: f64,
    /// Ranks (0-based) of `a` and `b` in the reference ranking for this seed.
    pub rank_a: usize,
    pub rank_b: usize,
}

/// One album seed's retrieval outcome over the per-world derived aggregates.
#[derive(Debug, Clone)]
pub struct AlbumSeedRetrieval {
    pub album_index: usize,
    pub top1_identical: bool,
    /// Ordered-sequence identity per evaluated k. B-B2's album gate uses
    /// [`ALBUM_GATE_K`]; the other k values are context.
    pub sequence_identical: BTreeMap<usize, bool>,
    /// `false` when fewer than `k` candidate albums exist in a world.
    pub sequence_evaluable: BTreeMap<usize, bool>,
    pub boundary_margin: BTreeMap<usize, Option<f64>>,
}

/// The album-level comparison. Both worlds' aggregates are derived
/// *independently* from that world's own stored vectors
/// (`G6B_METHODOLOGY.md` §4, §18 R-3): a recorded `albums[]` block is never
/// consulted, and no aggregate is ever copied between worlds.
#[derive(Debug, Clone)]
pub struct AlbumRetrievalComparison {
    pub album_count: usize,
    pub evaluable: bool,
    pub note: Option<String>,
    pub seeds: Vec<AlbumSeedRetrieval>,
    pub reversals: Vec<ReversalEvent>,
}

/// The corrected measurement over one corpus.
#[derive(Debug, Clone)]
pub struct StoragePathComparison {
    pub dimension: usize,
    pub tracks: usize,
    pub pair_count: usize,
    /// Symmetric regime — both operands through the storage path. The G-6B
    /// quantity.
    pub symmetric_max_abs_delta: f64,
    pub symmetric_mean_abs_delta: f64,
    pub symmetric_min_cosine_candidate: f32,
    /// Mixed regime — only the second operand quantized. The historical
    /// instrument's quantity, carried for continuity with
    /// `fixtures/g6/REPORT.txt`.
    pub mixed_max_abs_delta: f64,
    pub min_cosine_reference: f32,
    pub sign_crossings: usize,
    /// Spearman over pairs excluding the self-pair diagonal.
    pub spearman_excluding_self: f64,
    pub max_norm_deviation: f64,
    pub seeds: Vec<SeedRetrieval>,
    pub reversals: Vec<ReversalEvent>,
    /// Pairs exactly tied after quantization but distinct in the reference.
    pub new_ties: usize,
    /// `cos(aggregate_f32, aggregate_f16)` minimum and mean. Report only.
    pub aggregate_min: f64,
    pub aggregate_mean: f64,
    /// Album-level retrieval over per-world recomputed aggregates.
    pub albums: AlbumRetrievalComparison,
}

/// `cos(aggregate_reference, aggregate_stored)` minimum and mean: the M14
/// quantity of `g6::album_aggregate_drift`, computed from two already-built
/// worlds (`src/g6.rs` is immutable, so the loop lives here, arithmetically
/// identical).
fn album_aggregate_drift_records(
    reference_records: &[TrackEmbedding],
    stored_records: &[TrackEmbedding],
) -> (f64, f64) {
    let reference_albums = eval::aggregate_albums(reference_records);
    let stored_albums = eval::aggregate_albums(stored_records);
    let mut max_drift = 0.0f64;
    let mut sum_drift = 0.0f64;
    for (left, right) in reference_albums.iter().zip(stored_albums.iter()) {
        let drift = f64::from(eval::cosine(&left.vector, &right.vector));
        max_drift = max_drift.max(drift);
        sum_drift += drift;
    }
    let count = reference_albums.len().max(1) as f64;
    (max_drift, sum_drift / count)
}

/// Album-level ordered-sequence comparison at the ADR-sketched limit.
///
/// Reference aggregates come from the reference vectors; stored aggregates
/// come from the stored vectors. Each world is then ranked with the same
/// `eval::album_nearest` procedure (query album excluded, score descending,
/// index tie-break), and the ordered prefixes are compared.
pub fn compare_album_retrieval(
    reference_records: &[TrackEmbedding],
    stored_records: &[TrackEmbedding],
) -> AlbumRetrievalComparison {
    let reference_albums = eval::aggregate_albums(reference_records);
    let stored_albums = eval::aggregate_albums(stored_records);
    let album_count = reference_albums.len();
    // A single album has no candidate albums, so album retrieval is not
    // evaluable at all; mismatched aggregate counts mean one world's
    // aggregation degenerated and the comparison would be undefined.
    if album_count < 2 || stored_albums.len() != album_count {
        return AlbumRetrievalComparison {
            album_count,
            evaluable: false,
            note: Some(format!(
                "album aggregation produced {} reference and {} stored albums; album retrieval is not evaluable",
                reference_albums.len(),
                stored_albums.len()
            )),
            seeds: Vec::new(),
            reversals: Vec::new(),
        };
    }

    let mut seeds = Vec::with_capacity(album_count);
    let mut reversals = Vec::new();
    for album_index in 0..album_count {
        let reference_neighbors =
            eval::album_nearest(&reference_albums, album_index, album_count - 1);
        let stored_neighbors = eval::album_nearest(&stored_albums, album_index, album_count - 1);
        let reference_order: Vec<usize> = reference_neighbors.iter().map(|n| n.index).collect();
        let stored_order: Vec<usize> = stored_neighbors.iter().map(|n| n.index).collect();
        let reference_scores: Vec<f32> = reference_neighbors.iter().map(|n| n.score).collect();

        let top1_identical = reference_order.first() == stored_order.first();
        let mut sequence_identical = BTreeMap::new();
        let mut sequence_evaluable = BTreeMap::new();
        let mut boundary_margin = BTreeMap::new();
        for k in RETRIEVAL_KS {
            let evaluable = reference_order.len() >= k && stored_order.len() >= k;
            let identical = evaluable
                && reference_order
                    .iter()
                    .take(k)
                    .eq(stored_order.iter().take(k));
            sequence_evaluable.insert(k, evaluable);
            sequence_identical.insert(k, identical);
            boundary_margin.insert(
                k,
                if reference_scores.len() > k {
                    Some(f64::from(reference_scores[k - 1] - reference_scores[k]))
                } else {
                    None
                },
            );
        }

        // Reversal census over the full album rankings, recorded
        // unconditionally with no exemption floor.
        let mut reference_position = BTreeMap::new();
        for (rank, member) in reference_order.iter().enumerate() {
            reference_position.insert(*member, rank);
        }
        let mut stored_position = BTreeMap::new();
        for (rank, member) in stored_order.iter().enumerate() {
            stored_position.insert(*member, rank);
        }
        let members: Vec<usize> = reference_position.keys().copied().collect();
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let (a, b) = (members[i], members[j]);
                let (Some(&rank_a), Some(&rank_b)) =
                    (reference_position.get(&a), reference_position.get(&b))
                else {
                    continue;
                };
                let (Some(&stored_rank_a), Some(&stored_rank_b)) =
                    (stored_position.get(&a), stored_position.get(&b))
                else {
                    continue;
                };
                if (rank_a < rank_b) != (stored_rank_a < stored_rank_b) {
                    let score_a = eval::cosine(
                        &reference_albums[album_index].vector,
                        &reference_albums[a].vector,
                    );
                    let score_b = eval::cosine(
                        &reference_albums[album_index].vector,
                        &reference_albums[b].vector,
                    );
                    let margin = (f64::from(score_a) - f64::from(score_b)).abs();
                    reversals.push(ReversalEvent {
                        seed: album_index,
                        a,
                        b,
                        reference_margin: margin,
                        margin_in_f32_ulps: margin / F32_SCORE_ULP,
                        rank_a,
                        rank_b,
                    });
                }
            }
        }

        seeds.push(AlbumSeedRetrieval {
            album_index,
            top1_identical,
            sequence_identical,
            sequence_evaluable,
            boundary_margin,
        });
    }

    AlbumRetrievalComparison {
        album_count,
        evaluable: true,
        note: None,
        seeds,
        reversals,
    }
}

/// Compare the f32 reference world against the f16 storage path, with **both**
/// operands of every pairwise score travelling through the path. The two
/// worlds are provided fully built; album aggregates are recomputed inside
/// each world.
pub fn compare_worlds(
    reference_records: &[TrackEmbedding],
    stored_records: &[TrackEmbedding],
) -> StoragePathComparison {
    let full = reference_records.len();
    let mut sym_max = 0.0f64;
    let mut sym_sum = 0.0f64;
    let mut sym_min = f32::INFINITY;
    let mut mixed_max = 0.0f64;
    let mut min_reference = f32::INFINITY;
    let mut sign_crossings = 0usize;
    let mut reference_flat: Vec<f64> = Vec::with_capacity(full * (full - 1));
    let mut stored_flat: Vec<f64> = Vec::with_capacity(full * (full - 1));
    for a in 0..full {
        for b in 0..full {
            if a == b {
                continue;
            }
            let reference_score =
                eval::cosine(&reference_records[a].vector, &reference_records[b].vector);
            // The G-6B quantity: both operands are stored vectors.
            let stored_score = eval::cosine(&stored_records[a].vector, &stored_records[b].vector);
            // The historical instrument's quantity: only `b` quantized.
            let mixed_score = eval::cosine(&reference_records[a].vector, &stored_records[b].vector);
            let delta = (f64::from(stored_score) - f64::from(reference_score)).abs();
            sym_max = sym_max.max(delta);
            sym_sum += delta;
            sym_min = sym_min.min(stored_score);
            mixed_max = mixed_max.max((f64::from(mixed_score) - f64::from(reference_score)).abs());
            min_reference = min_reference.min(reference_score);
            if reference_score > 0.0 && stored_score <= 0.0
                || reference_score < 0.0 && stored_score >= 0.0
            {
                sign_crossings += 1;
            }
            reference_flat.push(f64::from(reference_score));
            stored_flat.push(f64::from(stored_score));
        }
    }
    let pair_count = full * (full - 1);

    let mut max_norm_deviation = 0.0f64;
    for record in stored_records {
        let mut sum = 0.0f64;
        for value in &record.vector {
            sum += f64::from(*value) * f64::from(*value);
        }
        max_norm_deviation = max_norm_deviation.max((sum.sqrt() - 1.0).abs());
    }

    let mut seeds = Vec::with_capacity(full);
    let mut reversals = Vec::new();
    let mut new_ties = 0usize;
    for seed in 0..full {
        let reference_order: Vec<usize> = eval::nearest(reference_records, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.index)
            .collect();
        let stored_order: Vec<usize> = eval::nearest(stored_records, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.index)
            .collect();
        let reference_scores: Vec<f32> = eval::nearest(reference_records, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.score)
            .collect();
        let stored_scores: Vec<f32> = eval::nearest(stored_records, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.score)
            .collect();

        let top1_identical = reference_order.first() == stored_order.first();
        let mut sequence_identical = BTreeMap::new();
        let mut boundary_margin = BTreeMap::new();
        let mut boundary_population = BTreeMap::new();
        for k in RETRIEVAL_KS {
            let evaluable = reference_order.len() >= k && stored_order.len() >= k;
            let identical = evaluable
                && reference_order
                    .iter()
                    .take(k)
                    .eq(stored_order.iter().take(k));
            sequence_identical.insert(k, identical);
            // The gap across the k-th selection boundary: the last included
            // score minus the first excluded score. Positive, since the
            // ranking is descending.
            boundary_margin.insert(
                k,
                if reference_scores.len() > k {
                    Some(f64::from(reference_scores[k - 1] - reference_scores[k]))
                } else {
                    None
                },
            );
            if reference_scores.len() > k {
                let kth = reference_scores[k - 1];
                boundary_population.insert(
                    k,
                    reference_scores
                        .iter()
                        .filter(|score| {
                            (f64::from(**score) - f64::from(kth)).abs() <= NUMERICAL_CEILING
                        })
                        .count(),
                );
            }
        }

        // Reversal census: every pair whose relative order differs, with no
        // exemption. Also count new exact ties. A candidate missing from the
        // stored ranking (only possible when its stored cosine went
        // non-finite, which B-C3 rejects upstream) is skipped rather than
        // panicked on; the k-evaluability checks above keep the gated
        // comparison honest.
        let mut reference_position = BTreeMap::new();
        for (rank, member) in reference_order.iter().enumerate() {
            reference_position.insert(*member, rank);
        }
        let mut stored_position = BTreeMap::new();
        for (rank, member) in stored_order.iter().enumerate() {
            stored_position.insert(*member, rank);
        }
        let members: Vec<usize> = reference_position.keys().copied().collect();
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let (a, b) = (members[i], members[j]);
                let (Some(&rank_a), Some(&rank_b)) =
                    (reference_position.get(&a), reference_position.get(&b))
                else {
                    continue;
                };
                let (Some(&stored_rank_a), Some(&stored_rank_b)) =
                    (stored_position.get(&a), stored_position.get(&b))
                else {
                    continue;
                };
                if (rank_a < rank_b) != (stored_rank_a < stored_rank_b) {
                    let score_a = eval::cosine(
                        &reference_records[seed].vector,
                        &reference_records[a].vector,
                    );
                    let score_b = eval::cosine(
                        &reference_records[seed].vector,
                        &reference_records[b].vector,
                    );
                    let margin = (f64::from(score_a) - f64::from(score_b)).abs();
                    reversals.push(ReversalEvent {
                        seed,
                        a,
                        b,
                        reference_margin: margin,
                        margin_in_f32_ulps: margin / F32_SCORE_ULP,
                        rank_a,
                        rank_b,
                    });
                }
                if reference_scores[rank_a] != reference_scores[rank_b] {
                    let stored_a = stored_scores[stored_rank_a];
                    let stored_b = stored_scores[stored_rank_b];
                    if stored_a == stored_b {
                        new_ties += 1;
                    }
                }
            }
        }

        seeds.push(SeedRetrieval {
            seed,
            top1_identical,
            sequence_identical,
            boundary_margin,
            boundary_population,
        });
    }

    let (aggregate_min, aggregate_mean) =
        album_aggregate_drift_records(reference_records, stored_records);
    let albums = compare_album_retrieval(reference_records, stored_records);

    StoragePathComparison {
        dimension: reference_records
            .first()
            .map(|r| r.vector.len())
            .unwrap_or(0),
        tracks: full,
        pair_count,
        symmetric_max_abs_delta: sym_max,
        symmetric_mean_abs_delta: sym_sum / pair_count as f64,
        symmetric_min_cosine_candidate: sym_min,
        mixed_max_abs_delta: mixed_max,
        min_cosine_reference: min_reference,
        sign_crossings,
        spearman_excluding_self: g6::spearman(&reference_flat, &stored_flat),
        max_norm_deviation,
        seeds,
        reversals,
        new_ties,
        aggregate_min,
        aggregate_mean,
        albums,
    }
}

/// Convenience wrapper for a corpus whose vectors are known to encode: the
/// surrogate. The real path goes through [`compare_storage_path_records`] so
/// B-C3 rejections are measured, never panicked.
pub fn compare_storage_path(corpus: &Corpus) -> StoragePathComparison {
    compare_storage_path_records(&corpus.as_embeddings())
        .expect("the surrogate corpus consists of finite unit vectors; B-C3 rejects nothing")
}

/// Encode the reference world through the storage path and compare. `Err`
/// carries every B-C3 rejection; a rejected corpus has no defined comparison.
pub fn compare_storage_path_records(
    reference_records: &[TrackEmbedding],
) -> Result<StoragePathComparison, Vec<ComponentRejection>> {
    let vectors: Vec<Vec<f32>> = reference_records.iter().map(|r| r.vector.clone()).collect();
    let stored_vectors = encode_corpus_storage_path(&vectors)?;
    let stored_records: Vec<TrackEmbedding> = reference_records
        .iter()
        .zip(stored_vectors)
        .map(|(record, vector)| {
            let mut copy = record.clone();
            copy.vector = vector;
            copy
        })
        .collect();
    Ok(compare_worlds(reference_records, &stored_records))
}

/// The deterministic reproduction of the f32-ULP ordering flip: a case where
/// the f32 reference distinguishes two candidates, binary16 storage reverses
/// them, and the margin is far inside the old `2^-9` floor.
#[derive(Debug, Clone)]
pub struct FlipCase {
    pub found: bool,
    pub trials_searched: usize,
    pub dimension: usize,
    /// The reference margin that flipped, in f32 ULP units near 1.0.
    pub margin_in_f32_ulps: f64,
    /// The flipped margin as a raw cosine difference.
    pub reference_margin: f64,
    /// Track index presented as top-1 in the reference world.
    pub reference_top1: usize,
    /// Track index presented as top-1 in the stored world.
    pub stored_top1: usize,
}

/// Search a deterministic stream for the smallest clear case: two candidates
/// separated by a handful of f32 score ULPs whose presented top-1 flips under
/// binary16 storage, with a third candidate far below so the flip is
/// unambiguously the presented answer.
pub fn ulp_flip_case() -> FlipCase {
    let dimension = 1280usize;
    let mut stream = g6::ByteStream::new("g6b-ulp-flip");
    for trial in 0..64usize {
        let mut query: Vec<f32> = (0..dimension).map(|_| stream.signed() as f32).collect();
        g6::normalize(&mut query);
        let near = perturb_towards(&mut stream, &query, 0.93);
        // Perturb `near` by a controlled amount that targets a margin of a few
        // f32 ULPs; both candidates stay near cos 0.93 to the query.
        let away = perturb_by(&mut stream, &near, &query, 6.0e-4);
        let distractor = perturb_towards(&mut stream, &query, 0.5);

        let reference = vec![
            query.clone(),
            near.clone(),
            away.clone(),
            distractor.clone(),
        ];
        let stored: Vec<Vec<f32>> = reference.iter().map(|v| g6::to_f16_candidate(v)).collect();
        let reference_records = embeddings(&reference);
        let stored_records = embeddings(&stored);
        let reference_order: Vec<usize> = eval::nearest(&reference_records, 0, 3)
            .into_iter()
            .map(|n| n.index)
            .collect();
        let stored_order: Vec<usize> = eval::nearest(&stored_records, 0, 3)
            .into_iter()
            .map(|n| n.index)
            .collect();
        let score = |world: &[Vec<f32>], candidate: usize| {
            f64::from(eval::cosine(&world[0], &world[candidate]))
        };
        let margin = (score(&reference, 1) - score(&reference, 2)).abs();
        let flips = reference_order.first() != stored_order.first();
        // The reference must genuinely distinguish the pair, the flip must be
        // real, and the margin must sit far inside the old 2^-9 floor.
        if margin > 0.0 && margin < 9.765_625e-4 / 4.0 && flips {
            return FlipCase {
                found: true,
                trials_searched: trial + 1,
                dimension,
                margin_in_f32_ulps: margin / F32_SCORE_ULP,
                reference_margin: margin,
                reference_top1: *reference_order.first().expect("non-empty"),
                stored_top1: *stored_order.first().expect("non-empty"),
            };
        }
    }
    FlipCase {
        found: false,
        trials_searched: 64,
        dimension,
        margin_in_f32_ulps: 0.0,
        reference_margin: 0.0,
        reference_top1: 0,
        stored_top1: 0,
    }
}

fn embeddings(vectors: &[Vec<f32>]) -> Vec<TrackEmbedding> {
    vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| TrackEmbedding {
            spec: crate::corpus::TrackSpec {
                path: Default::default(),
                artist: format!("c{index:02}"),
                album: format!("c{index:02}"),
                title: format!("t{index:02}"),
            },
            duration_seconds: 0.0,
            source_sha256: String::new(),
            frame_count: 0,
            patch_count: 0,
            embedding_sha256: crate::docfmt::sha256_hex(&g6::f32_le_bytes(vector)),
            vector: vector.clone(),
        })
        .collect()
}

/// A unit vector at a target cosine from `base`, via an orthogonal direction.
fn perturb_towards(stream: &mut g6::ByteStream, base: &[f32], target_cos: f64) -> Vec<f32> {
    let direction = orthogonal_to(stream, &[base]);
    let c = target_cos.clamp(1e-9, 1.0 - 1e-9);
    let t = ((1.0 / (c * c)) - 1.0).sqrt();
    let mut out: Vec<f32> = base
        .iter()
        .zip(&direction)
        .map(|(l, r)| (f64::from(*l) + t * f64::from(*r)) as f32)
        .collect();
    g6::normalize(&mut out);
    out
}

/// A unit vector near `base`, offset along a direction orthogonal to every
/// constraint, by a fixed step `t`.
fn perturb_by(stream: &mut g6::ByteStream, base: &[f32], constraint: &[f32], t: f64) -> Vec<f32> {
    let direction = orthogonal_to(stream, &[base, constraint]);
    let mut out: Vec<f32> = base
        .iter()
        .zip(&direction)
        .map(|(l, r)| (f64::from(*l) + t * f64::from(*r)) as f32)
        .collect();
    g6::normalize(&mut out);
    out
}

/// A deterministic unit direction orthogonal to each given constraint.
fn orthogonal_to(stream: &mut g6::ByteStream, constraints: &[&[f32]]) -> Vec<f32> {
    let d = constraints[0].len();
    let mut vector: Vec<f32> = (0..d).map(|_| stream.signed() as f32).collect();
    for constraint in constraints {
        let dot: f64 = constraint
            .iter()
            .zip(&vector)
            .map(|(l, r)| f64::from(*l) * f64::from(*r))
            .sum();
        for index in 0..d {
            vector[index] = (f64::from(vector[index]) - dot * f64::from(constraint[index])) as f32;
        }
    }
    g6::normalize(&mut vector);
    vector
}

// ---------------------------------------------------------------------------
// The frozen gate machinery (G6B_METHODOLOGY.md §11 — evaluation only ever on
// the real corpus; the surrogate artefact stays measurements-only)
// ---------------------------------------------------------------------------

/// Per-gate outcome. A gate that cannot be evaluated is recorded as
/// **not evaluated**, never as passed (G6B_METHODOLOGY.md §11).
#[derive(Debug, Clone, PartialEq)]
pub enum GateState {
    Pass,
    Fail(String),
    NotEvaluated(String),
}

impl GateState {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateState::Pass => "PASS",
            GateState::Fail(_) => "FAIL",
            GateState::NotEvaluated(_) => "NOT EVALUATED",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            GateState::Pass => None,
            GateState::Fail(reason) | GateState::NotEvaluated(reason) => Some(reason),
        }
    }
}

/// The run verdict, exactly the §11 rule: **PASS** requires every required
/// gate to be evaluated and passing on every real corpus/configuration; any
/// violated gate is **FAIL**; anything required but not evaluated means the
/// gates could not all be established, which is **INCONCLUSIVE** — never a
/// PASS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Inconclusive => "INCONCLUSIVE",
        }
    }
}

/// One gate's decision plus the measurement it came from.
#[derive(Debug, Clone)]
pub struct GateEvaluation {
    pub id: &'static str,
    pub state: GateState,
    pub measurement: String,
}

/// The seven-gate evaluation of one corpus/configuration.
#[derive(Debug, Clone)]
pub struct CorpusGateReport {
    pub label: String,
    /// Corpus provenance line from the recorded metadata (real corpus only).
    pub provenance: Option<String>,
    pub dimension: usize,
    pub profile: ProfileSafetyMeasurement,
    /// `None` exactly when the storage path is undefined (B-C3 rejections).
    pub comparison: Option<StoragePathComparison>,
    pub gates: Vec<GateEvaluation>,
    pub verdict: Verdict,
}

impl CorpusGateReport {
    pub fn gate(&self, id: &str) -> &GateEvaluation {
        self.gates
            .iter()
            .find(|gate| gate.id == id)
            .unwrap_or_else(|| panic!("gate {id} missing from the evaluation"))
    }

    pub fn gate_ids(&self) -> Vec<&'static str> {
        self.gates.iter().map(|gate| gate.id).collect()
    }
}

fn evaluate_ba1(c: &StoragePathComparison) -> GateEvaluation {
    let measurement = format!(
        "symmetric_max_abs_delta_cosine={:.4e} (limit {BA1_LIMIT:e})",
        c.symmetric_max_abs_delta
    );
    let state = if c.symmetric_max_abs_delta <= BA1_LIMIT {
        GateState::Pass
    } else {
        GateState::Fail(format!(
            "symmetric max |delta cos| {:.4e} exceeds the frozen ceiling {BA1_LIMIT:e}",
            c.symmetric_max_abs_delta
        ))
    };
    GateEvaluation {
        id: "B-A1",
        state,
        measurement,
    }
}

fn evaluate_ba2(c: &StoragePathComparison) -> GateEvaluation {
    let measurement = format!(
        "stored_max_norm_deviation={:.4e} (limit {BA2_LIMIT:e})",
        c.max_norm_deviation
    );
    let state = if c.max_norm_deviation <= BA2_LIMIT {
        GateState::Pass
    } else {
        GateState::Fail(format!(
            "stored max norm deviation {:.4e} exceeds the frozen ceiling {BA2_LIMIT:e}",
            c.max_norm_deviation
        ))
    };
    GateEvaluation {
        id: "B-A2",
        state,
        measurement,
    }
}

fn evaluate_bb1(c: &StoragePathComparison) -> GateEvaluation {
    let changed = c.seeds.iter().filter(|seed| !seed.top1_identical).count();
    let measurement = format!("top1_changed_seeds={} of {}", changed, c.seeds.len());
    let state = if c.seeds.is_empty() {
        GateState::NotEvaluated("no seeds".to_string())
    } else if let Some(seed) = c.seeds.iter().find(|seed| !seed.top1_identical) {
        GateState::Fail(format!(
            "seed {} presents a different top-1 neighbour",
            seed.seed
        ))
    } else {
        GateState::Pass
    };
    GateEvaluation {
        id: "B-B1",
        state,
        measurement,
    }
}

fn evaluate_bb2(c: &StoragePathComparison) -> GateEvaluation {
    // Population preconditions first: an insufficient candidate set is
    // "not evaluated", never a pass (G6B_METHODOLOGY.md §11).
    let mut gaps: Vec<String> = Vec::new();
    for k in RETRIEVAL_KS {
        if c.tracks < k + 1 {
            gaps.push(format!(
                "ordered top-{k} needs {} tracks (have {})",
                k + 1,
                c.tracks
            ));
        }
    }
    if !c.albums.evaluable {
        gaps.push(
            c.albums
                .note
                .clone()
                .unwrap_or_else(|| "album aggregation unavailable".to_string()),
        );
    } else if c.albums.album_count < ALBUM_GATE_K + 1 {
        gaps.push(format!(
            "ordered top-{ALBUM_GATE_K} albums needs {} albums (have {})",
            ALBUM_GATE_K + 1,
            c.albums.album_count
        ));
    }
    if !gaps.is_empty() {
        return GateEvaluation {
            id: "B-B2",
            state: GateState::NotEvaluated(gaps.join("; ")),
            measurement: "sequence identity not evaluable for the full k set".to_string(),
        };
    }

    for k in RETRIEVAL_KS {
        if let Some(seed) = c
            .seeds
            .iter()
            .find(|seed| seed.sequence_identical.get(&k) != Some(&true))
        {
            return GateEvaluation {
                id: "B-B2",
                state: GateState::Fail(format!(
                    "seed {} ordered top-{k} sequence differs",
                    seed.seed
                )),
                measurement: format!("first differing track boundary k={k}"),
            };
        }
    }
    if let Some(seed) = c
        .albums
        .seeds
        .iter()
        .find(|seed| seed.sequence_identical.get(&ALBUM_GATE_K) != Some(&true))
    {
        return GateEvaluation {
            id: "B-B2",
            state: GateState::Fail(format!(
                "album seed {} ordered top-{ALBUM_GATE_K} sequence differs",
                seed.album_index
            )),
            measurement: format!("first differing album boundary k={ALBUM_GATE_K}"),
        };
    }

    let track_summary = RETRIEVAL_KS
        .iter()
        .map(|k| {
            let changed = c
                .seeds
                .iter()
                .filter(|seed| seed.sequence_identical.get(k) != Some(&true))
                .count();
            format!("k={k}:{changed}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let album_changed = c
        .albums
        .seeds
        .iter()
        .filter(|seed| seed.sequence_identical.get(&ALBUM_GATE_K) != Some(&true))
        .count();
    GateEvaluation {
        id: "B-B2",
        state: GateState::Pass,
        measurement: format!(
            "track sequence changed [{track_summary}] of {} seeds; album k={ALBUM_GATE_K}: {album_changed}/{} changed",
            c.seeds.len(),
            c.albums.album_count
        ),
    }
}

fn evaluate_bc1(profile: &ProfileSafetyMeasurement) -> GateEvaluation {
    let measurement = format!(
        "max_reference_norm_deviation={:.4e} over {} vectors (limit {BC1_TOLERANCE:e})",
        profile.max_norm_deviation, profile.vectors
    );
    let state = if profile.vectors == 0 {
        GateState::NotEvaluated("no reference vectors".to_string())
    } else if profile.max_norm_deviation <= BC1_TOLERANCE {
        GateState::Pass
    } else {
        GateState::Fail(format!(
            "max |norm-1| = {:.4e} on reference vector {:?} exceeds {BC1_TOLERANCE:e}",
            profile.max_norm_deviation, profile.max_norm_deviation_vector
        ))
    };
    GateEvaluation {
        id: "B-C1",
        state,
        measurement,
    }
}

fn evaluate_bc2(profile: &ProfileSafetyMeasurement) -> GateEvaluation {
    let measurement = format!(
        "max_subnormal_energy_fraction={:.4e} over {} vectors (limit {BC2_MAX_SUBNORMAL_ENERGY:e})",
        profile.max_subnormal_energy_fraction, profile.vectors
    );
    let state = if profile.vectors == 0 {
        GateState::NotEvaluated("no reference vectors".to_string())
    } else if profile.max_subnormal_energy_fraction <= BC2_MAX_SUBNORMAL_ENERGY {
        GateState::Pass
    } else {
        GateState::Fail(format!(
            "max subnormal energy fraction = {:.4e} on reference vector {:?} exceeds {BC2_MAX_SUBNORMAL_ENERGY:e}",
            profile.max_subnormal_energy_fraction, profile.max_subnormal_energy_vector
        ))
    };
    GateEvaluation {
        id: "B-C2",
        state,
        measurement,
    }
}

fn evaluate_bc3(profile: &ProfileSafetyMeasurement) -> GateEvaluation {
    let measurement = format!(
        "component_rejections={} over {} vectors ({} vector(s) affected; limit {BC3_MAX_REJECTIONS})",
        profile.rejections.len(),
        profile.vectors,
        profile.affected_vectors
    );
    let state = if profile.rejections.len() == BC3_MAX_REJECTIONS {
        GateState::Pass
    } else {
        let first = &profile.rejections[0];
        GateState::Fail(format!(
            "{} component rejection(s); first: vector {} component {} value {:e} ({:?})",
            profile.rejections.len(),
            first.vector_index,
            first.component_index,
            first.value,
            first.reason
        ))
    };
    GateEvaluation {
        id: "B-C3",
        state,
        measurement,
    }
}

/// Evaluate the seven frozen gates for one corpus. `comparison` is `None`
/// exactly when the storage path is undefined (B-C3 rejections), in which
/// case the retrieval/numerical gates are recorded as not evaluated — a
/// configuration that can never yield a PASS.
pub fn evaluate_gates(
    label: &str,
    profile: &ProfileSafetyMeasurement,
    comparison: Option<&StoragePathComparison>,
) -> CorpusGateReport {
    let storage_defined = profile.rejections.is_empty() && comparison.is_some();
    let not_evaluated = || {
        GateState::NotEvaluated(
            "storage path undefined: B-C3 found component rejections, so no f16 world exists to measure"
                .to_string(),
        )
    };

    let (ba1, ba2, bb1, bb2) = match comparison {
        Some(comparison) if storage_defined => (
            evaluate_ba1(comparison),
            evaluate_ba2(comparison),
            evaluate_bb1(comparison),
            evaluate_bb2(comparison),
        ),
        _ => (
            GateEvaluation {
                id: "B-A1",
                state: not_evaluated(),
                measurement: "not measured".to_string(),
            },
            GateEvaluation {
                id: "B-A2",
                state: not_evaluated(),
                measurement: "not measured".to_string(),
            },
            GateEvaluation {
                id: "B-B1",
                state: not_evaluated(),
                measurement: "not measured".to_string(),
            },
            GateEvaluation {
                id: "B-B2",
                state: not_evaluated(),
                measurement: "not measured".to_string(),
            },
        ),
    };

    let gates = vec![
        ba1,
        ba2,
        bb1,
        bb2,
        evaluate_bc1(profile),
        evaluate_bc2(profile),
        evaluate_bc3(profile),
    ];
    debug_assert!(
        gate_set_complete(&gates),
        "the seven frozen gates must all be evaluated in §11 order"
    );
    let verdict = verdict_of_gates(&gates);
    CorpusGateReport {
        label: label.to_string(),
        provenance: None,
        dimension: comparison.map(|c| c.dimension).unwrap_or(0),
        profile: profile.clone(),
        comparison: comparison.cloned(),
        gates,
        verdict,
    }
}

/// Measure the profile preconditions and evaluate the full gate table for one
/// corpus/configuration. This is the single entry point the real-corpus
/// runner uses; it is never invoked on the surrogate (§14 rule 3).
pub fn run_corpus_experiment(label: &str, records: &[TrackEmbedding]) -> CorpusGateReport {
    let vectors: Vec<Vec<f32>> = records.iter().map(|record| record.vector.clone()).collect();
    let profile = measure_profile_safety(&vectors);
    let comparison = if profile.rejections.is_empty() {
        compare_storage_path_records(records).ok()
    } else {
        None
    };
    let mut report = evaluate_gates(label, &profile, comparison.as_ref());
    report.dimension = records
        .first()
        .map(|record| record.vector.len())
        .unwrap_or(0);
    report
}

/// The §11 verdict rule over one corpus's gate table.
pub fn verdict_of_gates(gates: &[GateEvaluation]) -> Verdict {
    let mut verdict = Verdict::Pass;
    for gate in gates {
        match gate.state {
            GateState::Fail(_) => return Verdict::Fail,
            GateState::NotEvaluated(_) => verdict = Verdict::Inconclusive,
            GateState::Pass => {}
        }
    }
    verdict
}

/// The §11 verdict rule across corpora: PASS only when every gate is green on
/// every real corpus/configuration.
pub fn overall_verdict(corpora: &[CorpusGateReport]) -> Verdict {
    let mut overall = Verdict::Pass;
    for corpus in corpora {
        match corpus.verdict {
            Verdict::Fail => return Verdict::Fail,
            Verdict::Inconclusive => overall = Verdict::Inconclusive,
            Verdict::Pass => {}
        }
    }
    overall
}

/// The gates must be exactly the frozen seven, in §11 order.
pub fn gate_set_complete(gates: &[GateEvaluation]) -> bool {
    gates.len() == GATE_IDS.len() && gates.iter().zip(GATE_IDS).all(|(gate, id)| gate.id == id)
}

// ---------------------------------------------------------------------------
// The §17.5 freeze check: the frozen §11 rows, pinned byte-for-byte
// ---------------------------------------------------------------------------

/// The frozen §11 acceptance-criteria table header.
pub const FROZEN_TABLE_HEADER: &str =
    "| ID | Category | Metric | Threshold | Rationale | Source | Frozen before run? |";

/// The seven frozen gate rows of `G6B_METHODOLOGY.md` §11, byte-for-byte. The
/// real-corpus runner refuses to evaluate the gates unless every row is
/// present in the methodology document it is given (§17.5), and
/// `g6b_tests` pins these constants against the committed document.
pub const FROZEN_GATE_ROWS: [&str; 7] = [
    r"| **B-A1** | A — numerical | M1 `max abs(Δcos)`, symmetric storage path, all ordered pairs, every corpus | **≤ 2^-10 = 9.766e-4** | Theorem given §9 preconditions (component 2^-11 → angular bound 2^-10). A violation implies a precondition breach or instrument bug, so the gate is free. | binary16 structure; re-derived independently by both reviews | **YES** |",
    r"| **B-A2** | A — numerical | M5 `max abs(‖v'‖ − 1)` | **≤ 2^-10** | Theorem: `2 × 2^-11` on a unit vector. | binary16 structure | **YES** |",
    r"| **B-B1** | B — retrieval | M8 top-1 identity | **exact identity, every seed, every corpus** | The product defines no interchangeability of near-tied results, so a changed top-1 is a user-visible change. Threshold is definitional (a property), not statistical. Relaxable only by D-2. | ADR 0017 §5.8 (query shape); §8 of this document | **YES** |",
    r"| **B-B2** | B — retrieval | M9 ordered top-k sequence identity | **exact identity for k ∈ {1,5,10,12,20} (tracks) and k = 12 (albums), every seed, every corpus** | Responses carry `rank`, so intra-list order is user-visible; set-identity alone is insufficient. k-set from ADR 0017 §5.8 sketch (D-3). Definitional threshold. | ADR 0017 §5.8; verified k-coverage gap (§6.2) | **YES** |",
    r"| **B-C1** | C — profile | M15 L2 norm conformance | **`\|‖v‖ − 1\| ≤ 2^-10` for every reference vector** | Load-bearing safety requirement; places components at `1/√d`, clear of the subnormal knee. | FORMAT_SPEC §7.3 precondition; G-6 subnormal evidence | **YES** |",
    r"| **B-C2** | C — profile | M15 subnormal energy fraction φ | **≤ 2^-24 per vector** | Derived in §9 (subnormal perturbation ≤ half the normal-range budget). | binary16 structure | **YES** |",
    r"| **B-C3** | C — profile | M16 conversion overflow count | **0** | The specified conversion rejects unrepresentable values; a profile producing any is ineligible. | FORMAT_SPEC §7.4 | **YES** |",
];

/// Verify that the frozen criteria are present, unaltered, in the methodology
/// document the run was given (§17.5). A drift is a protocol failure: the
/// runner must not produce a verdict from criteria that moved.
pub fn check_criteria_freeze(methodology_text: &str) -> Result<(), String> {
    if !methodology_text.contains(FROZEN_TABLE_HEADER) {
        return Err("the frozen §11 table header is missing or altered".to_string());
    }
    for row in FROZEN_GATE_ROWS {
        if !methodology_text.contains(row) {
            let id = row.split('|').nth(1).unwrap_or("?").trim();
            return Err(format!(
                "frozen criteria row for {id} is missing or altered"
            ));
        }
    }
    if !methodology_text.contains("same-data prohibition") {
        return Err("the same-data prohibition (§14 rule 3) is missing".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Artefact rendering
// ---------------------------------------------------------------------------

/// Render the G-6B surrogate instrument-check baseline. Measurements only:
/// this artefact is explicitly **not** criteria evidence (the same-data
/// prohibition in `G6B_METHODOLOGY.md` §14), so the gate machinery is never
/// evaluated here.
pub fn render_report() -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# G-6B storage-path instrument check - surrogate, measurements only"
    );
    let _ = writeln!(
        out,
        "# NOT criteria evidence: the criteria were frozen in G6B_METHODOLOGY.md"
    );
    let _ = writeln!(
        out,
        "# before this artefact was generated, and the surrogate that motivated"
    );
    let _ = writeln!(
        out,
        "# the G-6A repair may never be used to evaluate them (same-data prohibition)."
    );
    let _ = writeln!(out, "corpus=surrogate (g6::Corpus, unchanged parameters)");
    let _ = writeln!(
        out,
        "storage_path=f32 -> binary16 serialize -> binary16 decode -> cosine, BOTH operands"
    );
    let _ = writeln!(
        out,
        "cosine=eval::cosine ranking=eval::nearest aggregate=eval::aggregate_albums"
    );
    let _ = writeln!(
        out,
        "retrieval_k={}",
        RETRIEVAL_KS
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let _ = writeln!(out, "numerical_ceiling_context_only={NUMERICAL_CEILING:e}");
    let _ = writeln!(
        out,
        "gate_evaluation=real-corpus only (same-data prohibition); this artefact carries measurements only"
    );
    let _ = writeln!(out);

    for (label, dimension) in [
        ("primary", g6::DIM_PRIMARY),
        ("secondary", g6::DIM_SECONDARY),
        ("tiny", g6::DIM_TINY),
    ] {
        let corpus = Corpus::surrogate(dimension, 0.35, 0.55);
        let profile = measure_profile_safety(&corpus.vectors);
        let comparison = compare_storage_path(&corpus);
        let _ = writeln!(
            out,
            "[corpus:{label}] dimension={} tracks={} pairs={}",
            comparison.dimension, comparison.tracks, comparison.pair_count
        );
        let _ = writeln!(
            out,
            "  symmetric_max_abs_delta_cosine={:.4e}  mean={:.4e}",
            comparison.symmetric_max_abs_delta, comparison.symmetric_mean_abs_delta
        );
        let _ = writeln!(
            out,
            "  mixed_max_abs_delta_cosine={:.4e}  (historical instrument; ratio sym/mixed={:.2})",
            comparison.mixed_max_abs_delta,
            comparison.symmetric_max_abs_delta / comparison.mixed_max_abs_delta
        );
        let _ = writeln!(
            out,
            "  min_cosine_reference={:.9} min_cosine_stored={:.9} sign_crossings={}",
            comparison.min_cosine_reference,
            comparison.symmetric_min_cosine_candidate,
            comparison.sign_crossings
        );
        let _ = writeln!(
            out,
            "  spearman_excluding_self={:.9} max_norm_deviation={:.4e} new_ties={}",
            comparison.spearman_excluding_self, comparison.max_norm_deviation, comparison.new_ties
        );
        let _ = writeln!(
            out,
            "  aggregate_cosine_min={:.12e} mean={:.12e} (report only)",
            comparison.aggregate_min, comparison.aggregate_mean
        );
        let _ = writeln!(
            out,
            "  profile_max_norm_deviation={:.4e} max_subnormal_energy_fraction={:.4e} storage_path_rejections={} (B-C measurements, never gated here)",
            profile.max_norm_deviation,
            profile.max_subnormal_energy_fraction,
            profile.rejections.len()
        );
        let top1_changed = comparison
            .seeds
            .iter()
            .filter(|seed| !seed.top1_identical)
            .count();
        let _ = writeln!(out, "  top1_changed_seeds={top1_changed}");
        for k in RETRIEVAL_KS {
            let changed = comparison
                .seeds
                .iter()
                .filter(|seed| !seed.sequence_identical.get(&k).copied().unwrap_or(true))
                .count();
            let min_margin = comparison
                .seeds
                .iter()
                .filter_map(|seed| seed.boundary_margin.get(&k).and_then(|margin| *margin))
                .fold(f64::INFINITY, f64::min);
            let max_population = comparison
                .seeds
                .iter()
                .filter_map(|seed| seed.boundary_population.get(&k).copied())
                .max()
                .unwrap_or(0);
            let _ = writeln!(
                out,
                "  k={k:<2} sequence_changed_seeds={changed} min_boundary_margin={min_margin:.3e} max_boundary_population={max_population} (population within numerical ceiling: context)"
            );
        }
        let albums = &comparison.albums;
        if albums.evaluable {
            let album_gate_changed = albums
                .seeds
                .iter()
                .filter(|seed| seed.sequence_identical.get(&ALBUM_GATE_K) != Some(&true))
                .count();
            let album_gate_margin = albums
                .seeds
                .iter()
                .filter_map(|seed| {
                    seed.boundary_margin
                        .get(&ALBUM_GATE_K)
                        .and_then(|margin| *margin)
                })
                .fold(f64::INFINITY, f64::min);
            let album_top1_changed = albums
                .seeds
                .iter()
                .filter(|seed| !seed.top1_identical)
                .count();
            let _ = writeln!(
                out,
                "  albums={} (aggregates recomputed per world from that world's stored vectors)",
                albums.album_count
            );
            let _ = writeln!(
                out,
                "  album k={ALBUM_GATE_K} sequence_changed_seeds={album_gate_changed} min_boundary_margin={album_gate_margin:.3e} (album gate k)"
            );
            let _ = writeln!(
                out,
                "  album_top1_changed_seeds={album_top1_changed} album_reversals={} (recorded unconditionally)",
                albums.reversals.len()
            );
        } else {
            let _ = writeln!(
                out,
                "  albums={} evaluable=false note={}",
                albums.album_count,
                albums.note.as_deref().unwrap_or("")
            );
        }
        let _ = writeln!(
            out,
            "  reversals={} (recorded unconditionally; no exemption floor)",
            comparison.reversals.len()
        );
        for event in comparison.reversals.iter().take(8) {
            let _ = writeln!(
                out,
                "    seed={:02} a={:02} b={:02} ranks=({},{}) margin={:.3e} = {:.1} f32 ULPs",
                event.seed,
                event.a,
                event.b,
                event.rank_a,
                event.rank_b,
                event.reference_margin,
                event.margin_in_f32_ulps
            );
        }
        let _ = writeln!(out);
    }

    let flip = ulp_flip_case();
    let _ = writeln!(
        out,
        "[ulp-flip-case] deterministic reproduction of the f32-resolvable ordering flip"
    );
    if flip.found {
        let _ = writeln!(out, "  found=true trials={}", flip.trials_searched);
        let _ = writeln!(
            out,
            "  dimension={} reference_margin={:.3e} = {:.1} f32 ULPs",
            flip.dimension, flip.reference_margin, flip.margin_in_f32_ulps
        );
        let _ = writeln!(
            out,
            "  reference_top1=t{:02} stored_top1=t{:02} (the presented answer changes)",
            flip.reference_top1, flip.stored_top1
        );
        let _ = writeln!(
            out,
            "  margin vs old 2^-9 floor: {:.1}x inside; f32 distinguishes the pair, f16 reverses it",
            1.953_125e-3 / flip.reference_margin
        );
    } else {
        let _ = writeln!(
            out,
            "  found=false (search exhausted; investigate before relying on the claim)"
        );
    }
    out
}

fn render_gate_line(out: &mut String, gate: &GateEvaluation) {
    let _ = match gate.state.reason() {
        Some(reason) => writeln!(
            out,
            "  gate {}={} ({}) reason: {}",
            gate.id,
            gate.state.as_str(),
            gate.measurement,
            reason
        ),
        None => writeln!(
            out,
            "  gate {}={} ({})",
            gate.id,
            gate.state.as_str(),
            gate.measurement
        ),
    };
}

fn render_corpus_reversals(out: &mut String, comparison: &StoragePathComparison) {
    let _ = writeln!(
        out,
        "  reversal_census_events={} album_reversal_events={} (recorded unconditionally; no exemption floor)",
        comparison.reversals.len(),
        comparison.albums.reversals.len()
    );
    for event in &comparison.reversals {
        let _ = writeln!(
            out,
            "    seed={:02} a={:02} b={:02} ranks=({},{}) margin={:.3e} = {:.1} f32 ULPs",
            event.seed,
            event.a,
            event.b,
            event.rank_a,
            event.rank_b,
            event.reference_margin,
            event.margin_in_f32_ulps
        );
    }
    for event in &comparison.albums.reversals {
        let _ = writeln!(
            out,
            "    album_seed={:02} a={:02} b={:02} ranks=({},{}) margin={:.3e} = {:.1} f32 ULPs",
            event.seed,
            event.a,
            event.b,
            event.rank_a,
            event.rank_b,
            event.reference_margin,
            event.margin_in_f32_ulps
        );
    }
}

/// Render the real-corpus verdict artefact: measurements, the seven frozen
/// gate decisions, and the overall verdict. Deterministic: no timestamp, no
/// path, no toolchain, no corpus metadata beyond the provenance lines the
/// caller supplies (which carry digests and model identity, never track
/// metadata).
pub fn render_real_run_report(
    criteria_freeze: &Result<(), String>,
    corpora: &[CorpusGateReport],
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# G-6B real-corpus gate evaluation - the frozen seven-gate verdict artefact"
    );
    let _ = writeln!(
        out,
        "# gates exactly as frozen in G6B_METHODOLOGY.md section 11; thresholds appear nowhere else"
    );
    let _ = writeln!(
        out,
        "# G-6: FAIL - KEEP F32 (unchanged; this artefact does not revise any gate status)"
    );
    let _ = writeln!(
        out,
        "# a PASS is the mechanical gate result; G-6 sign-off additionally requires the"
    );
    let _ = writeln!(
        out,
        "# section 17 items: human review of the full reversal census and D-1..D-6 dispositions"
    );
    match criteria_freeze {
        Ok(()) => {
            let _ = writeln!(
                out,
                "criteria_freeze=verified (frozen gate rows matched byte-for-byte at run time)"
            );
        }
        Err(reason) => {
            let _ = writeln!(out, "criteria_freeze=FAILED: {reason}");
        }
    }
    let _ = writeln!(out, "corpus_count={}", corpora.len());
    let overall = overall_verdict(corpora);
    let _ = writeln!(out, "overall_verdict={}", overall.as_str());
    let _ = writeln!(out);
    for corpus in corpora {
        let _ = writeln!(
            out,
            "[corpus:{}] verdict={}",
            corpus.label,
            corpus.verdict.as_str()
        );
        if let Some(provenance) = &corpus.provenance {
            let _ = writeln!(out, "provenance: {provenance}");
        }
        let _ = writeln!(
            out,
            "tracks={} albums={} dimension={} ordered_pairs={}",
            corpus.comparison.as_ref().map_or(0, |c| c.tracks),
            corpus
                .comparison
                .as_ref()
                .map_or(0, |c| c.albums.album_count),
            corpus.dimension,
            corpus.comparison.as_ref().map_or(0, |c| c.pair_count)
        );
        let _ = writeln!(
            out,
            "  gate_set_complete={}",
            gate_set_complete(&corpus.gates)
        );
        for gate in &corpus.gates {
            render_gate_line(&mut out, gate);
        }
        match &corpus.comparison {
            Some(comparison) => {
                let _ = writeln!(
                    out,
                    "  report_only: symmetric_mean_abs_delta_cosine={:.4e} spearman_excluding_self={:.9} min_effective_support={:.4e} new_ties={} sign_crossings={}",
                    comparison.symmetric_mean_abs_delta,
                    comparison.spearman_excluding_self,
                    corpus.profile.min_effective_support,
                    comparison.new_ties,
                    comparison.sign_crossings
                );
                render_corpus_reversals(&mut out, comparison);
            }
            None => {
                let _ = writeln!(
                    out,
                    "  comparison=not built (storage path undefined; see gate B-C3)"
                );
            }
        }
        if !corpus.profile.rejections.is_empty() {
            let _ = writeln!(out, "  rejections:");
            for rejection in &corpus.profile.rejections {
                let _ = writeln!(
                    out,
                    "    vector={} component={} value={:e} reason={:?}",
                    rejection.vector_index,
                    rejection.component_index,
                    rejection.value,
                    rejection.reason
                );
            }
        }
        let _ = writeln!(out);
    }
    out
}
