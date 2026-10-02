//! Sonic52 Slice 6: training-readiness gate.
//!
//! A complete, deterministic contract for future training — **not
//! training itself**. Nothing here learns: no gradients, no autodiff,
//! no optimizers, no loops, no checkpoints, no learned weights, no
//! corpus, no quality evaluation. The outcome is the requirement stack
//! Slice 7 must implement without rediscovering it:
//!
//! ```text
//! dataset contract → sample identity → split/leakage contract
//!   → model contract → weight contract → loss oracle
//!   → evaluation contract → explicit Slice 7 boundary
//! ```
//!
//! Category discipline (see `FORENSICS.md`; never blurred here):
//!
//! - **Plex/reverse-engineering evidence**: 52-D output, sigmoid
//!   output, the reported `Conv2D → Flatten → Dense(200) → Dense(52)`
//!   chain. Nothing stronger is claimed.
//! - **Research choice**: 16 kHz, mono mean downmix, rubato resampling,
//!   512/256 framing, Hann, 96 Slaney mels 0–8 kHz, log compression,
//!   187-frame patches, stride 93, Discard, Mean aggregation, the
//!   reference convolution placeholder, tensor layout, f32, formula
//!   reference weights.
//! - **Future training requirement**: everything in this module.
//! - **Unresolved**: represented explicitly as `None` (rendered
//!   `undecided`), never silently defaulted.
//!
//! Identity conventions follow the existing research code: canonical
//! `|`-joined strings hashed with SHA-256 hex (cf. `corpus.rs`), stable
//! across machines, free of paths, ordering, timestamps, and timings.

#![forbid(unsafe_code)]

use std::fmt;

use crate::experiment::RESEARCH_V1_CONTRACT;

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

/// SHA-256 hex of a canonical string.
fn digest(canonical: &str) -> String {
    musicpack_core::format::checksum::sha256_hex(canonical.as_bytes())
}

/// Schema/version tag for the dataset manifest format itself.
pub const DATASET_SCHEMA_VERSION: &str = "sonic52-dataset-v1";

/// Version tag for the split-assignment rule.
pub const SPLIT_RULE_VERSION: &str = "split-v1";

// ---------------------------------------------------------------------
// provenance and licensing eligibility gate
// ---------------------------------------------------------------------

/// Licence/usage classification of one future training source. The
/// `license_id` / `reason` strings are caller-supplied metadata (e.g.
/// `"CC0-1.0"`); this type never invents licence facts, it only gates
/// on the classification Slice 7 must obtain for every sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseClass {
    /// Admissible, with the governing licence identifier recorded.
    Eligible {
        /// Caller-supplied licence identifier (mandatory, non-empty).
        license_id: String,
    },
    /// Inadmissible, with the reason recorded (never silently dropped).
    Excluded {
        /// Why this source may not train the model.
        reason: String,
    },
    /// Classification not yet established: treated as inadmissible
    /// until resolved (fail-closed default).
    Unknown,
}

/// Why a sample was refused admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// Source is classified excluded.
    Excluded {
        /// The recorded reason.
        reason: String,
    },
    /// Source classification is still unknown.
    UnknownLicense,
    /// Mandatory metadata (licence id) is missing.
    MissingLicenseId,
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateError::Excluded { reason } => write!(f, "source excluded: {reason}"),
            GateError::UnknownLicense => write!(f, "source licence unknown"),
            GateError::MissingLicenseId => write!(f, "eligible source lacks a licence id"),
        }
    }
}

impl std::error::Error for GateError {}

/// The minimum provenance gate: admits exactly the eligible-with-id
/// sources. Mandatory metadata before admission: a licence
/// classification and, for eligible sources, a non-empty licence id.
pub fn admit_source(license: &LicenseClass) -> Result<(), GateError> {
    match license {
        LicenseClass::Eligible { license_id } => {
            if license_id.is_empty() {
                Err(GateError::MissingLicenseId)
            } else {
                Ok(())
            }
        }
        LicenseClass::Excluded { reason } => Err(GateError::Excluded {
            reason: reason.clone(),
        }),
        LicenseClass::Unknown => Err(GateError::UnknownLicense),
    }
}

// ---------------------------------------------------------------------
// dataset splits and leakage rules
// ---------------------------------------------------------------------

/// Train/validation/test split assignment of one manifest row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// May train.
    Train,
    /// May validate / early-stop, never train.
    Validation,
    /// Held out; may only evaluate finished models.
    Test,
}

impl Split {
    /// Stable short name used in canonical strings.
    pub const fn as_str(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Validation => "validation",
            Split::Test => "test",
        }
    }
}

/// Split-assignment failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitError {
    /// Percentages do not sum to at most 100.
    BadRatios {
        /// Offered train percentage.
        train_pct: u8,
        /// Offered validation percentage.
        validation_pct: u8,
    },
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SplitError::BadRatios {
                train_pct,
                validation_pct,
            } => write!(
                f,
                "split ratios sum over 100: train {train_pct} + validation {validation_pct}"
            ),
        }
    }
}

impl std::error::Error for SplitError {}

