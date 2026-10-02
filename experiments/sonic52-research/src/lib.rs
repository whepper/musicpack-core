//! Sonic52 research scaffold: the I/O contract for an **independently
//! implemented** similarity-model research track.
//!
//! > Sonic52 is an independent research model inspired by publicly
//! > reported/reverse-engineered structural characteristics of Plex's
//! > historical Sonic Analysis model. It is not "the Plex model", not Plex
//! > Sonic Analysis, and not a Plex reimplementation. No Plex model weights
//! > are used, vendored, downloaded, or redistributed here.
//!
//! # Research-only boundary
//!
//! This crate contains **no weights, no training, no inference, no audio
//! decoding, and no production wiring**. It defines:
//!
//! - the provisional research profile identity
//!   ([`SONIC52_PROFILE_ID`]);
//! - the embedding contract: exactly 52 `f32` values with an intended
//!   sigmoid output range ([`Sonic52Embedding`]);
//! - the explicit preprocessing/input-tensor contract, in which every
//!   unknown is labelled as unknown ([`PREPROCESSING_TABLE`],
//!   [`Sonic52InputContract`]);
//! - the future dimensionality-experiment ladder ([`DIMENSIONALITY_LADDER`]).
//!
//! It must never gain a dependency on a production crate, a model runtime,
//! or a model artifact. The one deliberate reuse relationship goes the
//! other way and is documentary only: a future 52-D `f32le` Sonic52 vector
//! fits the frozen `.msim` container unchanged (52 is within `1..=4096`),
//! so no format change is required or proposed (see
//! [`msim_vector_bytes`]).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

// ---------------------------------------------------------------------
// profile identity
// ---------------------------------------------------------------------

/// Provisional research profile id. Display only; never a comparison,
// partition, or cache key on its own (ADR 0017 D-4 applies to any future
// use: identity is always id *plus* fingerprint).
pub const SONIC52_PROFILE_ID: &str = "musicpack-similarity-sonic52-research-v1";

/// Production Discogs-EffNet profile ids, recorded here read-only so the
/// non-collision rule is executable. These strings are owned by
/// `musicpack-author/src/similarity_effnet.rs`; this crate must never
/// define, alter, or reuse them.
pub const DISCOGS_EFFNET_PROFILE_IDS: &[&str] = &[
    "musicpack-similarity-discogs-effnet-multi-v1",
    "musicpack-similarity-discogs-effnet-release-v1",
];

/// Research embedding dimensionality: the publicly reported structural
/// characteristic motivating this track.
pub const SONIC52_DIMENSIONS: usize = 52;

/// Intended output element type.
pub const SONIC52_OUTPUT_ENCODING: &str = "f32le";

/// Intended output activation: sigmoid, producing values in `[0, 1]`.
/// This is an *intended* property of a future independently implemented
/// model, not a measured fact: nothing here produces such values.
pub const SONIC52_ACTIVATION: &str = "sigmoid";

/// Reported network head, recorded as reported (see `EVIDENCE.md` in the
/// crate README for provenance): `Conv2D -> Flatten -> Dense(200) ->
/// Dense(52)`. Corroborated in public Essentia MusiCNN material as a
/// `Dense(200)` embedding layer followed by a dense-plus-sigmoid head;
/// the exact 52-wide output is the reported characteristic, not an
/// independently verified measurement.
pub const SONIC52_REPORTED_HEAD: &str = "Conv2D -> Flatten -> Dense(200) -> Dense(52)";

/// What is wrong with a research profile definition supplied to this
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// Empty display id.
    EmptyId,
    /// All-zero fingerprint (meaningless as an identity key).
    ZeroFingerprint,
    /// The id belongs to a production profile and must never be reused
    /// by research definitions.
    ReservedProductionId,
    /// The `musicpack-similarity-sonic52-research-v1` id was used with a
    /// dimensionality other than 52 (a different width is a different
    /// research identity, never a silent reuse of the v1 id).
    V1IdentityMismatch,
    /// Dimension count outside the `.msim` format validation domain
    /// `1 ..= 4096` (FORMAT_SPEC decision C, cited read-only).
    BadDimensions(u32),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::EmptyId => write!(f, "profile_id must not be empty"),
            ProfileError::ZeroFingerprint => {
                write!(f, "profile_fingerprint must not be 32 zero bytes")
            }
            ProfileError::ReservedProductionId => {
                write!(f, "profile_id belongs to a production profile")
            }
            ProfileError::V1IdentityMismatch => write!(
                f,
                "the {SONIC52_PROFILE_ID} id requires exactly {SONIC52_DIMENSIONS} dimensions"
            ),
            ProfileError::BadDimensions(d) => {
                write!(f, "dimensions {d} outside 1..=4096")
            }
        }
    }
}

