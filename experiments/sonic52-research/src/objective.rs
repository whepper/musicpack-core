//! Sonic52 Slice 7: learning objective & training design spike.
//!
//! Answers what Sonic52 should learn and what the smallest defensible
//! training design is — **without training anything**. No gradients are
//! applied, no optimizer exists, no loop runs, no weights are learned.
//! The only calculus in this module is hand-derived gradient *formulas*
//! inside tests, checked against finite differences to validate the
//! mathematical formulation (never to update parameters).
//!
//! Category discipline (never blurred):
//!
//! - **Evidence**: 52-D output, sigmoid output, the reported
//!   `Conv2D → Flatten → Dense(200) → Dense(52)` chain, reported
//!   nearest-neighbour use (all per `FORENSICS.md`).
//! - **Research choice**: H0 frontend/patching/stride/aggregation, the
//!   reference convolution, tensor layout, f32, the loss below, the
//!   training target, dataset construction, every default in this file.
//! - **Design requirement**: reproducibility/run-manifest rules.
//! - **Unresolved**: explicit `Undecided` states, never silent defaults.
//!
//! Central finding, stated up front: the Slice 6 BCE/multi-label
//! assumption is **retained as the v1 working assumption** (lineage
//! formulation + direct compatibility with the reported sigmoid head),
//! with metric-learning and classification-derived formulations kept as
//! documented alternatives blocked on pair/label data that does not
//! exist yet.

#![forbid(unsafe_code)]

use std::fmt;

// ---------------------------------------------------------------------
// candidate learning objectives
// ---------------------------------------------------------------------

/// Status of an objective candidate in the v1 design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveStatus {
    /// Selected working assumption for the first experiments.
    AssumedV1,
    /// Documented alternative, blocked on data/design that is named.
    DocumentedAlternative,
    /// Explicitly not selected; reason recorded alongside.
    Undecided,
}

/// Candidate learning formulations for Sonic52.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainingObjective {
    /// Multi-label semantic/tag prediction: 52 sigmoid outputs trained
    /// with binary cross-entropy. Retained v1 assumption: it is the
    /// lineage formulation (Slice 6 oracle) and the only candidate
    /// directly compatible with the reported sigmoid head.
    MultiLabelBce {
        /// Output width (52 for the v1 track).
        outputs: usize,
    },
    /// Triplet metric learning over (anchor, positive, negative).
    /// Blocked: needs curated positive/negative track relationships.
    MetricTriplet {
        /// Required margin (still a research parameter, not a fact).
        margin_bits: u32,
    },
    /// Supervised contrastive learning over labelled batches. Blocked:
    /// needs label-complete batches plus a temperature decision.
    SupervisedContrastive {
        /// Output width under study.
        outputs: usize,
    },
    /// Classification training with a separate embedding readout
    /// (e.g. the hidden layer). Blocked: needs a label taxonomy plus
    /// the readout decision (§7 of the Slice 7 brief).
    ClassificationDerived {
        /// Class count of the training head.
        classes: usize,
    },
}

impl TrainingObjective {
    /// Canonical deterministic encoding (digested for identity).
    pub fn canonical(&self) -> String {
        match self {
            TrainingObjective::MultiLabelBce { outputs } => {
                format!("objective-v1|bce-multilabel|{outputs}")
            }
            TrainingObjective::MetricTriplet { margin_bits } => {
                format!("objective-v1|triplet|margin-bits-{margin_bits}")
            }
            TrainingObjective::SupervisedContrastive { outputs } => {
                format!("objective-v1|supcon|{outputs}")
            }
            TrainingObjective::ClassificationDerived { classes } => {
                format!("objective-v1|classification|{classes}")
            }
        }
    }

    /// Deterministic objective identity.
    pub fn digest(&self) -> String {
        musicpack_core::format::checksum::sha256_hex(self.canonical().as_bytes())
    }

    /// Design status of this candidate.
    pub const fn status(&self) -> ObjectiveStatus {
        match self {
            TrainingObjective::MultiLabelBce { .. } => ObjectiveStatus::AssumedV1,
            TrainingObjective::MetricTriplet { .. }
            | TrainingObjective::SupervisedContrastive { .. }
            | TrainingObjective::ClassificationDerived { .. } => {
                ObjectiveStatus::DocumentedAlternative
            }
        }
    }
}

/// The v1 working assumption (not a finding): 52-output multi-label BCE.
pub const V1_OBJECTIVE: TrainingObjective = TrainingObjective::MultiLabelBce { outputs: 52 };

// ---------------------------------------------------------------------
// target/output compatibility (what each objective requires)
// ---------------------------------------------------------------------