/// Deterministic split assignment from `(rule version, dataset
/// version, track id)` hashed to one of 100 buckets: below `train_pct`
/// → Train, below `train_pct + validation_pct` → Validation, else
/// Test. Same inputs always assign the same split on every machine;
/// the dataset version participates so re-versioning re-deals openly
/// rather than silently inheriting an old split.
pub fn assign_split(
    dataset_version: &str,
    track_id: &str,
    train_pct: u8,
    validation_pct: u8,
) -> Result<Split, SplitError> {
    if u16::from(train_pct) + u16::from(validation_pct) > 100 {
        return Err(SplitError::BadRatios {
            train_pct,
            validation_pct,
        });
    }
    let canonical = format!("{SPLIT_RULE_VERSION}|{dataset_version}|{track_id}");
    let digest = musicpack_core::format::checksum::sha256_hex(canonical.as_bytes());
    let byte = u8::from_str_radix(&digest[..2], 16).expect("hex of digest") as u32;
    let bucket = byte * 100 / 256;
    if bucket < u32::from(train_pct) {
        Ok(Split::Train)
    } else if bucket < u32::from(train_pct) + u32::from(validation_pct) {
        Ok(Split::Validation)
    } else {
        Ok(Split::Test)
    }
}

// ---------------------------------------------------------------------
// versioned training-dataset manifest (metadata + identity only)
// ---------------------------------------------------------------------

/// One future training-corpus row: metadata and identity, never audio.
/// `exclusion` records why an ingested row is held out (failed decode,
/// failed gate, …) instead of deleting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTrack {
    /// Stable source-track identifier (universe: one dataset version).
    pub track_id: String,
    /// Source/provenance identifier (mandatory, non-empty).
    pub provenance: String,
    /// Licence/usage classification (gated by [`admit_source`]).
    pub license: LicenseClass,
    /// Lowercase container extension.
    pub format: String,
    /// Source sample rate in Hz.
    pub sample_rate: u32,
    /// Source channel count.
    pub channels: u8,
    /// Decoded source frames.
    pub decoded_frames: usize,
    /// Mono frames at the contract rate entering the frontend.
    pub processed_frames: usize,
    /// Mel frames out of the frontend.
    pub mel_frames: usize,
    /// laid-out patch count under the contract.
    pub patch_count: usize,
    /// Split assignment.
    pub split: Split,
    /// Exclusion/failure reason where applicable (`None` = admitted).
    pub exclusion: Option<String>,
}

/// Manifest construction failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// Empty dataset identity or version.
    EmptyIdentity,
    /// No rows at all.
    EmptyManifest,
    /// Two rows share one track id (also the cross-split leakage trip).
    DuplicateTrack {
        /// The repeated identifier.
        track_id: String,
    },
    /// Mandatory per-row metadata missing.
    MissingMetadata {
        /// The offending track id (possibly empty itself).
        track_id: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::EmptyIdentity => write!(f, "dataset id/version must not be empty"),
            ManifestError::EmptyManifest => write!(f, "manifest has no rows"),
            ManifestError::DuplicateTrack { track_id } => {
                write!(f, "duplicate manifest track id: {track_id}")
            }
            ManifestError::MissingMetadata { track_id } => {
                write!(f, "manifest row lacks mandatory metadata: {track_id}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// Deterministic manifest for one dataset version: metadata and
/// identity only, rows sorted by track id. Separate identity levels
/// stay separate: source identity (`track_id` + `provenance`), dataset
/// identity (`dataset_id` + `dataset_version`), preprocessing identity
/// (`preprocessing_id`, e.g. the research-v1 contract text), sample
/// identity ([`training_sample_id`]), model identity
/// ([`FutureModelContract`]), weight identity (weight digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetManifest {
    /// Dataset identity (stable name).
    pub dataset_id: String,
    /// Dataset version (any split/content change bumps this).
    pub dataset_version: String,
    /// Preprocessing contract text the rows were produced under.
    pub preprocessing_id: String,
    /// Rows in track-id order.
    pub rows: Vec<ManifestTrack>,
}

impl DatasetManifest {
    /// Builds and validates a manifest: non-empty identity, at least
    /// one row, mandatory per-row metadata, no duplicate track ids
    /// (duplicates would also be cross-split leakage, so they fail
    /// closed here). Rows are stored sorted by track id regardless of
    /// insertion order.
    pub fn build(
        dataset_id: String,
        dataset_version: String,
        preprocessing_id: String,
        mut rows: Vec<ManifestTrack>,
    ) -> Result<Self, ManifestError> {
        if dataset_id.is_empty() || dataset_version.is_empty() {
            return Err(ManifestError::EmptyIdentity);
        }
        if rows.is_empty() {
            return Err(ManifestError::EmptyManifest);
        }
        for row in &rows {
            if row.track_id.is_empty() || row.provenance.is_empty() {
                return Err(ManifestError::MissingMetadata {
                    track_id: row.track_id.clone(),
                });
            }
        }
        rows.sort_by(|a, b| a.track_id.cmp(&b.track_id));
        for pair in rows.windows(2) {
            if pair[0].track_id == pair[1].track_id {
                return Err(ManifestError::DuplicateTrack {
                    track_id: pair[0].track_id.clone(),
                });
            }
        }
        Ok(Self {
            dataset_id,
            dataset_version,
            preprocessing_id,
            rows,
        })
    }

    /// Track ids assigned to one split, in order.
    pub fn split_members(&self, split: Split) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|row| row.split == split)
            .map(|row| row.track_id.as_str())
            .collect()
    }

    /// Canonical deterministic serialization: header plus one fixed-
    /// order line per row. No paths, no ordering, no timestamps, no
    /// timings — only identity and metadata.
    pub fn canonical(&self) -> String {
        let mut out = format!(
            "{}|{}|{}|{}",
            DATASET_SCHEMA_VERSION, self.dataset_id, self.dataset_version, self.preprocessing_id
        );
        for row in &self.rows {
            out.push_str(&format!(
                "\n{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                row.track_id,
                row.provenance,
                match &row.license {
                    LicenseClass::Eligible { license_id } => format!("eligible:{license_id}"),
                    LicenseClass::Excluded { reason } => format!("excluded:{reason}"),
                    LicenseClass::Unknown => "unknown".to_string(),
                },
                row.format,
                row.sample_rate,
                row.channels,
                row.decoded_frames,
                row.processed_frames,
                row.mel_frames,
                row.patch_count,
                row.split.as_str(),
                row.exclusion.as_deref().unwrap_or("-")
            ));
        }
        out
    }

    /// Deterministic manifest digest.
    pub fn digest(&self) -> String {
        digest(&self.canonical())
    }
}

