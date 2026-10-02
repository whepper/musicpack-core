//! G-6: does f16 storage change similarity rankings beyond a predeclared
//! tolerance?
//!
//! **Status: EXPERIMENT. Not a decision, and not production code.** The
//! methodology and the acceptance criteria live in [`G6_F16.md`](../G6_F16.md)
//! and were fixed before any result was produced. This module measures; it does
//! not decide, and nothing here changes the reference storage encoding, which
//! stays `f32le`.
//!
//! ## What is compared
//!
//! ```text
//! reference : v                              (canonical f32, unit norm)
//! candidate : f16_bits_to_f32(f32_to_f16_bits(v))
//! ```
//!
//! Nothing else. The candidate is the *same* vector narrowed and widened, not an
//! independently generated one, and it is deliberately **not** re-normalized —
//! see G6_F16.md §4.2. `eval::cosine` divides by both norms, so the comparison is
//! already normalization-robust, and renormalizing would hide exactly the norm
//! perturbation under test.
//!
//! ## What is reused unchanged
//!
//! - `docfmt::f32_to_f16_bits` / `f16_bits_to_f32`: the already-specified RNE
//!   binary16 implementation (FORMAT_SPEC §7.4).
//! - `eval::cosine` and `eval::nearest`: the established comparison and ranking
//!   procedures, so this experiment is measured with the same yardstick as the
//!   recorded patch-hop evidence.
//!
//! ## Corpus
//!
//! The recorded Discogs-EffNet run outputs are not in this repository (G6_F16.md
//! §3), so the corpus here is a deterministic **surrogate**: 45 tracks in 15
//! clusters of 3 at 1280 dimensions, matching the shape of the recorded run.
//! Values come from a SHA-256 counter-mode stream rather than a float PRNG, so
//! the corpus is byte-reproducible on any platform.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::corpus::TrackSpec;
use crate::docfmt;
use crate::eval::{self, TrackEmbedding};

/// Clusters in the surrogate corpus, matching the recorded run's 15 albums.
pub const CLUSTERS: usize = 15;
/// Tracks per cluster, matching the recorded run's 3.
pub const PER_CLUSTER: usize = 3;
/// Seeds (queries) evaluated, matching the recorded run's 45 tracks.
pub const TRACKS: usize = CLUSTERS * PER_CLUSTER;
/// Primary dimension: Discogs-EffNet `multi`.
pub const DIM_PRIMARY: usize = 1280;
/// Secondary dimension, for the scaling study.
pub const DIM_SECONDARY: usize = 512;
/// A deliberately small dimension, where per-component relative error is
/// largest and any dimension-independent threshold is most likely to strain.
pub const DIM_TINY: usize = 16;

/// Top-k values reported.
pub const TOP_KS: [usize; 3] = [1, 5, 10];

// ---------------------------------------------------------------------------
// Deterministic value source
// ---------------------------------------------------------------------------

/// A SHA-256 counter-mode byte stream: deterministic on every platform, with no
/// dependency and no floating-point behaviour in the generator itself.
pub struct ByteStream {
    seed: Vec<u8>,
    counter: u64,
    buffer: Vec<u8>,
    cursor: usize,
}

impl ByteStream {
    pub fn new(seed: &str) -> Self {
        Self {
            seed: seed.as_bytes().to_vec(),
            counter: 0,
            buffer: Vec::new(),
            cursor: 0,
        }
    }

    fn refill(&mut self) {
        let mut hasher = Sha256::new();
        hasher.update(&self.seed);
        hasher.update(self.counter.to_be_bytes());
        self.buffer = hasher.finalize().to_vec();
        self.counter += 1;
        self.cursor = 0;
    }

    /// The next byte.
    pub fn byte(&mut self) -> u8 {
        if self.cursor >= self.buffer.len() {
            self.refill();
        }
        let byte = self.buffer[self.cursor];
        self.cursor += 1;
        byte
    }