/// Why a target tensor cannot train an objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// Target count differs from the objective's output width.
    DimMismatch {
        /// Required count.
        expected: usize,
        /// Offered count.
        found: usize,
    },
    /// BCE targets must be probabilities in [0, 1].
    TargetOutOfRange,
    /// Triplet objectives need three roles, not one target vector.
    NeedsTriple,
    /// Contrastive objectives need a labelled batch, not one vector.
    NeedsBatch,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::DimMismatch { expected, found } => {
                write!(f, "{found} targets for {expected} outputs")
            }
            TargetError::TargetOutOfRange => write!(f, "BCE targets must lie in [0, 1]"),
            TargetError::NeedsTriple => {
                write!(f, "triplet objective needs (anchor, positive, negative)")
            }
            TargetError::NeedsBatch => {
                write!(f, "contrastive objective needs a labelled batch")
            }
        }
    }
}

impl std::error::Error for TargetError {}

/// Validates one target vector against an objective's contract.
/// Metric objectives reject single vectors by construction: their data
/// requirements (triples, batches) are part of the formulation, and
/// accepting a lone vector would hide that.
pub fn validate_target(objective: &TrainingObjective, targets: &[f32]) -> Result<(), TargetError> {
    match objective {
        TrainingObjective::MultiLabelBce { outputs } => {
            if targets.len() != *outputs {
                return Err(TargetError::DimMismatch {
                    expected: *outputs,
                    found: targets.len(),
                });
            }
            if targets.iter().any(|t| !(0.0..=1.0).contains(t)) {
                return Err(TargetError::TargetOutOfRange);
            }
            Ok(())
        }
        TrainingObjective::MetricTriplet { .. } => Err(TargetError::NeedsTriple),
        TrainingObjective::SupervisedContrastive { .. } => Err(TargetError::NeedsBatch),
        TrainingObjective::ClassificationDerived { classes } => {
            if targets.len() != *classes {
                return Err(TargetError::DimMismatch {
                    expected: *classes,
                    found: targets.len(),
                });
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------
// target semantics: what "related" could mean (and its shortcuts)
// ---------------------------------------------------------------------

/// Candidate meanings of a positive training relationship, each with
/// its documented shortcut-learning risk. None is selected here;
/// selecting one is dataset design with consequences, not a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relationship {
    /// Same artist.
    SameArtist,
    /// Same album.
    SameAlbum,
    /// Same genre/tag label.
    SameGenreTag,
    /// Shared metadata tags.
    SharedTags,
    /// Curated musical relationships (human/expert judgement).
    CuratedSimilar,
    /// Listening-derived co-occurrence.
    ListeningDerived,
}

impl Relationship {
    /// Stable name for documentation tables.
    pub const fn as_str(self) -> &'static str {
        match self {
            Relationship::SameArtist => "same-artist",
            Relationship::SameAlbum => "same-album",
            Relationship::SameGenreTag => "same-genre-tag",
            Relationship::SharedTags => "shared-tags",
            Relationship::CuratedSimilar => "curated-similar",
            Relationship::ListeningDerived => "listening-derived",
        }
    }

    /// The shortcut this target invites: what the model may learn
    /// *instead of* general musical similarity. Stated before any
    /// target is chosen, so the choice stays informed.
    pub const fn shortcut_risk(self) -> &'static str {
        match self {
            Relationship::SameArtist => {
                "artist-specific production/mastering fingerprints rather than general similarity"
            }
            Relationship::SameAlbum => {
                "album-level production coherence (same sessions, same master) rather than track similarity"
            }
            Relationship::SameGenreTag => {
                "genre semantics (vocabulary boundaries) more than timbre, instrumentation, or arrangement"
            }
            Relationship::SharedTags => {
                "tag co-occurrence statistics and annotator bias rather than sonic content"
            }
            Relationship::CuratedSimilar => {
                "curator taste and catalogue coverage limits; expensive to scale, hard to version"
            }
            Relationship::ListeningDerived => {
                "popularity and exposure feedback loops rather than sonic similarity"
            }
        }
    }
}

// ---------------------------------------------------------------------
// dataset requirements per objective (design exercise, no dataset)
// ---------------------------------------------------------------------

/// What one objective demands from a future dataset. All fields are
/// documentation; nothing here downloads, scrapes, or invents data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetRequirements {
    /// Labels each sample must carry.
    pub labels: &'static str,
    /// What counts as a positive relationship.
    pub positives: &'static str,
    /// What counts as a negative relationship.
    pub negatives: &'static str,
    /// Minimum corpus guidance.
    pub min_corpus: &'static str,
    /// Leakage risks specific to this objective.
    pub leakage: &'static str,
    /// Expected imbalance handling.
    pub imbalance: &'static str,
    /// Metadata the Slice 6 gate must additionally require.
    pub metadata: &'static str,
    /// Licensing posture.
    pub licensing: &'static str,
    /// Augmentation needs.
    pub augmentation: &'static str,
    /// What evaluation needs beyond the eval protocol.
    pub evaluation: &'static str,
}