impl std::error::Error for ProfileError {}

/// A Sonic52 research profile: the two identifiers of ADR 0017 §5.4 plus
/// the layout fact the embedding validator needs. The fingerprint is
/// supplied by the caller and never derived or invented here: there is no
/// trained model yet, so there is no implementation identity to hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sonic52Profile {
    /// Stable, human-facing research name (display only).
    pub profile_id: String,
    /// Exact future implementation identity (opaque 32 bytes for now).
    pub fingerprint: [u8; 32],
    /// Vector dimension count (52 for the v1 research identity).
    pub dimensions: u32,
}

impl Sonic52Profile {
    /// The v1 research identity: exactly 52 dimensions under the
    /// provisional research id.
    pub fn research_v1(fingerprint: [u8; 32]) -> Result<Self, ProfileError> {
        Self::new_variant(SONIC52_PROFILE_ID.to_string(), fingerprint, 52)
    }

    /// A future dimensionality-variant identity (the Sonic52-N ladder).
    /// The id must be caller-supplied, non-empty, distinct from every
    /// production profile id, and — when dimensions are 52 — must be the
    /// v1 id so the v1 identity cannot be silently redefined.
    pub fn new_variant(
        profile_id: String,
        fingerprint: [u8; 32],
        dimensions: u32,
    ) -> Result<Self, ProfileError> {
        if profile_id.is_empty() {
            return Err(ProfileError::EmptyId);
        }
        if fingerprint == [0u8; 32] {
            return Err(ProfileError::ZeroFingerprint);
        }
        if DISCOGS_EFFNET_PROFILE_IDS.contains(&profile_id.as_str()) {
            return Err(ProfileError::ReservedProductionId);
        }
        if dimensions == 0 || dimensions > 4096 {
            return Err(ProfileError::BadDimensions(dimensions));
        }
        if profile_id == SONIC52_PROFILE_ID && dimensions as usize != SONIC52_DIMENSIONS {
            return Err(ProfileError::V1IdentityMismatch);
        }
        Ok(Self {
            profile_id,
            fingerprint,
            dimensions,
        })
    }

    /// Two profiles share a comparison partition if and only if both the
    /// id and the fingerprint agree. A dimension collision alone never
    /// implies comparability.
    pub fn same_partition(&self, other: &Sonic52Profile) -> bool {
        self.profile_id == other.profile_id && self.fingerprint == other.fingerprint
    }
}

// ---------------------------------------------------------------------
// embedding contract
// ---------------------------------------------------------------------

/// What is wrong with a candidate Sonic52 embedding vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    /// Vector length differs from the profile dimensionality.
    WrongDimensions {
        /// Profile-declared dimension count.
        expected: usize,
        /// Offered vector length.
        found: usize,
    },
    /// A non-finite element (NaN or infinite) was offered.
    NonFinite,
}

impl std::fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbeddingError::WrongDimensions { expected, found } => {
                write!(
                    f,
                    "vector has {found} elements, profile declares {expected}"
                )
            }
            EmbeddingError::NonFinite => write!(f, "non-finite vector element"),
        }
    }
}

impl std::error::Error for EmbeddingError {}

/// A validated Sonic52 research embedding: exactly [`SONIC52_DIMENSIONS`]
/// finite `f32` values.
///
/// Structural acceptance only: the constructor checks length and
/// finiteness. The intended sigmoid output range is checked separately by
/// [`Sonic52Embedding::is_sigmoid_range`], so that range validation is
/// available without implying anything about trained-model quality (no
/// model exists yet, and this crate must never be read as claiming that
/// Sonic52 produces meaningful musical similarity).
#[derive(Debug, Clone, PartialEq)]
pub struct Sonic52Embedding {
    values: Vec<f32>,
}