/// Cross-manifest leakage check: track ids present in both manifests
/// (e.g. a training manifest and an evaluation manifest) must be
/// empty. Source-track identity is the primary leakage boundary: one
/// track, one split, every manifest.
///
/// Known limitation, stated plainly: track-level separation does not
/// prevent subtler relationships between recordings (same artist or
/// album across tracks, versions, remixes, related material). No
/// sophisticated heuristic is invented here; the minimum contract is
/// exact track-identity disjointness.
pub fn detect_cross_manifest_leakage(a: &DatasetManifest, b: &DatasetManifest) -> Vec<String> {
    let mut leaked = Vec::new();
    for row in &a.rows {
        if b.rows.iter().any(|other| other.track_id == row.track_id) {
            leaked.push(row.track_id.clone());
        }
    }
    leaked.sort();
    leaked
}

// ---------------------------------------------------------------------
// deterministic training-sample identity
// ---------------------------------------------------------------------

/// Stable identity of one future training sample, derived from
/// dataset identity + source-track identity + preprocessing contract +
/// patch index. Stable across machines; independent of absolute paths
/// (no path participates), filesystem order (no ordering participates),
/// and execution timing. The dataset version participates, so the same
/// track reprocessed under a new dataset version is unambiguously a
/// different sample — the property leak detection relies on.
pub fn training_sample_id(
    dataset_id: &str,
    dataset_version: &str,
    track_id: &str,
    preprocessing_id: &str,
    patch_index: u32,
) -> String {
    digest(&format!(
        "sample-v1|{dataset_id}|{dataset_version}|{track_id}|{preprocessing_id}|{patch_index}"
    ))
}

// ---------------------------------------------------------------------
// future model contract (decided fields + explicit UNDECIDED)
// ---------------------------------------------------------------------

/// Research contract for a future learned Sonic52-derived model: what
/// it exposes, with every unknown rendered as `None` (`undecided` in
/// canonical form) rather than defaulted.
///
/// Provenance per field: output dimension/activation and the hidden
/// width/activation are S1-reported structural characteristics;
/// aggregation follows the research-v1 choice; everything else is
/// UNDECIDED. The Slice 4 reference network keeps its concrete
/// placeholder implementation for plumbing tests — this contract
/// describes the future trained model, never the reference network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FutureModelContract {
    /// Stable model name (e.g. `"sonic52-52"`).
    pub model_id: String,
    /// Model iteration (any weight/architecture change bumps this).
    pub model_version: String,
    /// Preprocessing contract text (e.g. the research-v1 text).
    pub preprocessing_id: String,
    /// Input tensor shape as (height, width, channels).
    pub input_shape: Option<(usize, usize, usize)>,
    /// Input dtype (e.g. `"f32le"`).
    pub input_dtype: Option<String>,
    /// Tensor layout (e.g. `"[C,H,W]-row-major"`).
    pub tensor_layout: Option<String>,
    /// Convolution configuration identity (UNDECIDED: the reference
    /// placeholder is not promoted to a training decision).
    pub conv_config: Option<String>,
    /// Hidden dense width.
    pub hidden_dim: Option<usize>,
    /// Hidden activation.
    pub hidden_activation: Option<String>,
    /// Output width.
    pub output_dim: Option<usize>,
    /// Output activation.
    pub output_activation: Option<String>,
    /// Patch aggregation rule.
    pub aggregation: Option<String>,
    /// Embedding normalization policy.
    pub normalization: Option<String>,
    /// Weight-set digest once weights exist (always `None` in Slice 6:
    /// there are no learned weights yet).
    pub weight_digest: Option<String>,
}

fn undecided(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "undecided".to_string())
}

/// The research-v1 proposal: decided exactly where evidence or prior
/// research choices reach, UNDECIDED everywhere else.
pub fn research_v1_model_proposal() -> FutureModelContract {
    FutureModelContract {
        model_id: "sonic52-52".to_string(),
        model_version: "0".to_string(),
        preprocessing_id: RESEARCH_V1_CONTRACT.to_string(),
        input_shape: Some((187, 96, 1)),
        input_dtype: Some("f32le".to_string()),
        tensor_layout: Some("[C,H,W]-row-major".to_string()),
        conv_config: None,
        hidden_dim: Some(200),
        hidden_activation: Some("relu".to_string()),
        output_dim: Some(52),
        output_activation: Some("sigmoid".to_string()),
        aggregation: Some("mean".to_string()),
        normalization: None,
        weight_digest: None,
    }
}

impl FutureModelContract {
    /// True only when no field is UNDECIDED (false for the v1 proposal
    /// by design — Slice 6 freezes the questions, not the answers).
    pub fn is_complete(&self) -> bool {
        self.input_shape.is_some()
            && self.input_dtype.is_some()
            && self.tensor_layout.is_some()
            && self.conv_config.is_some()
            && self.hidden_dim.is_some()
            && self.hidden_activation.is_some()
            && self.output_dim.is_some()
            && self.output_activation.is_some()
            && self.aggregation.is_some()
            && self.normalization.is_some()
            && self.weight_digest.is_some()
    }