/// Requirements of each candidate objective. BCE's row is the only one
/// actionable under the Slice 6 gate today (per-sample licences +
/// multi-label targets); the metric rows additionally need curated
/// relationships that do not exist yet — which is exactly why they
/// stay alternatives.
pub const fn requirements(objective: &TrainingObjective) -> DatasetRequirements {
    match objective {
        TrainingObjective::MultiLabelBce { .. } => DatasetRequirements {
            labels: "per-track multi-label probability vectors (52 tag/class probabilities)",
            positives: "n/a (no pairs; each sample carries its own target distribution)",
            negatives: "implicit: low-target outputs act as negatives per class",
            min_corpus: "undecided: scales with label vocabulary sparsity; pilot on synthetic targets first",
            leakage: "track-level split minimum (Slice 6 gate); tag-annotator overlap across splits unaddressed",
            imbalance: "per-class positive-rate weighting or focal-style reweighting (Slice 7 decision)",
            metadata: "licence id + tag vocabulary version + annotator/provenance id per sample",
            licensing: "Slice 6 per-sample gate unchanged: eligible-with-id only",
            augmentation: "undecided: time/frequency masking candidates; must preserve label semantics explicitly",
            evaluation: "held-out per-class calibration + rank metrics on frozen embeddings (no quality claim yet)",
        },
        TrainingObjective::MetricTriplet { .. } => DatasetRequirements {
            labels: "curated (anchor, positive, negative) relationships with a stated relation definition",
            positives: "curated similar pairs under the chosen Relationship semantics",
            negatives: "curated dissimilar items; mining policy undecided (random vs hard-negative schedule)",
            min_corpus: "undecided: triplet count grows superlinearly; needs a mining design first",
            leakage: "track-level split minimum PLUS artist/album disjointness across splits (stricter than BCE)",
            imbalance: "relationship-type balance across triplets; popularity bias in curated sources",
            metadata: "licence id + relationship definition version + curator/provenance id per triple",
            licensing: "Slice 6 per-sample gate applied to all three members of every triple",
            augmentation: "undecided: must be relationship-preserving by construction, not by hope",
            evaluation: "held-out retrieval metrics on frozen embeddings (no quality claim yet)",
        },
        TrainingObjective::SupervisedContrastive { .. } => DatasetRequirements {
            labels: "label-complete batches: every batch member carries its class/tag set",
            positives: "same-class batch mates under the chosen Relationship semantics",
            negatives: "all other batch members (in-batch negatives); temperature undecided",
            min_corpus: "undecided: needs large batches to supply negatives; corpus must fill them",
            leakage: "track-level split minimum PLUS class-distribution matching across splits",
            imbalance: "batch composition policy per class frequency (undecided)",
            metadata: "licence id + class taxonomy version per sample",
            licensing: "Slice 6 per-sample gate unchanged",
            augmentation: "undecided: two-view augmentation policy per sample (views must share labels)",
            evaluation: "held-out retrieval metrics on frozen embeddings (no quality claim yet)",
        },
        TrainingObjective::ClassificationDerived { .. } => DatasetRequirements {
            labels: "single-label class per sample under a fixed taxonomy version",
            positives: "same-class membership (embedding readout is separate from the head)",
            negatives: "other classes via the classification loss",
            min_corpus: "undecided: scales with class count and tail-class coverage",
            leakage: "track-level split minimum PLUS taxonomy-version pinning across splits",
            imbalance: "class-balanced sampling or loss reweighting (Slice 7 decision)",
            metadata: "licence id + taxonomy version + embedding-readout layer id",
            licensing: "Slice 6 per-sample gate unchanged",
            augmentation: "undecided: must preserve class membership explicitly",
            evaluation: "classification accuracy (diagnostic) PLUS frozen-embedding retrieval (no quality claim yet)",
        },
    }
}

// ---------------------------------------------------------------------
// output-vs-embedding decision (logits vs sigmoid)
// ---------------------------------------------------------------------

/// Which tensor is persisted as the similarity embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingSource {
    /// Raw `Dense(52)` logits.
    Logits,
    /// Post-sigmoid activations in [0, 1].
    SigmoidOut,
}