impl Sonic52Embedding {
    /// Validates a candidate vector against the research profile's
    /// dimensionality. Rejects wrong lengths and NaN/Inf; accepts any
    /// finite values so that the sigmoid-range expectation stays a
    /// separately testable property rather than a hidden gate.
    pub fn from_vec(profile: &Sonic52Profile, values: Vec<f32>) -> Result<Self, EmbeddingError> {
        let expected = profile.dimensions as usize;
        if values.len() != expected {
            return Err(EmbeddingError::WrongDimensions {
                expected,
                found: values.len(),
            });
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(EmbeddingError::NonFinite);
        }
        Ok(Self { values })
    }

    /// Vector length (always the profile dimensionality).
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the embedding is empty (never true for validated values;
    /// provided so callers need no len-based special cases).
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The validated values.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Whether every value lies in the intended sigmoid output range
    /// `[0, 1]`. A validator for the intended activation contract, not a
    /// quality claim: a future trained model is *expected* to satisfy
    /// this, and anything that does not is outside the research contract.
    pub fn is_sigmoid_range(&self) -> bool {
        self.values.iter().all(|v| (0.0..=1.0).contains(v))
    }

    /// Deterministic little-endian serialization: `52 * 4 = 208` bytes.
    /// Equal logical content yields equal bytes (fixed order, no
    /// timestamps, no paths). This mirrors the `.msim` `f32le` vector
    /// encoding read-only; it does not implement, alter, or bypass the
    /// production `.msim` writer.
    pub fn to_f32le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.values.len() * 4);
        for value in &self.values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }
}

/// Serialized byte length of one 52-D `f32le` Sonic52 vector.
pub const SONIC52_F32LE_BYTES: usize = SONIC52_DIMENSIONS * 4;

/// The `.msim` format validation domain upper bound, cited read-only
/// (FORMAT_SPEC decision C): 52 is inside it, so a future 52-D `f32le`
/// vector fits the frozen container with no format change.
pub const MSIM_MAX_DIMENSIONS: u16 = 4096;

/// States that a future 52-D `f32le` Sonic52 vector fits the frozen
/// `.msim` container unchanged. Documentary only: this crate neither
/// reads nor writes `.msim` bytes.
pub fn fits_frozen_msim(dimensions: u32) -> bool {
    dimensions >= 1 && dimensions <= u32::from(MSIM_MAX_DIMENSIONS)
}

// ---------------------------------------------------------------------
// preprocessing / input-tensor contract: known vs unknown
// ---------------------------------------------------------------------

/// Evidence classification for one preprocessing parameter. The eventual
/// experimental choices must be labelled [`EvidenceStatus::ExperimentalChoice`]
/// — ours, not Plex facts. This phase makes no experimental preprocessing
/// choices at all (there is no model to feed yet), so no preprocessing-fact
/// row below uses that variant; it exists for the future training slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    /// Publicly reported or documented, with the source cited in the note.
    KnownFromEvidence,
    /// Bounded by public lineage material but unverified for the reported
    /// historical artifact.
    PartiallyConstrained,
    /// Not publicly documented and not decided here. Must not be assumed.
    Unknown,
    /// An explicit MusicPack research decision (not a Plex fact).
    ExperimentalChoice,
}

/// One row of the preprocessing/input-tensor contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractParam {
    /// Parameter name (e.g. `"fft_size"`).
    pub name: &'static str,
    /// Evidence classification.
    pub status: EvidenceStatus,
    /// Provenance or constraint note. For `KnownFromEvidence` this cites
    /// the public source; for `Unknown` it states what must not be assumed.
    pub note: &'static str,
}