    /// Canonical deterministic serialization with explicit `undecided`
    /// tokens for every open field.
    pub fn canonical(&self) -> String {
        let (shape, dtype, layout, conv) = (
            self.input_shape
                .map(|(h, w, c)| format!("{h}x{w}x{c}"))
                .unwrap_or_else(|| "undecided".to_string()),
            undecided(&self.input_dtype),
            undecided(&self.tensor_layout),
            undecided(&self.conv_config),
        );
        let dims = (
            self.hidden_dim
                .map(|d| d.to_string())
                .unwrap_or_else(|| "undecided".to_string()),
            self.output_dim
                .map(|d| d.to_string())
                .unwrap_or_else(|| "undecided".to_string()),
        );
        format!(
            "model-v1|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.model_id,
            self.model_version,
            self.preprocessing_id,
            shape,
            dtype,
            layout,
            conv,
            dims.0,
            undecided(&self.hidden_activation),
            dims.1,
            undecided(&self.output_activation),
            undecided(&self.aggregation),
            undecided(&self.normalization),
            undecided(&self.weight_digest)
        )
    }

    /// Deterministic model identity.
    pub fn digest(&self) -> String {
        digest(&self.canonical())
    }
}

// ---------------------------------------------------------------------
// future weight-format contract (learned weights, none exist yet)
// ---------------------------------------------------------------------

/// One named tensor in a future learned-weight file.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightTensor {
    /// Tensor name (e.g. `"conv/weights"`); unique within a file.
    pub name: String,
    /// Element counts per axis (row-major flattening implied).
    pub shape: Vec<usize>,
    /// Element dtype tag (only `"f32le"` exists in v1).
    pub dtype: String,
    /// Row-major little-endian-decoded values.
    pub data: Vec<f32>,
}

/// Weight-file construction failures. Malformed input fails closed:
/// no partial files, no lenient parsing, no NaN smuggling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeightError {
    /// No tensors at all.
    Empty,
    /// Empty tensor name.
    EmptyName,
    /// Two tensors share one name.
    DuplicateTensor {
        /// The repeated name.
        name: String,
    },
    /// Data length does not equal the shape product.
    ShapeMismatch {
        /// Tensor name.
        name: String,
        /// Shape product.
        expected: usize,
        /// Data length.
        found: usize,
    },
    /// A non-finite weight value.
    NonFinite {
        /// Tensor name.
        name: String,
    },
    /// Unsupported dtype tag.
    BadDtype {
        /// Tensor name.
        name: String,
        /// Offered tag.
        found: String,
    },
}

impl fmt::Display for WeightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WeightError::Empty => write!(f, "weight file has no tensors"),
            WeightError::EmptyName => write!(f, "weight tensor has no name"),
            WeightError::DuplicateTensor { name } => {
                write!(f, "duplicate weight tensor: {name}")
            }
            WeightError::ShapeMismatch {
                name,
                expected,
                found,
            } => write!(
                f,
                "tensor {name} has {found} values for shape product {expected}"
            ),
            WeightError::NonFinite { name } => {
                write!(f, "tensor {name} has a non-finite value")
            }
            WeightError::BadDtype { name, found } => {
                write!(f, "tensor {name} has unsupported dtype {found}")
            }
        }
    }
}

impl std::error::Error for WeightError {}

/// Serialization version of the weight-file format itself.
pub const WEIGHT_FORMAT_VERSION: &str = "sonic52-weights-v1";

/// Future learned-weight file: deterministic, auditable, versioned,
/// independently hashable. Slice 6 creates no learned weights; this
/// type defines what Slice 7+ must produce and accept. The digest
/// convention (SHA-256 over canonical little-endian bytes) is shared
/// with the existing reference-weight digest machinery.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightFile {
    /// Model identity this file belongs to.
    pub model_id: String,
    /// Architecture identity (topology fingerprint when defined).
    pub arch_id: String,
    /// Preprocessing contract text the weights were trained under.
    pub preprocessing_id: String,
    /// Named tensors in file order.
    pub tensors: Vec<WeightTensor>,
}

impl WeightFile {
    /// Builds and validates a weight file: non-empty tensor list,
    /// non-empty unique names, `f32le` dtype, shape-product data
    /// lengths, all values finite.
    pub fn build(
        model_id: String,
        arch_id: String,
        preprocessing_id: String,
        tensors: Vec<WeightTensor>,
    ) -> Result<Self, WeightError> {
        if tensors.is_empty() {
            return Err(WeightError::Empty);
        }
        for tensor in &tensors {
            if tensor.name.is_empty() {
                return Err(WeightError::EmptyName);
            }
            if tensor.dtype != "f32le" {
                return Err(WeightError::BadDtype {
                    name: tensor.name.clone(),
                    found: tensor.dtype.clone(),
                });
            }
            let mut product: u128 = 1;
            for dim in &tensor.shape {
                product = product.saturating_mul(*dim as u128);
            }
            if product != tensor.data.len() as u128 {
                return Err(WeightError::ShapeMismatch {
                    name: tensor.name.clone(),
                    expected: product.min(usize::MAX as u128) as usize,
                    found: tensor.data.len(),
                });
            }
            if tensor.data.iter().any(|v| !v.is_finite()) {
                return Err(WeightError::NonFinite {
                    name: tensor.name.clone(),
                });
            }
        }
        let mut names: Vec<&str> = tensors.iter().map(|tensor| tensor.name.as_str()).collect();
        names.sort_unstable();
        for pair in names.windows(2) {
            if pair[0] == pair[1] {
                return Err(WeightError::DuplicateTensor {
                    name: pair[0].to_string(),
                });
            }
        }
        Ok(Self {
            model_id,
            arch_id,
            preprocessing_id,
            tensors,
        })
    }