/// v1 selection: sigmoid outputs. Rationale (research choice, not a
/// proof): the forensic record says the persisted Plex representation
/// is 52-D and sigmoid-shaped, and BCE trains those outputs directly —
/// so persisting them keeps training target and stored representation
/// identical, with no extra normalization step to justify. Whether the
/// historical training objective was simply BCE on those outputs is
/// explicitly unknown (FORENSICS.md); this decision does not claim it.
pub const V1_EMBEDDING_SOURCE: EmbeddingSource = EmbeddingSource::SigmoidOut;

/// Post-aggregation normalization: UNDECIDED (no evidence, no
/// experiment yet — a future axis, not a silent default).
pub const V1_POST_AGGREGATION_NORMALIZATION: Option<&str> = None;

// ---------------------------------------------------------------------
// experiment matrix (small, controlled, mostly not run)
// ---------------------------------------------------------------------

/// Readiness of one matrix arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmStatus {
    /// Runnable with current plumbing (reference weights only).
    PlumbingReady,
    /// Documented but blocked: needs what the note names.
    BlockedOn(&'static str),
}

/// One controlled experiment arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentArm {
    /// Arm name (stable).
    pub name: &'static str,
    /// Output width under study.
    pub output_dim: usize,
    /// Head description.
    pub head: &'static str,
    /// Aggregation rule.
    pub aggregation: &'static str,
    /// Objective under study.
    pub objective: TrainingObjective,
    /// Readiness.
    pub status: ArmStatus,
}

/// The smallest matrix capable of answering the key questions: the v1
/// baseline, one width variant, one aggregation variant, and the two
/// documented (blocked) objective alternatives. Nothing here is
/// trained in Slice 7 of this document's scope — the matrix is the
/// plan, not the execution.
pub const EXPERIMENT_MATRIX_V1: &[ExperimentArm] = &[
    ExperimentArm {
        name: "v1-baseline",
        output_dim: 52,
        head: "52-D sigmoid",
        aggregation: "mean",
        objective: TrainingObjective::MultiLabelBce { outputs: 52 },
        status: ArmStatus::PlumbingReady,
    },
    ExperimentArm {
        name: "width-128",
        output_dim: 128,
        head: "128-D sigmoid",
        aggregation: "mean",
        objective: TrainingObjective::MultiLabelBce { outputs: 128 },
        status: ArmStatus::PlumbingReady,
    },
    ExperimentArm {
        name: "agg-normalize-then-mean",
        output_dim: 52,
        head: "52-D sigmoid",
        aggregation: "normalize-then-mean",
        objective: TrainingObjective::MultiLabelBce { outputs: 52 },
        status: ArmStatus::PlumbingReady,
    },
    ExperimentArm {
        name: "triplet-documented",
        output_dim: 52,
        head: "52-D sigmoid",
        aggregation: "mean",
        objective: TrainingObjective::MetricTriplet { margin_bits: 0 },
        status: ArmStatus::BlockedOn("curated positive/negative relationships"),
    },
    ExperimentArm {
        name: "classification-documented",
        output_dim: 52,
        head: "52-D sigmoid",
        aggregation: "mean",
        objective: TrainingObjective::ClassificationDerived { classes: 50 },
        status: ArmStatus::BlockedOn("label taxonomy + embedding-readout decision"),
    },
];

// ---------------------------------------------------------------------
// training-framework boundary (decision deferred with reasons)
// ---------------------------------------------------------------------

/// Future training-stack options. No option is adopted in Slice 7 of
/// this document's scope: adding a framework now, before the objective
/// and data exist, would be speculation dressed as design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingFramework {
    /// Hand-derived gradients in pure Rust (sufficient only for toy
    /// validation, not real training).
    PureRust,
    /// Train elsewhere (audited research environment), import frozen
    /// weights through the Slice 6 weight contract for Rust inference.
    ExternalResearchEnv,
    /// Python research training with Rust-only inference (same weight
    /// contract boundary as above, different tooling).
    PythonResearchTraining,
}

/// The framework decision is deferred: the objective, data, and scale
/// are not yet fixed, so no stack can be justified. The production
/// constraint stands regardless — MusicPack must not acquire a
/// heavyweight runtime merely for inference — and any research-only
/// training environment must export frozen weights through the Slice 6
/// weight contract, never a live runtime dependency.
pub const FRAMEWORK_DECISION: Option<TrainingFramework> = None;

// ---------------------------------------------------------------------
// reproducibility: training-run manifest
// ---------------------------------------------------------------------