/// The explicit preprocessing/input-tensor contract. Every parameter the
/// task requires is classified; nothing is invented. Notes cite public
/// sources without reproducing proprietary artifacts:
///
/// - *Public lineage*: Essentia's public MSD-MusiCNN material (model
///   metadata schema, `TensorflowPredictMusiCNN` frontend expecting
///   16 kHz audio, `[frames, 96]` mel-spectrogram input, `Dense(200)`
///   embedding layer plus dense-plus-sigmoid head) and community reports
///   that Plex's historical analysis used that model family (`Music.tflite`)
///   with an Annoy cosine/angular index.
/// - *Reported characteristic*: the 52-wide sigmoid head motivating this
///   track, recorded as reported, not independently verified here.
pub const PREPROCESSING_TABLE: &[ContractParam] = &[
    ContractParam {
        name: "sample_rate",
        status: EvidenceStatus::PartiallyConstrained,
        note: "16 kHz mono input is documented for the public MusiCNN frontend lineage; the historical artifact's rate is unverified",
    },
    ContractParam {
        name: "mono_stereo_handling",
        status: EvidenceStatus::PartiallyConstrained,
        note: "mono loading is documented for the public lineage; channel policy of the historical artifact is unverified",
    },
    ContractParam {
        name: "fft_size",
        status: EvidenceStatus::Unknown,
        note: "not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "hop_size",
        status: EvidenceStatus::Unknown,
        note: "not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "window_function",
        status: EvidenceStatus::Unknown,
        note: "not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "mel_bin_count",
        status: EvidenceStatus::PartiallyConstrained,
        note: "96 bands documented for the public msd-musicnn input signature; the historical artifact's count is unverified",
    },
    ContractParam {
        name: "mel_frequency_range",
        status: EvidenceStatus::Unknown,
        note: "not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "logarithmic_scaling",
        status: EvidenceStatus::Unknown,
        note: "scaling parameters not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "normalization",
        status: EvidenceStatus::Unknown,
        note: "not publicly documented for the reported artifact; must not be assumed",
    },
    ContractParam {
        name: "temporal_window_length",
        status: EvidenceStatus::Unknown,
        note: "window length of the reported artifact is unverified; the public lineage uses its own frame counts, which must not be copied as facts",
    },
    ContractParam {
        name: "input_tensor_shape",
        status: EvidenceStatus::Unknown,
        note: "exact input tensor shape of the reported artifact is unverified; see Sonic52InputContract",
    },
    ContractParam {
        name: "temporal_aggregation",
        status: EvidenceStatus::Unknown,
        note: "frame-to-track aggregation of the reported artifact is not publicly documented; must not be assumed",
    },
    ContractParam {
        name: "similarity_metric",
        status: EvidenceStatus::PartiallyConstrained,
        note: "community reports describe an Annoy cosine/angular index; unverified here and not adopted as a MusicPack decision",
    },
    ContractParam {
        name: "output_dimensionality",
        status: EvidenceStatus::KnownFromEvidence,
        note: "52 dimensions is the publicly reported structural characteristic motivating this track",
    },
    ContractParam {
        name: "output_activation",
        status: EvidenceStatus::KnownFromEvidence,
        note: "a terminal sigmoid op is documented in public MusiCNN-family model material; the 52-wide sigmoid head is the reported characteristic",
    },
    ContractParam {
        name: "network_head",
        status: EvidenceStatus::KnownFromEvidence,
        note: "Conv2D -> Flatten -> Dense(200) -> Dense(52) is the reported architecture; Dense(200)-then-sigmoid structure is corroborated by public Essentia model material",
    },
    ContractParam {
        name: "deterministic_inference",
        status: EvidenceStatus::ExperimentalChoice,
        note: "OUR requirement of the future research model, not a reported Plex fact: identical input and implementation must yield identical vectors",
    },
];

/// The explicit input-tensor contract. Both fields are `None`: the exact
/// input dtype and shape of the reported historical artifact are unknown,
/// and this phase — which builds no model — must not default them into
/// existence. A future training slice fills these in as labelled
/// experimental choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sonic52InputContract {
    /// Exact input tensor dtype, if decided. Currently undecided.
    pub dtype: Option<&'static str>,
    /// Exact input tensor shape, if decided. Currently undecided.
    pub shape: Option<&'static str>,
}

impl Sonic52InputContract {
    /// The current contract: everything undecided.
    pub const UNDECIDED: Self = Self {
        dtype: None,
        shape: None,
    };
}