    /// Canonical deterministic serialization: header plus one section
    /// per tensor in file order (names unique, order significant).
    pub fn canonical(&self) -> String {
        let mut out = format!(
            "{}|{}|{}|{}",
            WEIGHT_FORMAT_VERSION, self.model_id, self.arch_id, self.preprocessing_id
        );
        for tensor in &self.tensors {
            let shape = tensor
                .shape
                .iter()
                .map(|dim| dim.to_string())
                .collect::<Vec<_>>()
                .join("x");
            out.push_str(&format!("\n{}|{}|{}", tensor.name, shape, tensor.dtype));
        }
        out
    }

    /// Deterministic weight digest: canonical text plus every tensor's
    /// little-endian bytes. Changing one weight bit changes the digest.
    pub fn digest(&self) -> String {
        let mut bytes = self.canonical().into_bytes();
        bytes.push(0);
        for tensor in &self.tensors {
            for value in &tensor.data {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        musicpack_core::format::checksum::sha256_hex(&bytes)
    }
}

// ---------------------------------------------------------------------
// minimum loss-computation oracle (gradient-free)
// ---------------------------------------------------------------------

/// Loss-oracle failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LossError {
    /// No logits at all.
    Empty,
    /// Logit/target count mismatch.
    DimMismatch {
        /// Logit count.
        expected: usize,
        /// Target count.
        found: usize,
    },
    /// A non-finite logit.
    NonFiniteLogits,
    /// A non-finite target.
    NonFiniteTargets,
}

impl fmt::Display for LossError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LossError::Empty => write!(f, "no logits to score"),
            LossError::DimMismatch { expected, found } => {
                write!(f, "{found} targets for {expected} logits")
            }
            LossError::NonFiniteLogits => write!(f, "non-finite logit"),
            LossError::NonFiniteTargets => write!(f, "non-finite target"),
        }
    }
}

impl std::error::Error for LossError {}

/// Binary cross-entropy with logits, averaged over outputs:
///
/// ```text
/// mean_i( max(l_i, 0) − l_i · t_i + ln(1 + exp(−|l_i|)) )
/// ```
///
/// `f64` accumulation, `f32` result; fixed evaluation order; finite
/// scalar for valid inputs. This oracle assumes the stated
/// formulation — multi-label sigmoid outputs trained against target
/// probabilities (the lineage training loss, FORENSICS.md S9). That
/// assumption is load-bearing: if Slice 7 adopts single-label softmax
/// outputs instead, this oracle is superseded, not reused. No
/// gradients, no optimizer, no batching, no epochs — scoring only.
pub fn bce_with_logits(logits: &[f32], targets: &[f32]) -> Result<f32, LossError> {
    if logits.is_empty() {
        return Err(LossError::Empty);
    }
    if logits.len() != targets.len() {
        return Err(LossError::DimMismatch {
            expected: logits.len(),
            found: targets.len(),
        });
    }
    if logits.iter().any(|v| !v.is_finite()) {
        return Err(LossError::NonFiniteLogits);
    }
    if targets.iter().any(|v| !v.is_finite()) {
        return Err(LossError::NonFiniteTargets);
    }
    let mut total = 0.0f64;
    for (logit, target) in logits.iter().zip(targets.iter()) {
        let l = f64::from(*logit);
        let t = f64::from(*target);
        total += l.max(0.0) - l * t + (1.0 + (-l.abs()).exp()).ln();
    }
    Ok((total / logits.len() as f64) as f32)
}

// ---------------------------------------------------------------------
// future evaluation contract (specified before training exists)
// ---------------------------------------------------------------------

/// Evaluation protocol identity (pre-registered, versioned).
pub const EVALUATION_PROTOCOL_ID: &str = "sonic52-eval-v1";

/// Reference baseline profile ids (naming only — the research
/// implementation never invokes Discogs-EffNet; comparison becomes
/// meaningful only with trained weights).
pub const EVAL_BASELINE_PROFILES: &[&str] = &[
    "musicpack-similarity-discogs-effnet-multi-v1",
    "musicpack-similarity-discogs-effnet-release-v1",
];

/// Future dimensionality arms the contract stays open for, with no
/// superiority claim attached to any of them.
pub const EVAL_DIMENSIONALITY_ARMS: &[&str] = &["sonic52-52", "sonic52-128"];

/// The conceptual embedding-evaluation pipeline, fixed before training
/// exists: track → patches → patch embeddings → aggregation → track
/// embedding → nearest neighbours.
pub fn evaluation_pipeline() -> [&'static str; 6] {
    [
        "track",
        "patches",
        "patch-embeddings",
        "aggregation",
        "track-embedding",
        "nearest-neighbours",
    ]
}

/// Future human-relevance procedure (procedure text, not execution):
/// blind paired listening over seed tracks, candidate pools drawn from
/// model and baseline nearest neighbours, rater instructions fixed in
/// the Slice 7 evaluation plan, inter-rater agreement reported before
/// any quality claim. No listening is performed in Slice 6.
pub fn human_eval_procedure() -> &'static str {
    "blind paired listening: raters compare seed-track neighbour sets from the model under test against baseline neighbour sets, without model labels; instructions and the agreement metric are fixed before rating begins; no human evaluation is performed in Slice 6"
}