/// Deterministic record of one future training run. Every
/// scientifically relevant input is a digest or pinned string; timing
/// and host facts have no fields here by construction (there is
/// nowhere to put them). The run identity changes if any listed
/// component changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainingRunManifest {
    /// Dataset identity (`id@version`).
    pub dataset: String,
    /// Dataset manifest digest (Slice 6).
    pub dataset_digest: String,
    /// Preprocessing contract text.
    pub preprocessing: String,
    /// Architecture identity digest.
    pub architecture: String,
    /// Weight-initialization identity digest.
    pub weight_init: String,
    /// Random seed (decimal).
    pub seed: u64,
    /// Objective identity digest.
    pub objective: String,
    /// Augmentation configuration text (`"none"` if unaugmented).
    pub augmentation: String,
    /// Optimizer identity (`name + version/pinning`).
    pub optimizer: String,
    /// Learning-rate schedule text.
    pub learning_rate: String,
    /// Batch size.
    pub batch_size: u32,
    /// Epoch/step budget text.
    pub budget: String,
    /// Software/toolchain identity (compiler, library versions).
    pub toolchain: String,
    /// Resulting learned-weight digest.
    pub weights: String,
    /// Evaluation manifest digest.
    pub evaluation: String,
}

impl TrainingRunManifest {
    /// Canonical deterministic serialization, fixed field order.
    pub fn canonical(&self) -> String {
        format!(
            "run-v1|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.dataset,
            self.dataset_digest,
            self.preprocessing,
            self.architecture,
            self.weight_init,
            self.seed,
            self.objective,
            self.augmentation,
            self.optimizer,
            self.learning_rate,
            self.batch_size,
            self.budget,
            self.toolchain,
            self.weights,
            self.evaluation
        )
    }

    /// Deterministic run identity.
    pub fn digest(&self) -> String {
        musicpack_core::format::checksum::sha256_hex(self.canonical().as_bytes())
    }
}

/// Fixture run manifest used by the tests (every axis exercised once).
pub fn fixture_run_manifest() -> TrainingRunManifest {
    TrainingRunManifest {
        dataset: "bank@v3".to_string(),
        dataset_digest: "dataset-digest".to_string(),
        preprocessing: crate::experiment::RESEARCH_V1_CONTRACT.to_string(),
        architecture: "arch-digest".to_string(),
        weight_init: "init-digest".to_string(),
        seed: 7,
        objective: V1_OBJECTIVE.digest(),
        augmentation: "none".to_string(),
        optimizer: "sgd-1.0".to_string(),
        learning_rate: "constant-0.01".to_string(),
        batch_size: 32,
        budget: "epochs-10".to_string(),
        toolchain: "rustc-1.85 + sonic52-research".to_string(),
        weights: "weights-digest".to_string(),
        evaluation: "eval-digest".to_string(),
    }
}

// ---------------------------------------------------------------------
// what can be validated now vs only with learned weights
// ---------------------------------------------------------------------