// ---------------------------------------------------------------------
// future experiment contract (documented, not implemented)
// ---------------------------------------------------------------------

/// Dimensionality ladder for future embedding-size experiments. 52-D is
/// the initial target because it is the publicly reported structural
/// characteristic motivating this research — not because any quality
/// result prefers it (no training or benchmarking exists yet).
pub const DIMENSIONALITY_LADDER: &[u32] = &[32, 52, 64, 96, 128];

/// Initial research target width.
pub const INITIAL_TARGET_DIMENSIONS: u32 = 52;

/// Baseline profiles for the future comparison, cited read-only from the
/// production implementation (discogs-effnet 1280-D multi and 512-D
/// release). The future experiment is conceptually: test corpus through
/// Discogs-EffNet, Sonic52-52, and Sonic52-N variants, then embeddings to
/// nearest neighbours to quantitative metrics to human evaluation to the
/// MusicPack similarity benchmark. None of that is implemented here.
pub const BASELINE_PROFILES: &[(&str, u32)] = &[
    ("musicpack-similarity-discogs-effnet-multi-v1", 1280),
    ("musicpack-similarity-discogs-effnet-release-v1", 512),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_profile() -> Sonic52Profile {
        Sonic52Profile::research_v1([0x5a; 32]).unwrap()
    }

    fn embedding_of(values: Vec<f32>) -> Sonic52Embedding {
        Sonic52Embedding::from_vec(&v1_profile(), values).unwrap()
    }

    #[test]
    fn embedding_has_exactly_52_dimensions() {
        assert_eq!(SONIC52_DIMENSIONS, 52);
        let ok = embedding_of(vec![0.5; 52]);
        assert_eq!(ok.len(), 52);
        assert!(!ok.is_empty());
        // 51 and 53 are rejected with the expected counts attached.
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), vec![0.5; 51]).unwrap_err(),
            EmbeddingError::WrongDimensions {
                expected: 52,
                found: 51
            }
        );
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), vec![0.5; 53]).unwrap_err(),
            EmbeddingError::WrongDimensions {
                expected: 52,
                found: 53
            }
        );
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), vec![]).unwrap_err(),
            EmbeddingError::WrongDimensions {
                expected: 52,
                found: 0
            }
        );
    }

    #[test]
    fn values_must_be_valid_finite_f32() {
        let mut with_nan = vec![0.5f32; 52];
        with_nan[7] = f32::NAN;
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), with_nan).unwrap_err(),
            EmbeddingError::NonFinite
        );
        let mut with_inf = vec![0.5f32; 52];
        with_inf[51] = f32::INFINITY;
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), with_inf).unwrap_err(),
            EmbeddingError::NonFinite
        );
        let mut with_neg_inf = vec![0.5f32; 52];
        with_neg_inf[0] = f32::NEG_INFINITY;
        assert_eq!(
            Sonic52Embedding::from_vec(&v1_profile(), with_neg_inf).unwrap_err(),
            EmbeddingError::NonFinite
        );
    }

    #[test]
    fn intended_sigmoid_output_range_can_be_validated() {
        // Structural acceptance does not gate on range: finite values are
        // accepted so that range validation stays a separate, explicit check.
        let in_range = embedding_of(vec![0.5; 52]);
        assert!(in_range.is_sigmoid_range());
        let edges = embedding_of(
            (0..52)
                .map(|i| if i % 2 == 0 { 0.0 } else { 1.0 })
                .collect(),
        );
        assert!(edges.is_sigmoid_range());
        let below = embedding_of((0..52).map(|i| if i == 0 { -0.1 } else { 0.5 }).collect());
        assert!(!below.is_sigmoid_range());
        let above = embedding_of((0..52).map(|i| if i == 51 { 1.1 } else { 0.5 }).collect());
        assert!(!above.is_sigmoid_range());
    }

    #[test]
    fn model_version_identity_is_explicit() {
        assert_eq!(
            SONIC52_PROFILE_ID,
            "musicpack-similarity-sonic52-research-v1"
        );
        let profile = v1_profile();
        assert_eq!(profile.profile_id, SONIC52_PROFILE_ID);
        assert_eq!(profile.fingerprint, [0x5a; 32]);
        assert_eq!(profile.dimensions, 52);
        // Identity failures are explicit, never defaulted.
        assert_eq!(
            Sonic52Profile::research_v1([0u8; 32]).unwrap_err(),
            ProfileError::ZeroFingerprint
        );
        assert_eq!(
            Sonic52Profile::new_variant(String::new(), [0x5a; 32], 52).unwrap_err(),
            ProfileError::EmptyId
        );
        assert_eq!(
            Sonic52Profile::new_variant("x".to_string(), [0u8; 32], 52).unwrap_err(),
            ProfileError::ZeroFingerprint
        );
    }

    #[test]
    fn invalid_dimensionality_is_rejected() {
        // Zero and above the .msim validation domain are rejected.
        assert_eq!(
            Sonic52Profile::new_variant("x-sonic52-test".to_string(), [0x5a; 32], 0).unwrap_err(),
            ProfileError::BadDimensions(0)
        );
        assert_eq!(
            Sonic52Profile::new_variant("x-sonic52-test".to_string(), [0x5a; 32], 4097)
                .unwrap_err(),
            ProfileError::BadDimensions(4097)
        );
        // The v1 id with a non-52 width is a different identity, never a
        // silent redefinition.
        assert_eq!(
            Sonic52Profile::new_variant(SONIC52_PROFILE_ID.to_string(), [0x5a; 32], 64)
                .unwrap_err(),
            ProfileError::V1IdentityMismatch
        );
        // A future dimensionality variant with its own id is accepted.
        let n64 = Sonic52Profile::new_variant(
            "musicpack-similarity-sonic52-research-64-v0".to_string(),
            [0x5a; 32],
            64,
        )
        .unwrap();
        assert_eq!(n64.dimensions, 64);
        // And the embedding validator follows the variant's width.
        assert!(Sonic52Embedding::from_vec(&n64, vec![0.5; 64]).is_ok());
        assert_eq!(
            Sonic52Embedding::from_vec(&n64, vec![0.5; 52]).unwrap_err(),
            EmbeddingError::WrongDimensions {
                expected: 64,
                found: 52
            }
        );
    }

    #[test]
    fn nan_and_inf_rejected_at_both_boundaries() {
        // Embedding boundary (above) plus determinism of the rejection:
        // the same bad input is rejected the same way every time.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut values = vec![0.25f32; 52];
            values[13] = bad;
            assert_eq!(
                Sonic52Embedding::from_vec(&v1_profile(), values.clone()).unwrap_err(),
                Sonic52Embedding::from_vec(&v1_profile(), values).unwrap_err(),
            );
        }
    }

    #[test]
    fn sonic52_identity_cannot_collide_with_discogs_effnet() {
        // The research id is outside the production namespace values.
        assert!(!DISCOGS_EFFNET_PROFILE_IDS.contains(&SONIC52_PROFILE_ID));
        assert_eq!(DISCOGS_EFFNET_PROFILE_IDS.len(), 2);
        // Neither production id can be registered as a research profile.
        for production_id in DISCOGS_EFFNET_PROFILE_IDS {
            assert_eq!(
                Sonic52Profile::new_variant(production_id.to_string(), [0x5a; 32], 1280)
                    .unwrap_err(),
                ProfileError::ReservedProductionId
            );
        }
        // Partitioning requires id *and* fingerprint agreement: same id
        // with a different fingerprint is a different partition, and a
        // dimension collision alone never implies comparability.
        let a = Sonic52Profile::research_v1([0x11; 32]).unwrap();
        let b = Sonic52Profile::research_v1([0x22; 32]).unwrap();
        let a_again = Sonic52Profile::research_v1([0x11; 32]).unwrap();
        assert!(a.same_partition(&a_again));
        assert!(!a.same_partition(&b));
        let n52_other_id = Sonic52Profile::new_variant(
            "musicpack-similarity-sonic52-other-v0".to_string(),
            [0x11; 32],
            52,
        )
        .unwrap();
        assert!(!a.same_partition(&n52_other_id));
    }

    #[test]
    fn serialization_is_deterministic() {
        let embedding = embedding_of((0..52).map(|i| i as f32 / 52.0).collect());
        let first = embedding.to_f32le_bytes();
        let second = embedding.to_f32le_bytes();
        assert_eq!(first, second);
        assert_eq!(first.len(), 208);
        assert_eq!(first.len(), SONIC52_F32LE_BYTES);
        // Little-endian round-trip of the first element (0.0).
        assert_eq!(&first[0..4], &0.0f32.to_le_bytes());
        // A different vector serializes differently.
        let other = embedding_of(vec![1.0; 52]);
        assert_ne!(first, other.to_f32le_bytes());
    }

    #[test]
    fn future_52d_vector_fits_the_frozen_msim_container() {
        // Documentary pin: 52-D f32le needs no format change. The format
        // itself is untouched — this only asserts the arithmetic that the
        // README/ADR cite.
        assert_eq!(MSIM_MAX_DIMENSIONS, 4096);
        assert!(fits_frozen_msim(52));
        assert!(fits_frozen_msim(32));
        assert!(fits_frozen_msim(128));
        assert!(!fits_frozen_msim(0));
        assert!(!fits_frozen_msim(4097));
        assert_eq!(SONIC52_OUTPUT_ENCODING, "f32le");
    }

    #[test]
    fn preprocessing_contract_classifies_every_required_parameter() {
        let required = [
            "sample_rate",
            "mono_stereo_handling",
            "fft_size",
            "hop_size",
            "window_function",
            "mel_bin_count",
            "mel_frequency_range",
            "logarithmic_scaling",
            "normalization",
            "temporal_window_length",
            "input_tensor_shape",
            "temporal_aggregation",
            "similarity_metric",
        ];
        for name in required {
            assert!(
                PREPROCESSING_TABLE.iter().any(|row| row.name == name),
                "contract is missing parameter {name}"
            );
        }
        // The must-not-invent rows stay Unknown: FFT, hop, window, mel
        // range, log scaling, normalization, temporal window, tensor
        // shape, aggregation.
        for name in [
            "fft_size",
            "hop_size",
            "window_function",
            "mel_frequency_range",
            "logarithmic_scaling",
            "normalization",
            "temporal_window_length",
            "input_tensor_shape",
            "temporal_aggregation",
        ] {
            let row = PREPROCESSING_TABLE
                .iter()
                .find(|row| row.name == name)
                .unwrap();
            assert_eq!(
                row.status,
                EvidenceStatus::Unknown,
                "{name} must stay Unknown"
            );
            assert!(!row.note.is_empty());
        }
        // No preprocessing *fact* is claimed as our experimental choice in
        // this phase; the only ExperimentalChoice row is the determinism
        // requirement we impose on the future model.
        for row in PREPROCESSING_TABLE {
            if row.status == EvidenceStatus::ExperimentalChoice {
                assert_eq!(row.name, "deterministic_inference");
            }
        }
        // KnownFromEvidence rows always carry their provenance note.
        for row in PREPROCESSING_TABLE
            .iter()
            .filter(|row| row.status == EvidenceStatus::KnownFromEvidence)
        {
            assert!(!row.note.is_empty(), "{} needs a provenance note", row.name);
        }
    }

    #[test]
    fn input_tensor_contract_leaves_unknowns_explicit() {
        // This phase decides nothing about the input tensor: unknown
        // means None, never a convenient default.
        assert_eq!(Sonic52InputContract::UNDECIDED.dtype, None);
        assert_eq!(Sonic52InputContract::UNDECIDED.shape, None);
    }

    #[test]
    fn future_dimensionality_ladder_targets_52_first() {
        assert_eq!(INITIAL_TARGET_DIMENSIONS, 52);
        assert!(DIMENSIONALITY_LADDER.contains(&52));
        for width in [32u32, 64, 96, 128] {
            assert!(DIMENSIONALITY_LADDER.contains(&width));
        }
        // Baselines are the production Discogs-EffNet widths, cited only.
        assert_eq!(
            BASELINE_PROFILES,
            &[
                ("musicpack-similarity-discogs-effnet-multi-v1", 1280),
                ("musicpack-similarity-discogs-effnet-release-v1", 512),
            ]
        );
    }
}