    /// A value uniform in [0, 1) built from 8 bytes, so it is a pure function of
    /// the stream and not of the platform's float formatting.
    pub fn unit(&mut self) -> f64 {
        let mut raw = [0u8; 8];
        for byte in raw.iter_mut() {
            *byte = self.byte();
        }
        // 53 significand bits, the full f64 mantissa.
        (u64::from_be_bytes(raw) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A value uniform in [-1, 1).
    pub fn signed(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

use sha2::{Digest, Sha256};

/// L2-normalize in f64, matching the accumulation precision the producing
/// pipeline uses (`eval::norm` accumulates in f64).
pub fn normalize(values: &mut [f32]) {
    let mut sum = 0.0f64;
    for value in values.iter() {
        sum += f64::from(*value) * f64::from(*value);
    }
    let norm = sum.sqrt();
    if norm > 0.0 && norm.is_finite() {
        for value in values.iter_mut() {
            *value = (f64::from(*value) / norm) as f32;
        }
    }
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// A deterministic surrogate corpus of unit vectors with cluster structure.
///
/// Random iid unit vectors would make every pairwise cosine ≈ 0 and the ranking
/// trivially stable, which would be an unrealistically easy test. The cluster
/// structure reproduces what the experiment actually observed: same-album
/// neighbours scoring far above cross-album ones.
pub struct Corpus {
    pub dimension: usize,
    pub vectors: Vec<Vec<f32>>,
    /// Cluster index per track.
    pub cluster: Vec<usize>,
}

impl Corpus {
    /// Build the surrogate corpus.
    ///
    /// `spread` controls within-cluster separation and `cluster_spread`
    /// controls cross-cluster similarity. Both are fixed constants of the
    /// experiment, not tunables, so the corpus is a single well-defined object.
    pub fn surrogate(dimension: usize, spread: f64, cluster_spread: f64) -> Self {
        let mut stream = ByteStream::new(&format!(
            "g6-surrogate-d{dimension}-s{spread}-c{cluster_spread}"
        ));
        // Cluster centres: unit vectors with a controlled pairwise cosine band.
        let mut centres: Vec<Vec<f32>> = Vec::with_capacity(CLUSTERS);
        for _ in 0..CLUSTERS {
            let mut centre: Vec<f32> = (0..dimension).map(|_| stream.signed() as f32).collect();
            normalize(&mut centre);
            centres.push(centre);
        }
        // Pull the centres towards each other by `cluster_spread`, so
        // cross-cluster cosines are positive and varied rather than ~0.
        let mut pulled: Vec<Vec<f32>> = Vec::with_capacity(CLUSTERS);
        for index in 0..CLUSTERS {
            let mut value = centres[index].clone();
            for (other_index, other) in centres.iter().enumerate() {
                if other_index == index {
                    continue;
                }
                for (slot, component) in value.iter_mut().enumerate() {
                    *component += (other[slot] as f64 * cluster_spread) as f32;
                }
            }
            normalize(&mut value);
            pulled.push(value);
        }

        let mut vectors = Vec::with_capacity(TRACKS);
        let mut cluster = Vec::with_capacity(TRACKS);
        for (cluster_index, centre) in pulled.iter().enumerate() {
            for member in 0..PER_CLUSTER {
                let mut value = centre.clone();
                // Slightly different noise scale per member so within-cluster
                // cosines spread out instead of being identical.
                let scale = spread * (1.0 + 0.35 * member as f64);
                for slot in value.iter_mut() {
                    *slot += (stream.signed() * scale) as f32;
                }
                normalize(&mut value);
                vectors.push(value);
                cluster.push(cluster_index);
            }
        }
        Self {
            dimension,
            vectors,
            cluster,
        }
    }

    /// Wrap the corpus in the experiment's own `TrackEmbedding`, so the
    /// established `eval::nearest` can be used unchanged.
    ///
    /// Identities are anonymous and positional. There is no path, artist, album
    /// or title: the experiment generates no metadata of any kind.
    pub fn as_embeddings(&self) -> Vec<TrackEmbedding> {
        self.vectors
            .iter()
            .enumerate()
            .map(|(index, vector)| TrackEmbedding {
                spec: TrackSpec {
                    path: Default::default(),
                    artist: format!("c{:02}", self.cluster[index]),
                    album: format!("c{:02}", self.cluster[index]),
                    title: format!("t{index:02}"),
                },
                duration_seconds: 0.0,
                source_sha256: String::new(),
                frame_count: 0,
                patch_count: 0,
                embedding_sha256: docfmt::sha256_hex(&f32_le_bytes(vector)),
                vector: vector.clone(),
            })
            .collect()
    }
}

/// Canonical little-endian f32 bytes, matching how the experiment digests a
/// vector elsewhere.
pub fn f32_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// The candidate representation: narrow to binary16 and widen back, using only
/// the already-specified conversion. No renormalization.
pub fn to_f16_candidate(vector: &[f32]) -> Vec<f32> {
    vector
        .iter()
        .map(|value| {
            let bits = docfmt::f32_to_f16_bits(*value).expect(
                "a finite unit vector component is representable, or the run must report it",
            );
            docfmt::f16_bits_to_f32(bits)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// One seed's comparison between the two rankings.
#[derive(Debug, Clone)]
pub struct SeedOutcome {
    pub seed: usize,
    /// Jaccard overlap of the top-k sets, per k.
    pub jaccard: BTreeMap<usize, f64>,
    /// Rank displacement of each shared top-10 member, in positions.
    pub displacements: Vec<usize>,
    pub max_displacement: usize,
    pub mean_displacement: f64,
    /// f32 reference cosine of the top-1 neighbour, and its f16 counterpart.
    pub top1_reference: f32,
    pub top1_candidate: f32,
    pub top1_changed: bool,
    /// The cosine gap between reference rank 1 and rank 2, i.e. how close the
    /// decision was before quantization.
    pub top1_margin: f32,
    /// The full reference ranking, for swap counting.
    pub reference_order: Vec<usize>,
    pub candidate_order: Vec<usize>,
}

/// The full comparison for one corpus.
#[derive(Debug, Clone)]
pub struct Comparison {
    pub dimension: usize,
    pub tracks: usize,
    pub seeds: Vec<SeedOutcome>,
    /// Cosine of every ordered pair, reference and candidate.
    pub pair_count: usize,
    pub min_cosine_reference: f32,
    pub min_cosine_candidate: f32,
    pub max_abs_delta: f64,
    pub mean_abs_delta: f64,
    /// Pairs whose cosine changed sign — meaningless as a *relative* change, so
    /// reported separately rather than averaged.
    pub sign_crossings: usize,
    /// Pairs where the relative difference is meaningful (both cosines bounded
    /// away from zero).
    pub max_meaningful_relative: Option<f64>,
    pub spearman: f64,
    pub swaps: usize,
    /// The reference cosine margin each swap crossed, in the order found.
    pub swap_margins: Vec<f64>,
    /// L2 norm of the candidate vectors, which f16 perturbs slightly.
    pub candidate_norm_min: f64,
    pub candidate_norm_max: f64,
    pub candidate_norm_max_deviation: f64,
}

impl Comparison {
    pub fn mean_jaccard(&self, k: usize) -> f64 {
        if self.seeds.is_empty() {
            return f64::NAN;
        }
        self.seeds
            .iter()
            .map(|seed| seed.jaccard.get(&k).copied().unwrap_or(f64::NAN))
            .sum::<f64>()
            / self.seeds.len() as f64
    }

    pub fn seeds_with_changed_top(&self, k: usize) -> usize {
        self.seeds
            .iter()
            .filter(|seed| seed.jaccard.get(&k).copied().unwrap_or(1.0) < 1.0)
            .count()
    }

    pub fn top1_changed(&self) -> usize {
        self.seeds.iter().filter(|seed| seed.top1_changed).count()
    }

    pub fn max_displacement(&self) -> usize {
        self.seeds
            .iter()
            .map(|seed| seed.max_displacement)
            .max()
            .unwrap_or(0)
    }

    pub fn mean_displacement(&self) -> f64 {
        if self.seeds.is_empty() {
            return f64::NAN;
        }
        self.seeds
            .iter()
            .map(|seed| seed.mean_displacement)
            .sum::<f64>()
            / self.seeds.len() as f64
    }

    /// Displacement percentiles over every seed and every shared top-10 member.
    pub fn displacement_percentiles(&self) -> Vec<(usize, f64)> {
        let mut all: Vec<usize> = self
            .seeds
            .iter()
            .flat_map(|seed| seed.displacements.clone())
            .collect();
        all.sort_unstable();
        [50usize, 90, 95, 99, 100]
            .iter()
            .map(|percentile| {
                let index = if *percentile == 100 {
                    all.len().saturating_sub(1)
                } else {
                    ((all.len() - 1) * percentile) / 100
                };
                (*percentile, all.get(index).copied().unwrap_or(0) as f64)
            })
            .collect()
    }
}

/// Compare the two rankings over a whole corpus.
pub fn compare(corpus: &Corpus) -> Comparison {
    let reference = corpus.as_embeddings();
    let candidate: Vec<TrackEmbedding> = corpus
        .vectors
        .iter()
        .map(|vector| {
            let mut record = reference[0].clone();
            record.spec = TrackSpec {
                path: Default::default(),
                artist: String::new(),
                album: String::new(),
                title: String::new(),
            };
            record.embedding_sha256 = docfmt::sha256_hex(&f32_le_bytes(vector));
            record.vector = to_f16_candidate(vector);
            record
        })
        .collect();

    let full = corpus.vectors.len();
    let mut seeds = Vec::with_capacity(full);
    for seed in 0..full {
        // `eval::nearest` with a top_k of every other track gives the complete
        // ranking, which is what the swap and Spearman metrics need.
        let reference_order: Vec<usize> = eval::nearest(&reference, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.index)
            .collect();
        let candidate_order: Vec<usize> = eval::nearest(&candidate, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.index)
            .collect();
        let reference_scores: Vec<f32> = eval::nearest(&reference, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.score)
            .collect();
        let candidate_scores: Vec<f32> = eval::nearest(&candidate, seed, full - 1)
            .into_iter()
            .map(|neighbor| neighbor.score)
            .collect();

        let mut jaccard = BTreeMap::new();
        let mut displacements = Vec::new();
        for k in TOP_KS {
            let a: Vec<usize> = reference_order.iter().take(k).copied().collect();
            let b: Vec<usize> = candidate_order.iter().take(k).copied().collect();
            let intersection = a.iter().filter(|item| b.contains(item)).count();
            let union = a.len() + b.len() - intersection;
            jaccard.insert(
                k,
                if union == 0 {
                    1.0
                } else {
                    intersection as f64 / union as f64
                },
            );
        }
        // Rank displacement of each member present in both top-10 lists.
        for (position, member) in reference_order.iter().take(10).enumerate() {
            if let Some(other) = candidate_order.iter().take(10).position(|x| x == member) {
                displacements.push(position.abs_diff(other));
            }
        }
        let top1_changed = reference_order.first() != candidate_order.first();
        let top1_margin = if reference_scores.len() >= 2 {
            reference_scores[0] - reference_scores[1]
        } else {
            f32::NAN
        };
        seeds.push(SeedOutcome {
            seed,
            jaccard,
            max_displacement: displacements.iter().copied().max().unwrap_or(0),
            mean_displacement: if displacements.is_empty() {
                0.0
            } else {
                displacements.iter().sum::<usize>() as f64 / displacements.len() as f64
            },
            displacements,
            top1_reference: reference_scores.first().copied().unwrap_or(f32::NAN),
            top1_candidate: candidate_scores.first().copied().unwrap_or(f32::NAN),
            top1_changed,
            top1_margin,
            reference_order,
            candidate_order,
        });
    }

    // Pairwise scores over every ordered pair, reference and candidate.
    let mut pair_count = 0usize;
    let mut min_reference = f32::INFINITY;
    let mut min_candidate = f32::INFINITY;
    let mut max_abs_delta = 0.0f64;
    let mut sum_abs_delta = 0.0f64;
    let mut sign_crossings = 0usize;
    let mut max_relative = 0.0f64;
    let mut any_relative = false;
    let mut reference_flat: Vec<f64> = Vec::with_capacity(full * full);
    let mut candidate_flat: Vec<f64> = Vec::with_capacity(full * full);
    for a in 0..full {
        for b in 0..full {
            if a == b {
                reference_flat.push(1.0);
                candidate_flat.push(1.0);
                continue;
            }
            let reference = eval::cosine(&corpus.vectors[a], &corpus.vectors[b]);
            let narrowed = to_f16_candidate(&corpus.vectors[b]);
            let candidate = eval::cosine(&corpus.vectors[a], &narrowed);
            pair_count += 1;
            min_reference = min_reference.min(reference);
            min_candidate = min_candidate.min(candidate);
            let delta = (f64::from(candidate) - f64::from(reference)).abs();
            max_abs_delta = max_abs_delta.max(delta);
            sum_abs_delta += delta;
            if reference > 0.0 && candidate <= 0.0 || reference < 0.0 && candidate >= 0.0 {
                sign_crossings += 1;
            }
            // Relative difference is only meaningful where the reference is
            // bounded away from zero; a cosine near zero has no meaningful
            // relative error, and averaging one in would be meaningless.
            if reference.abs() > 0.1 {
                let relative = delta / f64::from(reference).abs();
                max_relative = max_relative.max(relative);
                any_relative = true;
            }
            reference_flat.push(f64::from(reference));
            candidate_flat.push(f64::from(candidate));
        }
    }

    // Relative-order swaps: an unordered pair whose preference order reverses
    // for at least one seed. For each, record the reference cosine margin that
    // the reversal crossed, so a swap in the arbitrary near-zero tail can be
    // told apart from one between genuine neighbours.
    let mut swapped: Vec<(usize, usize)> = Vec::new();
    let mut swap_margins: Vec<f64> = Vec::new();
    for seed in &seeds {
        let mut position = BTreeMap::new();
        for (rank, member) in seed.reference_order.iter().enumerate() {
            position.insert(*member, rank);
        }
        let mut candidate_position = BTreeMap::new();
        for (rank, member) in seed.candidate_order.iter().enumerate() {
            candidate_position.insert(*member, rank);
        }
        let members: Vec<usize> = position.keys().copied().collect();
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let (a, b) = (members[i], members[j]);
                let reference_prefers_a = position[&a] < position[&b];
                let candidate_prefers_a = candidate_position[&a] < candidate_position[&b];
                if reference_prefers_a != candidate_prefers_a {
                    let key = if a < b { (a, b) } else { (b, a) };
                    if !swapped.contains(&key) {
                        swapped.push(key);
                        // The margin the reversal crossed, in the reference world.
                        let score_a =
                            eval::cosine(&corpus.vectors[seed.seed], &corpus.vectors[key.0]);
                        let score_b =
                            eval::cosine(&corpus.vectors[seed.seed], &corpus.vectors[key.1]);
                        swap_margins.push((f64::from(score_a) - f64::from(score_b)).abs());
                    }
                }
            }
        }
    }

    let mut candidate_norm_min = f64::INFINITY;
    let mut candidate_norm_max = f64::NEG_INFINITY;
    let mut max_deviation = 0.0f64;
    for record in &candidate {
        let mut sum = 0.0f64;
        for value in &record.vector {
            sum += f64::from(*value) * f64::from(*value);
        }
        let norm = sum.sqrt();
        candidate_norm_min = candidate_norm_min.min(norm);
        candidate_norm_max = candidate_norm_max.max(norm);
        max_deviation = max_deviation.max((norm - 1.0).abs());
    }

    Comparison {
        dimension: corpus.dimension,
        tracks: full,
        pair_count,
        min_cosine_reference: min_reference,
        min_cosine_candidate: min_candidate,
        max_abs_delta,
        mean_abs_delta: if pair_count == 0 {
            f64::NAN
        } else {
            sum_abs_delta / pair_count as f64
        },
        sign_crossings,
        max_meaningful_relative: if any_relative {
            Some(max_relative)
        } else {
            None
        },
        spearman: spearman(&reference_flat, &candidate_flat),
        swaps: swapped.len(),
        swap_margins,
        candidate_norm_min,
        candidate_norm_max,
        candidate_norm_max_deviation: max_deviation,
        seeds,
    }
}

/// Spearman ρ between two score vectors, via Pearson on ranks with average ranks
/// for ties. Both inputs here are cosine values over the same pairs, so the
/// ranking being compared is the ranking those cosines induce.
pub fn spearman(left: &[f64], right: &[f64]) -> f64 {
    if left.len() != right.len() || left.len() < 2 {
        return f64::NAN;
    }
    let left_ranks = average_ranks(left);
    let right_ranks = average_ranks(right);
    let n = left_ranks.len() as f64;
    let mean_left = left_ranks.iter().sum::<f64>() / n;
    let mean_right = right_ranks.iter().sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut variance_left = 0.0;
    let mut variance_right = 0.0;
    for index in 0..left_ranks.len() {
        let dl = left_ranks[index] - mean_left;
        let dr = right_ranks[index] - mean_right;
        covariance += dl * dr;
        variance_left += dl * dl;
        variance_right += dr * dr;
    }
    if variance_left == 0.0 || variance_right == 0.0 {
        return f64::NAN;
    }
    covariance / (variance_left.sqrt() * variance_right.sqrt())
}

/// Ranks with ties averaged, ascending (rank 1 = smallest).
fn average_ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| values[*a].total_cmp(&values[*b]).then_with(|| a.cmp(b)));
    let mut ranks = vec![0.0f64; values.len()];
    let mut index = 0usize;
    while index < order.len() {
        let mut end = index;
        while end + 1 < order.len() && values[order[end + 1]] == values[order[index]] {
            end += 1;
        }
        let average = ((index + end) as f64) / 2.0 + 1.0;
        for slot in index..=end {
            ranks[order[slot]] = average;
        }
        index = end + 1;
    }
    ranks
}

// ---------------------------------------------------------------------------
// Adversarial and boundary cases
// ---------------------------------------------------------------------------

/// One constructed case and what it found.
#[derive(Debug, Clone)]
pub struct BoundaryCase {
    pub name: &'static str,
    pub intent: &'static str,
    pub dimension: usize,
    pub pairs: usize,
    /// Smallest |Δcos| observed.
    pub min_abs_delta: f64,
    pub max_abs_delta: f64,
    /// Pairs whose preference order reversed.
    pub swaps: usize,
    /// Pairs that became exactly tied after quantization.
    pub new_ties: usize,
    /// True when at least one component overflowed binary16.
    pub overflowed: bool,
}

/// Two unit vectors constructed to sit at a **target cosine** from each other.
///
/// `v = normalize(u + t·n)` with `n` orthogonal to `u` gives
/// `cos(u,v) = 1/sqrt(1+t²)`, so a target cosine can be set directly and `t`
/// solved for. This replaced a cos-wave probe whose "separation" parameter did
/// not produce a controlled vector distance and gave erratic, non-monotonic
/// results; targeting the cosine is both cleaner and the quantity a similarity
/// index actually compares.
///
/// Returns the pair and the cosine actually achieved.
pub fn cosine_pair(dimension: usize, target_cosine: f64) -> (Vec<f32>, Vec<f32>, f64) {
    let mut stream = ByteStream::new(&format!("g6-resolution-d{dimension}"));
    let mut u: Vec<f32> = (0..dimension).map(|_| stream.signed() as f32).collect();
    normalize(&mut u);
    // Project an independent vector orthogonal to u, then unit-normalize it.
    let mut n: Vec<f32> = (0..dimension).map(|_| stream.signed() as f32).collect();
    let dot: f64 = u
        .iter()
        .zip(&n)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum();
    for index in 0..dimension {
        n[index] = (f64::from(n[index]) - dot * f64::from(u[index])) as f32;
    }
    normalize(&mut n);
    let clamped = target_cosine.clamp(1e-15, 1.0 - 1e-15);
    let t = ((1.0 / (clamped * clamped)) - 1.0).sqrt();
    let mut v: Vec<f32> = u
        .iter()
        .zip(&n)
        .map(|(left, right)| (f64::from(*left) + t * f64::from(*right)) as f32)
        .collect();
    normalize(&mut v);
    let achieved = f64::from(eval::cosine(&u, &v));
    (u, v, achieved)
}

/// One point on the cosine-targeted sweep.
#[derive(Debug, Clone)]
pub struct ResolutionPoint {
    pub target_cosine: f64,
    pub achieved_cosine: f64,
    pub abs_delta: f64,
    /// True when f16 still represents the two vectors, and their cosine, as
    /// distinct.
    pub distinct: bool,
}

/// Sweep the target cosine downwards, three steps per decade, reporting both the
/// perturbation and whether the distinction survives at all.
pub fn resolution_sweep(dimension: usize) -> Vec<ResolutionPoint> {
    let mut out = Vec::new();
    for exponent in 0..=9i32 {
        for multiplier in [5.0f64, 2.0, 1.0] {
            let delta = multiplier * 10.0f64.powi(-exponent);
            let target = 1.0 - delta;
            if target <= 0.0 {
                continue;
            }
            let (u, v, achieved) = cosine_pair(dimension, target);
            let a = to_f16_candidate(&u);
            let b = to_f16_candidate(&v);
            let candidate = f64::from(eval::cosine(&a, &b));
            out.push(ResolutionPoint {
                target_cosine: target,
                achieved_cosine: achieved,
                abs_delta: (candidate - achieved).abs(),
                distinct: a != b && candidate != achieved,
            });
        }
    }
    out
}

/// **The resolution limit**: the closest two distinct unit vectors can be and
/// still be represented as distinct after f16, as `(achieved cosine, |Δcos|)`.
///
/// This is the number that matters to a similarity index, and it is the opposite
/// of the perturbation sweep. Below the limit f16 does not perturb the cosine —
/// it *erases the distinction between two tracks*, which is the false
/// near-duplicate failure C2 exists to catch. It tightens as the dimension grows,
/// because more components means smaller components, and smaller components means
/// fewer binary16 significand bits between them.
pub fn resolution_limit(dimension: usize) -> Option<(f64, f64)> {
    resolution_sweep(dimension)
        .into_iter()
        .filter(|point| point.distinct)
        .max_by(|left, right| left.achieved_cosine.total_cmp(&right.achieved_cosine))
        .map(|point| (point.achieved_cosine, point.abs_delta))
}

/// Relative error introduced by f16 as a function of a single component's
/// magnitude.
///
/// This quantifies the binary16 subnormal knee. Above `2^-14` the relative
/// precision is the constant `2^-11`. Below it, precision degrades as values
/// shrink, because subnormals are integer multiples of `2^-24`. It is the
/// quantitative form of the `subnormal-boundary` failure mode, and it is the
/// reason a unit-normalized embedding is safe while a small-magnitude vector is
/// not.
pub fn magnitude_precision_sweep() -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    // A deliberately non-representable mantissa. Powers of two are exact in
    // binary16 at every magnitude, so a power-of-two sweep would report zero
    // error everywhere and hide the effect entirely.
    const MANTISSA: f64 = 1.7;
    for exponent in -30i32..=2 {
        let magnitude = MANTISSA * 2.0f64.powi(exponent);
        let value = magnitude as f32;
        let narrowed = to_f16_candidate(&[value])[0];
        let relative = if value != 0.0 {
            (f64::from(narrowed) - f64::from(value)).abs() / f64::from(value).abs()
        } else {
            0.0
        };
        out.push((magnitude, relative));
    }
    out
}

/// The constructed boundary suite. Deterministic, and reported separately from
/// the corpus results.
pub fn boundary_cases() -> Vec<BoundaryCase> {
    let mut cases = Vec::new();

    // 1. Near-identical: two vectors differing in the last f32 bit.
    {
        let mut a = vec![0.0f32; 8];
        for (index, slot) in a.iter_mut().enumerate() {
            *slot = if index % 2 == 0 { 0.5 } else { -0.5 };
        }
        normalize(&mut a);
        let mut b = a.clone();
        b[3] = f32::from_bits(a[3].to_bits() + 1);
        cases.push(measure(
            "near-identical",
            "two unit vectors one f32 ULP apart",
            &a,
            &[b],
        ));
    }

    // 2. Near tie: a third vector placed between two candidates.
    {
        let mut a = vec![0.0f32; 6];
        for (index, slot) in a.iter_mut().enumerate() {
            *slot = if index < 3 { 0.5 } else { -0.5 };
        }
        normalize(&mut a);
        let mut b = a.clone();
        b[0] *= 1.0 - 1e-6;
        normalize(&mut b);
        let mut c = a.clone();
        c[1] *= 1.0 + 1e-6;
        normalize(&mut c);
        cases.push(measure(
            "near-tie",
            "three candidates within a quantization-sized band",
            &a,
            &[b, c],
        ));
    }

    // 3. Full binary16 normal range.
    {
        let exponents = [24.0f32, 12.0, 6.0, 1.0, 0.25, 0.0625, 0.0078125, -6.0];
        let a: Vec<f32> = exponents.to_vec();
        let b: Vec<f32> = exponents.iter().rev().copied().collect();
        let mut normalized_a = a;
        let mut normalized_b = b;
        normalize(&mut normalized_a);
        normalize(&mut normalized_b);
        cases.push(measure(
            "full-f16-range",
            "components spanning the binary16 normal range",
            &normalized_a,
            &[normalized_b],
        ));
    }

    // 4. Subnormal boundary of binary16.
    {
        let smallest = 2.0f32.powi(-24);
        let a = vec![smallest; 4];
        let b = vec![smallest, smallest * 1.5, smallest * 0.5, smallest * 2.0];
        cases.push(measure(
            "subnormal-boundary",
            "binary16 subnormal values at and around 2^-24",
            &a,
            &[b],
        ));
    }

    // 5. Rounding-sensitive: values exactly on a binary16 tie, forcing RNE-even.
    {
        let tie = 1.0f32 + 2.0f32.powi(-11);
        let a = vec![1.0f32, 1.0, 1.0, 1.0];
        let b = vec![tie, tie, 1.0, 1.0];
        cases.push(measure(
            "rounding-tie",
            "components landing exactly on a binary16 tie",
            &a,
            &[b],
        ));
    }

    // 6. Minimal dimension, where relative error per component is largest.
    {
        // Both components sit at 1/sqrt(2); the second vector differs in the
        // last bit of its second component, which is the smallest perturbation a
        // binary32 vector can carry.
        let unit = std::f32::consts::FRAC_1_SQRT_2;
        let a = vec![unit, unit];
        let b = vec![unit, f32::from_bits(unit.to_bits() + 1)];
        cases.push(measure(
            "dimension-2",
            "d=2, maximum relative error",
            &a,
            &[b],
        ));
        let a = vec![0.57735026f32, 0.57735026, 0.57735026];
        let b = vec![0.5773503f32, 0.57735026, 0.57735026];
        cases.push(measure("dimension-3", "d=3", &a, &[b]));
    }

    // 7. All-equal components: no small components to average error away.
    {
        let a = vec![0.5f32; 4];
        let b = vec![0.5f32, 0.5, 0.5000001, 0.5];
        cases.push(measure(
            "all-equal-components",
            "a vector whose components are all identical",
            &a,
            &[b],
        ));
    }

    cases
}

fn measure(
    name: &'static str,
    intent: &'static str,
    query: &[f32],
    candidates: &[Vec<f32>],
) -> BoundaryCase {
    let narrowed: Vec<Vec<f32>> = candidates
        .iter()
        .map(|vector| to_f16_candidate(vector))
        .collect();
    let reference_scores: Vec<f32> = candidates
        .iter()
        .map(|vector| eval::cosine(query, vector))
        .collect();
    let candidate_scores: Vec<f32> = narrowed
        .iter()
        .map(|vector| eval::cosine(query, vector))
        .collect();
    let mut min_delta = f64::INFINITY;
    let mut max_delta = 0.0f64;
    for index in 0..candidates.len() {
        let delta = (f64::from(candidate_scores[index]) - f64::from(reference_scores[index])).abs();
        min_delta = min_delta.min(delta);
        max_delta = max_delta.max(delta);
    }
    if candidates.is_empty() {
        min_delta = 0.0;
    }
    // Swaps within this small candidate set.
    let mut swaps = 0usize;
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            let reference_order = reference_scores[i] > reference_scores[j];
            let candidate_order = candidate_scores[i] > candidate_scores[j];
            if reference_order != candidate_order {
                swaps += 1;
            }
        }
    }
    // New exact ties created by quantization.
    let mut new_ties = 0usize;
    for i in 0..candidate_scores.len() {
        for j in (i + 1)..candidate_scores.len() {
            if reference_scores[i] != reference_scores[j]
                && candidate_scores[i] == candidate_scores[j]
            {
                new_ties += 1;
            }
        }
    }
    let overflowed = candidates
        .iter()
        .flatten()
        .any(|value| docfmt::f32_to_f16_bits(*value).is_none());
    BoundaryCase {
        name,
        intent,
        dimension: query.len(),
        pairs: candidates.len(),
        min_abs_delta: if min_delta.is_finite() {
            min_delta
        } else {
            0.0
        },
        max_abs_delta: max_delta,
        swaps,
        new_ties,
        overflowed,
    }
}

// ---------------------------------------------------------------------------
// Album aggregate (server-side derivation; v1.0 documents carry none)
// ---------------------------------------------------------------------------

/// f16 storage feeding the server-side album aggregate.
///
/// Reported because the server computes the aggregate from stored vectors, so
/// quantization error accumulates through the mean even though the v1.0 document
/// carries no aggregate (accepted decision B).
pub fn album_aggregate_drift(corpus: &Corpus) -> (f64, f64) {
    let reference = corpus.as_embeddings();
    let narrowed: Vec<TrackEmbedding> = reference
        .iter()
        .map(|record| {
            let mut copy = record.clone();
            copy.vector = to_f16_candidate(&record.vector);
            copy
        })
        .collect();
    let a = eval::aggregate_albums(&reference);
    let b = eval::aggregate_albums(&narrowed);
    let mut max_drift = 0.0f64;
    let mut sum_drift = 0.0f64;
    for (left, right) in a.iter().zip(b.iter()) {
        let drift = f64::from(eval::cosine(&left.vector, &right.vector));
        max_drift = max_drift.max(drift);
        sum_drift += drift;
    }
    let count = a.len().max(1) as f64;
    (max_drift, sum_drift / count)
}

// ---------------------------------------------------------------------------
// Storage arithmetic (contextual only; never a criterion)
// ---------------------------------------------------------------------------

/// Bytes per vector and percentage reduction, for representative dimensions.
pub fn storage_table(dimensions: &[usize]) -> Vec<(usize, usize, usize, f64)> {
    dimensions
        .iter()
        .map(|d| {
            let f32_bytes = d * 4;
            let f16_bytes = d * 2;
            let reduction = 100.0 * (1.0 - f16_bytes as f64 / f32_bytes as f64);
            (*d, f32_bytes, f16_bytes, reduction)
        })
        .collect()
}

/// Render a compact, deterministic, machine-checkable text report.
pub fn render_report() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# G-6 f16 vs f32 ranking equivalence — result");
    let _ = writeln!(
        out,
        "# generated by `cargo run --bin g6`; see G6_F16.md for the"
    );
    let _ = writeln!(
        out,
        "# predeclared criteria. This file records measurements only."
    );
    let _ = writeln!(
        out,
        "corpus=surrogate clusters={CLUSTERS} per_cluster={PER_CLUSTER} tracks={TRACKS}"
    );
    let _ = writeln!(out, "reference=f32 canonical unit vectors");
    let _ = writeln!(
        out,
        "candidate=f16_to_f32(f32_to_f16_bits(v)), no renormalization"
    );
    let _ = writeln!(
        out,
        "cosine=eval::cosine ranking=eval::nearest aggregate=eval::aggregate_albums"
    );
    let _ = writeln!(
        out,
        "conversion=RNE ties-to-even, subnormals supported, overflow rejected"
    );
    // The report is deliberately toolchain-independent so that two runs are
    // byte-comparable and the committed artefact is portable. The toolchain and
    // run provenance live in RUN.txt, which is a record of one run rather than a
    // regenerating artefact.
    let _ = writeln!(
        out,
        "experiment_crate_version={}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "run_provenance=fixtures/g6/RUN.txt");
    let _ = writeln!(out);

    // Surrogate corpus, three dimensions.
    for (label, dimension, spread, cluster_spread) in [
        ("primary", DIM_PRIMARY, 0.35f64, 0.55f64),
        ("secondary", DIM_SECONDARY, 0.35, 0.55),
        ("tiny", DIM_TINY, 0.35, 0.55),
    ] {
        let corpus = Corpus::surrogate(dimension, spread, cluster_spread);
        let comparison = compare(&corpus);
        let (max_drift, mean_drift) = album_aggregate_drift(&corpus);
        let _ = writeln!(
            out,
            "[corpus:{label}] dimension={} tracks={} pairs={}",
            comparison.dimension, comparison.tracks, comparison.pair_count
        );
        let _ = writeln!(
            out,
            "  relative_order_swaps={} ({:.4}% of pairs)",
            comparison.swaps,
            100.0 * comparison.swaps as f64 / comparison.pair_count.max(1) as f64
        );
        if comparison.swap_margins.is_empty() {
            let _ = writeln!(out, "  swap_margins=none");
        } else {
            let rendered: Vec<String> = comparison
                .swap_margins
                .iter()
                .map(|margin| format!("{margin:.3e}"))
                .collect();
            let _ = writeln!(
                out,
                "  swap_margins={} (reference cosine gap each reversal crossed)",
                rendered.join(" ")
            );
        }
        let _ = writeln!(
            out,
            "  min_cosine_reference={:.9}",
            comparison.min_cosine_reference
        );
        let _ = writeln!(
            out,
            "  min_cosine_candidate={:.9}",
            comparison.min_cosine_candidate
        );
        let _ = writeln!(
            out,
            "  max_abs_delta_cosine={:.3e}",
            comparison.max_abs_delta
        );
        let _ = writeln!(
            out,
            "  mean_abs_delta_cosine={:.3e}",
            comparison.mean_abs_delta
        );
        let _ = writeln!(out, "  spearman_rho={:.9}", comparison.spearman);
        for k in TOP_KS {
            let _ = writeln!(
                out,
                "  top{k}_jaccard_mean={:.6} top{k}_set_changed_seeds={}",
                comparison.mean_jaccard(k),
                comparison.seeds_with_changed_top(k)
            );
        }
        let _ = writeln!(
            out,
            "  top1_changed_seeds={} ({:.2}%)",
            comparison.top1_changed(),
            100.0 * comparison.top1_changed() as f64 / comparison.tracks as f64
        );
        let _ = writeln!(
            out,
            "  max_rank_displacement={}",
            comparison.max_displacement()
        );
        let _ = writeln!(
            out,
            "  mean_rank_displacement={:.6}",
            comparison.mean_displacement()
        );
        for (percentile, value) in comparison.displacement_percentiles() {
            let _ = writeln!(out, "  displacement_p{percentile}={value:.0}");
        }
        let _ = writeln!(
            out,
            "  candidate_norm_range={:.9}..{:.9} max_norm_deviation={:.3e}",
            comparison.candidate_norm_min,
            comparison.candidate_norm_max,
            comparison.candidate_norm_max_deviation
        );
        let _ = writeln!(
            out,
            "  album_aggregate_cosine_min={max_drift:.12e} mean={mean_drift:.12e}"
        );
        let _ = writeln!(out);
    }

    // Supplementary: a deliberately harder corpus whose within-cluster
    // neighbours are near-tied, so the ranking metrics are actually exercised.
    // NOT part of the verdict: the predeclared corpus parameters are the ones
    // above, and this exists only to locate the limit.
    {
        let corpus = Corpus::surrogate(DIM_PRIMARY, 0.02, 0.55);
        let comparison = compare(&corpus);
        let _ = writeln!(
            out,
            "[corpus:supplementary-near-tie] dimension={} tracks={} spread=0.02 (within-cluster neighbours deliberately near-tied; NOT part of the verdict)",
            comparison.dimension, comparison.tracks
        );
        let _ = writeln!(
            out,
            "  max_abs_delta_cosine={:.3e}",
            comparison.max_abs_delta
        );
        let _ = writeln!(
            out,
            "  mean_abs_delta_cosine={:.3e}",
            comparison.mean_abs_delta
        );
        let _ = writeln!(out, "  spearman_rho={:.9}", comparison.spearman);
        let _ = writeln!(
            out,
            "  relative_order_swaps={} ({:.4}% of pairs)",
            comparison.swaps,
            100.0 * comparison.swaps as f64 / comparison.pair_count.max(1) as f64
        );
        if comparison.swap_margins.is_empty() {
            let _ = writeln!(out, "  swap_margins=none");
        } else {
            let rendered: Vec<String> = comparison
                .swap_margins
                .iter()
                .map(|margin| format!("{margin:.3e}"))
                .collect();
            let _ = writeln!(
                out,
                "  swap_margins={} (reference cosine gap each reversal crossed)",
                rendered.join(" ")
            );
        }
        for k in TOP_KS {
            let _ = writeln!(
                out,
                "  top{k}_jaccard_mean={:.6} top{k}_set_changed_seeds={}",
                comparison.mean_jaccard(k),
                comparison.seeds_with_changed_top(k)
            );
        }
        let _ = writeln!(
            out,
            "  top1_changed_seeds={} ({:.2}%)",
            comparison.top1_changed(),
            100.0 * comparison.top1_changed() as f64 / comparison.tracks as f64
        );
        let _ = writeln!(
            out,
            "  max_rank_displacement={}",
            comparison.max_displacement()
        );
        let _ = writeln!(
            out,
            "  mean_rank_displacement={:.6}",
            comparison.mean_displacement()
        );
        for (percentile, value) in comparison.displacement_percentiles() {
            let _ = writeln!(out, "  displacement_p{percentile}={value:.0}");
        }
        let _ = writeln!(out);
    }

    // Separation sweep and the resolution limit.
    let _ = writeln!(
        out,
        "[separation-margin] cosine-targeted probe: |delta cos| and whether f16 keeps the pair distinct"
    );
    for dimension in [DIM_PRIMARY, DIM_SECONDARY, 64usize, 16, 4] {
        let sweep = resolution_sweep(dimension);
        let rendered: Vec<String> = sweep
            .iter()
            .map(|point| {
                format!(
                    "{:.0e}:{:.1e}{}",
                    point.achieved_cosine,
                    point.abs_delta,
                    if point.distinct { "" } else { "*" }
                )
            })
            .collect();
        let _ = writeln!(out, "  dimension={dimension} {}", rendered.join(" "));
    }
    let _ = writeln!(out, "  (* = f16 no longer distinguishes the pair)");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "[resolution-limit] closest pair still distinguished after f16"
    );
    for dimension in [DIM_PRIMARY, DIM_SECONDARY, 2048usize, 64, 16, 4] {
        let limit = resolution_limit(dimension);
        let _ = writeln!(
            out,
            "  dimension={dimension} {}",
            limit
                .map(|(cosine, delta)| {
                    format!(
                        "closest_cosine={cosine:.17} one_minus_cosine={:.3e} abs_delta={delta:.3e}",
                        1.0 - cosine
                    )
                })
                .unwrap_or_else(|| "merged at every probed separation".to_string())
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "[magnitude-precision] f16 relative error vs component magnitude (subnormal knee)"
    );
    for (magnitude, relative) in magnitude_precision_sweep() {
        if magnitude >= 2.0f64.powi(-26) {
            let _ = writeln!(out, "  magnitude={magnitude:.3e} relative={relative:.3e}");
        }
    }
    let _ = writeln!(out);

    // Boundary cases.
    let _ = writeln!(out, "[boundary]");
    for case in boundary_cases() {
        let _ = writeln!(
            out,
            "  case={} dimension={} pairs={} max_abs_delta={:.3e} swaps={} new_ties={} overflow={} intent={}",
            case.name,
            case.dimension,
            case.pairs,
            case.max_abs_delta,
            case.swaps,
            case.new_ties,
            case.overflowed,
            case.intent
        );
    }
    let _ = writeln!(out);

    // Storage, contextual only.
    let _ = writeln!(
        out,
        "[storage] bytes per vector; contextual only, never a criterion"
    );
    for (dimension, f32_bytes, f16_bytes, reduction) in storage_table(&[512, 1280, 2048, 4096]) {
        let _ = writeln!(
            out,
            "  dimension={dimension} f32={f32_bytes} f16={f16_bytes} reduction={reduction:.1}%"
        );
    }
    out
}