/// Explicit validation frontier: each question mapped to whether the
/// current research instrumentation can answer it. Anything marked
/// `requires-learned-weights` must not be claimed from plumbing tests.
pub const VALIDATION_TABLE: &[(&str, &str)] = &[
    ("tensor serialization", "now"),
    ("loss mathematics", "now"),
    ("sample identity", "now"),
    ("leakage rules", "now"),
    ("forward pass", "now"),
    ("gradient formulation", "tiny synthetic check now"),
    ("objective identity", "now"),
    ("run identity", "now"),
    ("useful embeddings", "requires-learned-weights"),
    ("best objective", "requires-learned-weights"),
    ("plex similarity reproduction", "requires-learned-weights"),
    ("human similarity quality", "requires-learned-weights"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::training::bce_with_logits;

    // -- objective identity + validation ---------------------------------------

    #[test]
    fn objective_identity_is_deterministic() {
        assert_eq!(V1_OBJECTIVE.digest(), V1_OBJECTIVE.digest());
        assert_eq!(V1_OBJECTIVE.digest().len(), 64);
        assert_eq!(V1_OBJECTIVE.status(), ObjectiveStatus::AssumedV1);
        assert_eq!(V1_OBJECTIVE.canonical(), "objective-v1|bce-multilabel|52");
        // Distinct candidates digest distinctly.
        assert_ne!(
            V1_OBJECTIVE.digest(),
            TrainingObjective::MultiLabelBce { outputs: 128 }.digest()
        );
        assert_ne!(
            V1_OBJECTIVE.digest(),
            TrainingObjective::MetricTriplet { margin_bits: 0 }.digest()
        );
        // Alternatives stay alternatives.
        assert_eq!(
            TrainingObjective::MetricTriplet { margin_bits: 0 }.status(),
            ObjectiveStatus::DocumentedAlternative
        );
    }

    #[test]
    fn target_output_combinations_are_checked() {
        // BCE: width match + [0,1] range.
        assert!(validate_target(&V1_OBJECTIVE, &[0.0; 52]).is_ok());
        assert!(validate_target(&V1_OBJECTIVE, &vec![1.0; 52]).is_ok());
        assert_eq!(
            validate_target(&V1_OBJECTIVE, &[0.0; 51]).unwrap_err(),
            TargetError::DimMismatch {
                expected: 52,
                found: 51
            }
        );
        assert_eq!(
            validate_target(&V1_OBJECTIVE, &[vec![0.0; 51], vec![2.0]].concat()).unwrap_err(),
            TargetError::TargetOutOfRange
        );
        assert_eq!(
            validate_target(&V1_OBJECTIVE, &[vec![0.0; 51], vec![-0.5]].concat()).unwrap_err(),
            TargetError::TargetOutOfRange
        );
        // Metric objectives reject lone vectors with their data
        // requirement attached, never a bare dimension error.
        assert_eq!(
            validate_target(
                &TrainingObjective::MetricTriplet { margin_bits: 0 },
                &[0.0; 52]
            )
            .unwrap_err(),
            TargetError::NeedsTriple
        );
        assert_eq!(
            validate_target(
                &TrainingObjective::SupervisedContrastive { outputs: 52 },
                &[0.0; 52]
            )
            .unwrap_err(),
            TargetError::NeedsBatch
        );
        // Classification head checks its own width.
        let classifier = TrainingObjective::ClassificationDerived { classes: 50 };
        assert!(validate_target(&classifier, &[0.0; 50]).is_ok());
        assert_eq!(
            validate_target(&classifier, &[0.0; 52]).unwrap_err(),
            TargetError::DimMismatch {
                expected: 50,
                found: 52
            }
        );
    }

    // -- synthetic problem: formulation validation only ----------------------------

    /// Toy linear-logit model evaluated in f64 (independent path from
    /// the f32 oracle): `loss = mean(max(l,0) − l·t + ln(1+e^−|l|))`.
    fn toy_loss_f64(params: &[f64; 3], inputs: &[[f64; 2]; 2], targets: &[f64; 2]) -> f64 {
        let mut total = 0.0;
        for (input, target) in inputs.iter().zip(targets.iter()) {
            let logit = params[0] * input[0] + params[1] * input[1] + params[2];
            total += logit.max(0.0) - logit * target + (1.0 + (-logit.abs()).exp()).ln();
        }
        total / targets.len() as f64
    }

    /// Hand-derived analytic gradient of the toy loss:
    /// `dL/dw_j = mean((sigmoid(l_i) − t_i) · x_ij)`,
    /// `dL/db = mean(sigmoid(l_i) − t_i)`.
    fn toy_analytic_gradient(
        params: &[f64; 3],
        inputs: &[[f64; 2]; 2],
        targets: &[f64; 2],
    ) -> [f64; 3] {
        let mut gradient = [0.0; 3];
        for (input, target) in inputs.iter().zip(targets.iter()) {
            let logit = params[0] * input[0] + params[1] * input[1] + params[2];
            let error = 1.0 / (1.0 + (-logit).exp()) - target;
            gradient[0] += error * input[0];
            gradient[1] += error * input[1];
            gradient[2] += error;
        }
        gradient.map(|component| component / targets.len() as f64)
    }

    const TOY_PARAMS: [f64; 3] = [0.3, -0.2, 0.1];
    const TOY_INPUTS: [[f64; 2]; 2] = [[1.0, 2.0], [3.0, -1.0]];
    const TOY_TARGETS: [f64; 2] = [1.0, 0.0];

    #[test]
    fn synthetic_problem_is_deterministic_finite_and_sensitive() {
        let loss = toy_loss_f64(&TOY_PARAMS, &TOY_INPUTS, &TOY_TARGETS);
        assert!(loss.is_finite());
        assert_eq!(loss, toy_loss_f64(&TOY_PARAMS, &TOY_INPUTS, &TOY_TARGETS));
        // Hand golden: computed term by term (see comment for values).
        // l1 = 0.3·1 − 0.2·2 + 0.1 = 0.0 → ln2 ≈ 0.693147.
        // l2 = 0.3·3 − 0.2·(−1) + 0.1 = 1.2, t = 0 →
        //   1.2 + ln(1 + e^−1.2) ≈ 1.2 + 0.263282 = 1.463282.
        // mean ≈ (0.693147 + 1.463282) / 2 ≈ 1.078215.
        assert!((loss - 1.078_215).abs() < 1e-5, "{loss}");
        // Parameter sensitivity: different params, different loss.
        let moved = toy_loss_f64(&[0.31, -0.2, 0.1], &TOY_INPUTS, &TOY_TARGETS);
        assert_ne!(loss, moved);
        // Nonzero gradient: the formulation has something to learn.
        let gradient = toy_analytic_gradient(&TOY_PARAMS, &TOY_INPUTS, &TOY_TARGETS);
        assert!(gradient.iter().any(|component| component.abs() > 1e-9));
    }

    #[test]
    fn finite_differences_confirm_the_hand_gradient() {
        // Central differences (eps 1e-5) on the f64 toy path agree with
        // the hand-derived analytic gradient to 1e-7. This validates
        // the mathematics, not an implementation: no autodiff system is
        // built, and no parameter is ever updated.
        let eps = 1e-5;
        let analytic = toy_analytic_gradient(&TOY_PARAMS, &TOY_INPUTS, &TOY_TARGETS);
        for j in 0..3 {
            let mut plus = TOY_PARAMS;
            let mut minus = TOY_PARAMS;
            plus[j] += eps;
            minus[j] -= eps;
            let numeric = (toy_loss_f64(&plus, &TOY_INPUTS, &TOY_TARGETS)
                - toy_loss_f64(&minus, &TOY_INPUTS, &TOY_TARGETS))
                / (2.0 * eps);
            assert!(
                (numeric - analytic[j]).abs() < 1e-7,
                "param {j}: numeric {numeric} vs analytic {}",
                analytic[j]
            );
        }
    }

    #[test]
    fn f32_oracle_agrees_with_f64_reference() {
        // The Slice 6 oracle (f32, fixed order) reproduces the f64 toy
        // path within float precision on equivalent inputs: weights
        // [1.2, 0, 0] over inputs [[0,0],[1,0]] yield logits [0, 1.2].
        let logits: Vec<f32> = [0.0, 1.2].to_vec();
        let targets: Vec<f32> = [1.0, 0.0].to_vec();
        let oracle = bce_with_logits(&logits, &targets).unwrap();
        let reference = toy_loss_f64(&[1.2, 0.0, 0.0], &[[0.0, 0.0], [1.0, 0.0]], &[1.0, 0.0]);
        assert!((oracle as f64 - reference).abs() < 1e-6);
        assert!((oracle as f64 - 1.078_215).abs() < 1e-5);
    }

    #[test]
    fn softmax_comparison_only() {
        // Comparison-only softmax + cross-entropy in test code (never
        // production, never the oracle): documents WHAT changes under a
        // single-label formulation — the loss value differs on duties
        // BCE treats independently.
        fn softmax_ce(logits: &[f64], target_index: usize) -> f64 {
            let max = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let shifted: Vec<f64> = logits.iter().map(|l| l - max).collect();
            let sum: f64 = shifted.iter().map(|l| l.exp()).sum();
            -(shifted[target_index] - sum.ln())
        }
        // Two independent binary duties vs one coupled choice differ:
        // logits [0.5, 0.5] score ln(1+e^−0.5) ≈ 0.474 under BCE but
        // ln 2 under single-target CE.
        let bce = toy_loss_f64(&[1.0, 0.0, 0.0], &[[0.5, 0.0], [0.5, 0.0]], &[1.0, 1.0]);
        let ce = softmax_ce(&[0.5, 0.5], 0);
        assert!((bce - 0.474_077).abs() < 1e-5);
        assert_ne!(bce, ce);
        // Uniform logits, single target: CE = ln(2), BCE duties differ.
        assert!((ce - 2.0f64.ln()).abs() < 1e-12);
    }

    // -- output vs embedding -------------------------------------------------

    #[test]
    fn embedding_source_selection_is_explicit() {
        assert_eq!(V1_EMBEDDING_SOURCE, EmbeddingSource::SigmoidOut);
        assert_eq!(V1_POST_AGGREGATION_NORMALIZATION, None);
    }

    // -- experiment matrix ------------------------------------------------------

    #[test]
    fn experiment_matrix_is_small_and_honest() {
        assert_eq!(EXPERIMENT_MATRIX_V1.len(), 5);
        let names: Vec<&str> = EXPERIMENT_MATRIX_V1.iter().map(|arm| arm.name).collect();
        assert_eq!(
            names,
            vec![
                "v1-baseline",
                "width-128",
                "agg-normalize-then-mean",
                "triplet-documented",
                "classification-documented"
            ]
        );
        // Ready arms need no missing data; blocked arms name it.
        for arm in EXPERIMENT_MATRIX_V1 {
            match arm.status {
                ArmStatus::PlumbingReady => {}
                ArmStatus::BlockedOn(reason) => assert!(!reason.is_empty(), "{}", arm.name),
            }
        }
        // The v1 baseline matches the frozen research contract fields.
        let baseline = &EXPERIMENT_MATRIX_V1[0];
        assert_eq!(baseline.output_dim, 52);
        assert_eq!(baseline.aggregation, "mean");
        assert_eq!(baseline.objective, V1_OBJECTIVE);
    }

    // -- target semantics ----------------------------------------------------------

    #[test]
    fn every_relationship_names_its_shortcut() {
        // No target semantics without a stated shortcut risk.
        for relationship in [
            Relationship::SameArtist,
            Relationship::SameAlbum,
            Relationship::SameGenreTag,
            Relationship::SharedTags,
            Relationship::CuratedSimilar,
            Relationship::ListeningDerived,
        ] {
            assert!(!relationship.as_str().is_empty());
            assert!(!relationship.shortcut_risk().is_empty());
        }
    }

    // -- dataset requirements ----------------------------------------------------------

    #[test]
    fn every_objective_states_its_data_demands() {
        // BCE is actionable under the Slice 6 gate; the metric rows
        // additionally demand curated relationships (why they wait).
        let bce = requirements(&V1_OBJECTIVE);
        assert!(bce.licensing.contains("Slice 6"));
        let triplet = requirements(&TrainingObjective::MetricTriplet { margin_bits: 0 });
        assert!(triplet.positives.contains("curated"));
        assert!(triplet.negatives.contains("mining"));
        let classifier = requirements(&TrainingObjective::ClassificationDerived { classes: 50 });
        assert!(classifier.labels.contains("taxonomy"));
        for objective in [
            V1_OBJECTIVE,
            TrainingObjective::MetricTriplet { margin_bits: 0 },
            TrainingObjective::SupervisedContrastive { outputs: 52 },
            TrainingObjective::ClassificationDerived { classes: 50 },
        ] {
            let required = requirements(&objective);
            assert!(!required.labels.is_empty());
            assert!(!required.leakage.is_empty());
            assert!(!required.evaluation.is_empty());
        }
    }

    // -- run identity ----------------------------------------------------------

    #[test]
    fn run_identity_separates_every_axis() {
        let base = fixture_run_manifest();
        assert_eq!(base.digest(), fixture_run_manifest().digest());
        assert_eq!(base.digest().len(), 64);
        // Each scientific axis changes the identity…
        let mut changed = base.clone();
        changed.dataset_digest = "other".to_string();
        assert_ne!(base.digest(), changed.digest());
        let mut changed = base.clone();
        changed.preprocessing = "other".to_string();
        assert_ne!(base.digest(), changed.digest());
        let mut changed = base.clone();
        changed.architecture = "other".to_string();
        assert_ne!(base.digest(), changed.digest());
        let mut changed = base.clone();
        changed.objective = "other".to_string();
        assert_ne!(base.digest(), changed.digest());
        let mut changed = base.clone();
        changed.seed = 8;
        assert_ne!(base.digest(), changed.digest());
        let mut changed = base.clone();
        changed.weights = "other".to_string();
        assert_ne!(base.digest(), changed.digest());
        // …while timing/host-style fields cannot, having no fields.
        assert!(!base.canonical().contains("ms"));
        assert!(!base.canonical().contains("host"));
    }

    // -- validation frontier ----------------------------------------------------------

    #[test]
    fn validation_table_states_the_frontier() {
        let lookup = |question: &str| {
            VALIDATION_TABLE
                .iter()
                .find(|(asked, _)| *asked == question)
                .map(|(_, status)| *status)
        };
        assert_eq!(lookup("loss mathematics"), Some("now"));
        assert_eq!(
            lookup("gradient formulation"),
            Some("tiny synthetic check now")
        );
        assert_eq!(
            lookup("useful embeddings"),
            Some("requires-learned-weights")
        );
        assert_eq!(
            lookup("human similarity quality"),
            Some("requires-learned-weights")
        );
        assert_eq!(lookup("no-such-question"), None);
    }

    // -- framework boundary ----------------------------------------------------------

    #[test]
    fn framework_decision_stays_deferred() {
        // No stack can be justified before objective, data, and scale
        // exist; the deferred state is the decision.
        assert_eq!(FRAMEWORK_DECISION, None);
    }
}