/// Held-out disjointness check over source-track ids: evaluation ids
/// must share no track with training ids. Returns the leaked ids
/// (empty means clean).
pub fn eval_leakage(training_ids: &[String], eval_ids: &[String]) -> Vec<String> {
    let mut leaked = Vec::new();
    for id in eval_ids {
        if training_ids.iter().any(|train| train == id) && !leaked.contains(id) {
            leaked.push(id.clone());
        }
    }
    leaked.sort();
    leaked
}

// ---------------------------------------------------------------------
// explicit Slice 7 boundary
// ---------------------------------------------------------------------

/// What Slice 7 may eventually implement. Open-ended by necessity
/// (training design), bounded by the MUST_NOT list below.
pub const SLICE7_MAY: &[&str] = &[
    "approved dataset ingestion",
    "deterministic sample generation",
    "model parameterization",
    "forward pass",
    "loss (per the Slice 6 oracle formulation)",
    "gradients/backpropagation",
    "optimizer",
    "training/evaluation loops",
    "checkpoints",
    "learned-weight serialization (per the Slice 6 weight contract)",
];

/// What Slice 7 must not silently redefine: any change here needs an
/// explicit versioned contract change, not a quiet trainer edit.
pub const SLICE7_MUST_NOT_REDEFINE: &[&str] = &[
    "dataset identity",
    "preprocessing identity",
    "sample identity",
    "model identity",
    "split semantics",
    "weight serialization",
    "evaluation semantics",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{reference_network, REFERENCE_SEED};

    fn manifest_row(track_id: &str, split: Split) -> ManifestTrack {
        ManifestTrack {
            track_id: track_id.to_string(),
            provenance: "fixture-bank".to_string(),
            license: LicenseClass::Eligible {
                license_id: "CC0-1.0".to_string(),
            },
            format: "wav".to_string(),
            sample_rate: 16_000,
            channels: 1,
            decoded_frames: 48_000,
            processed_frames: 48_000,
            mel_frames: 188,
            patch_count: 1,
            split,
            exclusion: None,
        }
    }

    fn two_track_manifest() -> DatasetManifest {
        DatasetManifest::build(
            "test-bank".to_string(),
            "v3".to_string(),
            RESEARCH_V1_CONTRACT.to_string(),
            vec![
                manifest_row("b-track", Split::Train),
                manifest_row("a-track", Split::Test),
            ],
        )
        .expect("valid fixture manifest")
    }

    // -- provenance gate ----------------------------------------------------

    #[test]
    fn provenance_gate_admits_only_eligible_with_id() {
        assert!(admit_source(&LicenseClass::Eligible {
            license_id: "CC0-1.0".to_string()
        })
        .is_ok());
        assert_eq!(
            admit_source(&LicenseClass::Eligible {
                license_id: String::new()
            })
            .unwrap_err(),
            GateError::MissingLicenseId
        );
        assert_eq!(
            admit_source(&LicenseClass::Excluded {
                reason: "NC weights".to_string()
            })
            .unwrap_err(),
            GateError::Excluded {
                reason: "NC weights".to_string()
            }
        );
        assert_eq!(
            admit_source(&LicenseClass::Unknown).unwrap_err(),
            GateError::UnknownLicense
        );
    }

    // -- splits ---------------------------------------------------------------

    #[test]
    fn split_assignment_is_deterministic_and_versioned() {
        let first = assign_split("v3", "a-track", 70, 15).unwrap();
        assert_eq!(first, assign_split("v3", "a-track", 70, 15).unwrap());
        assert_eq!(assign_split("v3", "a-track", 100, 0).unwrap(), Split::Train);
        assert_eq!(assign_split("v3", "a-track", 0, 0).unwrap(), Split::Test);
        assert_eq!(
            assign_split("v3", "a-track", 70, 31).unwrap_err(),
            SplitError::BadRatios {
                train_pct: 70,
                validation_pct: 31
            }
        );
        // The dataset version participates: re-versioning re-deals.
        // (Pinned observation over four tracks, deterministic forever —
        // not a theorem that every single track moves.)
        let tracks = ["a-track", "b-track", "c-track", "d-track"];
        let v3: Vec<Split> = tracks
            .iter()
            .map(|track| assign_split("v3", track, 70, 15).unwrap())
            .collect();
        let v4: Vec<Split> = tracks
            .iter()
            .map(|track| assign_split("v4", track, 70, 15).unwrap())
            .collect();
        assert_ne!(v3, v4);
    }

    // -- manifest ---------------------------------------------------------------

    #[test]
    fn manifest_orders_rejects_and_digests() {
        let manifest = two_track_manifest();
        // Stable ordering regardless of insertion order.
        assert_eq!(manifest.rows[0].track_id, "a-track");
        assert_eq!(manifest.rows[1].track_id, "b-track");
        assert_eq!(manifest.split_members(Split::Train), vec!["b-track"]);
        assert_eq!(manifest.split_members(Split::Test), vec!["a-track"]);
        assert!(manifest.split_members(Split::Validation).is_empty());
        // Deterministic digest, stable across rebuilds.
        assert_eq!(manifest.digest(), two_track_manifest().digest());
        assert_eq!(manifest.digest().len(), 64);
        // Failures: empty identity, empty rows, duplicates, metadata.
        assert_eq!(
            DatasetManifest::build(
                String::new(),
                "v3".to_string(),
                "p".to_string(),
                vec![manifest_row("a", Split::Train)]
            )
            .unwrap_err(),
            ManifestError::EmptyIdentity
        );
        assert_eq!(
            DatasetManifest::build("d".to_string(), "v3".to_string(), "p".to_string(), vec![])
                .unwrap_err(),
            ManifestError::EmptyManifest
        );
        assert_eq!(
            DatasetManifest::build(
                "d".to_string(),
                "v3".to_string(),
                "p".to_string(),
                vec![
                    manifest_row("a", Split::Train),
                    manifest_row("a", Split::Test)
                ]
            )
            .unwrap_err(),
            ManifestError::DuplicateTrack {
                track_id: "a".to_string()
            }
        );
        let mut bad = manifest_row("a", Split::Train);
        bad.provenance.clear();
        assert_eq!(
            DatasetManifest::build(
                "d".to_string(),
                "v3".to_string(),
                "p".to_string(),
                vec![bad]
            )
            .unwrap_err(),
            ManifestError::MissingMetadata {
                track_id: "a".to_string()
            }
        );
    }

    #[test]
    fn cross_manifest_leakage_is_detected() {
        let train = two_track_manifest();
        let mut eval_rows = vec![manifest_row("c-track", Split::Test)];
        eval_rows.push(manifest_row("a-track", Split::Test));
        let eval = DatasetManifest::build(
            "test-bank".to_string(),
            "v3".to_string(),
            RESEARCH_V1_CONTRACT.to_string(),
            eval_rows,
        )
        .unwrap();
        assert_eq!(
            detect_cross_manifest_leakage(&train, &eval),
            vec!["a-track".to_string()]
        );
        let clean = DatasetManifest::build(
            "test-bank".to_string(),
            "v3".to_string(),
            RESEARCH_V1_CONTRACT.to_string(),
            vec![manifest_row("z-track", Split::Test)],
        )
        .unwrap();
        assert!(detect_cross_manifest_leakage(&train, &clean).is_empty());
    }

    // -- sample identity ------------------------------------------------------------

    #[test]
    fn sample_identity_separates_every_axis() {
        let base = training_sample_id("bank", "v3", "track-a", "preproc-v1", 7);
        assert_eq!(
            base,
            training_sample_id("bank", "v3", "track-a", "preproc-v1", 7)
        );
        assert_eq!(base.len(), 64);
        // Different patch, preprocessing, dataset version, dataset, track.
        assert_ne!(
            base,
            training_sample_id("bank", "v3", "track-a", "preproc-v1", 8)
        );
        assert_ne!(
            base,
            training_sample_id("bank", "v3", "track-a", "preproc-v2", 7)
        );
        assert_ne!(
            base,
            training_sample_id("bank", "v4", "track-a", "preproc-v1", 7)
        );
        assert_ne!(
            base,
            training_sample_id("other", "v3", "track-a", "preproc-v1", 7)
        );
        assert_ne!(
            base,
            training_sample_id("bank", "v3", "track-b", "preproc-v1", 7)
        );
        // No path participates: identical metadata alongside different
        // absolute locations still identifies one sample.
        let _locations = ["/mnt/a/song.wav", "/data/b/song.wav"];
        assert_eq!(
            base,
            training_sample_id("bank", "v3", "track-a", "preproc-v1", 7)
        );
    }

    // -- model contract -----------------------------------------------------------

    #[test]
    fn model_proposal_freezes_decided_and_undecided() {
        let proposal = research_v1_model_proposal();
        assert_eq!(proposal.model_id, "sonic52-52");
        assert_eq!(proposal.output_dim, Some(52));
        assert_eq!(proposal.output_activation.as_deref(), Some("sigmoid"));
        assert_eq!(proposal.hidden_dim, Some(200));
        // UNDECIDED stays UNDECIDED: conv, normalization, weight digest.
        assert_eq!(proposal.conv_config, None);
        assert_eq!(proposal.normalization, None);
        assert_eq!(proposal.weight_digest, None);
        assert!(!proposal.is_complete());
        assert!(proposal.canonical().contains("undecided"));
        // Deterministic identity; one changed field changes it.
        assert_eq!(proposal.digest(), research_v1_model_proposal().digest());
        let mut other = proposal.clone();
        other.model_version = "1".to_string();
        assert_ne!(proposal.digest(), other.digest());
    }

    #[test]
    fn model_proposal_is_consistent_with_reference_topology() {
        // Consistency, not identity: the reference network's concrete
        // dims agree with the proposal's decided fields, while the
        // placeholder conv is NOT promoted into the contract.
        let network = reference_network(REFERENCE_SEED);
        let proposal = research_v1_model_proposal();
        assert_eq!(Some(network.dense_hidden.output_dim), proposal.hidden_dim);
        assert_eq!(Some(network.dense_output.output_dim), proposal.output_dim);
        assert_eq!(proposal.conv_config, None);
    }

    // -- weight contract ------------------------------------------------------------

    fn weight_file_fixture() -> WeightFile {
        WeightFile::build(
            "sonic52-52".to_string(),
            "arch-fingerprint".to_string(),
            RESEARCH_V1_CONTRACT.to_string(),
            vec![
                WeightTensor {
                    name: "dense/weights".to_string(),
                    shape: vec![2, 3],
                    dtype: "f32le".to_string(),
                    data: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
                },
                WeightTensor {
                    name: "dense/bias".to_string(),
                    shape: vec![2],
                    dtype: "f32le".to_string(),
                    data: vec![0.5, -0.5],
                },
            ],
        )
        .expect("valid fixture weight file")
    }

    #[test]
    fn weight_file_validates_and_digests() {
        let file = weight_file_fixture();
        assert_eq!(file.digest(), weight_file_fixture().digest());
        assert_eq!(file.digest().len(), 64);
        assert!(file
            .canonical()
            .starts_with("sonic52-weights-v1|sonic52-52|"));
        // One changed weight bit changes the digest.
        let mut other = weight_file_fixture();
        other.tensors[0].data[0] = 1.5;
        assert_ne!(file.digest(), other.digest());
        // Failures: empty, empty name, duplicates, shape, NaN, dtype.
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), vec![])
                .unwrap_err(),
            WeightError::Empty
        );
        let mut nameless = weight_file_fixture().tensors;
        nameless[0].name.clear();
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), nameless)
                .unwrap_err(),
            WeightError::EmptyName
        );
        let mut dupes = weight_file_fixture().tensors;
        dupes[1].name = dupes[0].name.clone();
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), dupes)
                .unwrap_err(),
            WeightError::DuplicateTensor {
                name: "dense/weights".to_string()
            }
        );
        let mut misshapen = weight_file_fixture().tensors;
        misshapen[0].data.pop();
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), misshapen)
                .unwrap_err(),
            WeightError::ShapeMismatch {
                name: "dense/weights".to_string(),
                expected: 6,
                found: 5
            }
        );
        let mut poisoned = weight_file_fixture().tensors;
        poisoned[1].data[0] = f32::NAN;
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), poisoned)
                .unwrap_err(),
            WeightError::NonFinite {
                name: "dense/bias".to_string()
            }
        );
        let mut retyped = weight_file_fixture().tensors;
        retyped[0].dtype = "f16le".to_string();
        assert_eq!(
            WeightFile::build("m".to_string(), "a".to_string(), "p".to_string(), retyped)
                .unwrap_err(),
            WeightError::BadDtype {
                name: "dense/weights".to_string(),
                found: "f16le".to_string()
            }
        );
    }

    // -- loss oracle ----------------------------------------------------------

    #[test]
    fn loss_oracle_golden_and_deterministic() {
        // Hand golden: logit 0 against target 1 is ln(2).
        let loss = bce_with_logits(&[0.0], &[1.0]).unwrap();
        assert!((loss - std::f32::consts::LN_2).abs() < 1e-6, "{loss}");
        // Two-way golden: mean over outputs, still ln(2).
        let loss = bce_with_logits(&[0.0, 0.0], &[1.0, 0.0]).unwrap();
        assert!((loss - std::f32::consts::LN_2).abs() < 1e-6, "{loss}");
        // Confident-correct scores below chance-level.
        let loss = bce_with_logits(&[5.0, -5.0], &[1.0, 0.0]).unwrap();
        assert!(loss < 0.01, "{loss}");
        assert!(loss.is_finite());
        // Deterministic across calls.
        let vectors = vec![0.7f32, -1.3, 2.2];
        let targets = vec![1.0f32, 0.0, 1.0];
        assert_eq!(
            bce_with_logits(&vectors, &targets).unwrap(),
            bce_with_logits(&vectors, &targets).unwrap()
        );
        // Failures: empty, dims, NaN logits, NaN targets.
        assert_eq!(bce_with_logits(&[], &[]).unwrap_err(), LossError::Empty);
        assert_eq!(
            bce_with_logits(&[0.0, 1.0], &[1.0]).unwrap_err(),
            LossError::DimMismatch {
                expected: 2,
                found: 1
            }
        );
        assert_eq!(
            bce_with_logits(&[f32::NAN], &[1.0]).unwrap_err(),
            LossError::NonFiniteLogits
        );
        assert_eq!(
            bce_with_logits(&[0.0], &[f32::INFINITY]).unwrap_err(),
            LossError::NonFiniteTargets
        );
    }

    // -- evaluation contract ----------------------------------------------------

    #[test]
    fn evaluation_contract_is_pinned() {
        assert_eq!(EVALUATION_PROTOCOL_ID, "sonic52-eval-v1");
        assert_eq!(
            evaluation_pipeline(),
            [
                "track",
                "patches",
                "patch-embeddings",
                "aggregation",
                "track-embedding",
                "nearest-neighbours"
            ]
        );
        assert!(human_eval_procedure().contains("blind paired listening"));
        assert!(human_eval_procedure().contains("Slice 6"));
        assert_eq!(EVAL_BASELINE_PROFILES.len(), 2);
        assert_eq!(EVAL_DIMENSIONALITY_ARMS, &["sonic52-52", "sonic52-128"]);
        // Held-out disjointness validator.
        let train = ["a".to_string(), "b".to_string()];
        let clean = ["c".to_string()];
        let leaked = ["b".to_string(), "c".to_string()];
        assert!(eval_leakage(&train, &clean).is_empty());
        assert_eq!(eval_leakage(&train, &leaked), vec!["b".to_string()]);
    }

    // -- slice 7 boundary --------------------------------------------------------

    #[test]
    fn slice7_boundary_lists_are_explicit() {
        assert!(SLICE7_MAY.contains(&"gradients/backpropagation"));
        assert!(SLICE7_MAY.contains(&"training/evaluation loops"));
        for guarded in [
            "dataset identity",
            "preprocessing identity",
            "sample identity",
            "model identity",
            "split semantics",
            "weight serialization",
            "evaluation semantics",
        ] {
            assert!(SLICE7_MUST_NOT_REDEFINE.contains(&guarded), "{guarded}");
        }
    }
}
