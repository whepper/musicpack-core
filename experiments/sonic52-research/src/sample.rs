//! Sonic52 Slice 8: deterministic training sample + forward pipeline.
//!
//! Connects Slices 1–7 into one end-to-end training-example path that
//! uses frozen reference weights and learns nothing:
//!
//! ```text
//! dataset row + selection
//!   → sample identity (Slice 6)
//!   → real audio fixture (Slice 2 fixtures only)
//!   → H0 preprocessing → [187,96] patches (Slices 1–3, unchanged)
//!   → reference network → 52-D logits + sigmoid (Slice 4 + logits)
//!   → Mean aggregation (v1 choice)
//!   → synthetic target → BCE loss oracle (Slices 6–7)
//! ```
//!
//! Two rules govern everything here:
//!
//! 1. **Logits feed the loss; embeddings persist.** The BCE oracle
//!    takes pre-sigmoid logits, so the forward path exposes both and
//!    digests both — the wiring is auditable instead of accidental,
//!    and the stored representation stays post-sigmoid (unchanged).
//! 2. **The target is synthetic.** Slice 7 selected no real-world
//!    target semantics, so none is invented now: targets come from a
//!    documented toy rule (`synthetic-rule-v1`), never from artist,
//!    album, genre, or tag metadata.
//!
//! No gradients, optimizers, loops, checkpoints, or learned weights.
//! Sample → forward → target → loss is proven correct first; learning
//! comes only after this pipeline is trusted.

#![forbid(unsafe_code)]

use std::fmt;

use crate::corpus::{reference_weight_digest, SONIC52_REFERENCE_NETWORK_ID};
use crate::experiment::{digest_patches, layout_patches, PATCH_CONFIG_H0};
use crate::frontend::{MelFrontend, Sonic52MelPatch};
use crate::ingest::{ingest_bytes, DEFAULT_READ_FRAMES};
use crate::network::{aggregate_reference_vectors, serialize_output, Sonic52ReferenceNetwork};
use crate::objective::{validate_target, V1_OBJECTIVE};
use crate::training::{bce_with_logits, training_sample_id, DatasetManifest, Split};

// ---------------------------------------------------------------------
// synthetic training target (documented toy rule, not real labels)
// ---------------------------------------------------------------------

/// Identifier of the toy target rule. Recorded in every trace so a
/// synthetic target can never be mistaken for selected real labels.
pub const SYNTHETIC_TARGET_RULE_ID: &str = "synthetic-rule-v1";

/// Deterministic 52-D binary target from a sample identity: bit `k` of
/// `SHA-256("synthetic-target-v1|{sample_id}")` (first 52 bits) becomes
/// 1.0 or 0.0. Always 52-dimensional, always valid BCE input, stable
/// across machines. The rule is arbitrary by design — its only job is
/// giving the loss oracle a reproducible, well-formed target while no
/// real target semantics exist.
pub fn synthetic_target(sample_id: &str) -> [f32; 52] {
    let digest = musicpack_core::format::checksum::sha256_hex(
        format!("synthetic-target-v1|{sample_id}").as_bytes(),
    );
    let mut target = [0.0f32; 52];
    for (k, slot) in target.iter_mut().enumerate() {
        let byte =
            u8::from_str_radix(&digest[2 * (k / 8)..2 * (k / 8) + 2], 16).expect("hex of digest");
        if byte >> (k % 8) & 1 == 1 {
            *slot = 1.0;
        }
    }
    target
}

// ---------------------------------------------------------------------
// deterministic sample selection
// ---------------------------------------------------------------------

/// Which rows of a dataset manifest become training samples. Only the
/// split filter and the track cap vary: patch layout stays v1 (stride
/// ablation remains the experiment harness's job, so manifest patch
/// counts always apply). Both axes join the scientific identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleSelection {
    /// Restrict to one split (`None` = every row).
    pub split_filter: Option<Split>,
    /// Take at most this many tracks in id order (`None` = all).
    pub max_tracks: Option<usize>,
}

/// The v1 selection: every row, no cap.
pub const SAMPLE_SELECTION_V1: SampleSelection = SampleSelection {
    split_filter: None,
    max_tracks: None,
};

impl SampleSelection {
    /// Canonical deterministic encoding (digested for identity).
    pub fn canonical(&self) -> String {
        format!(
            "selection-v1|{}|{}",
            match self.split_filter {
                Some(split) => split.as_str(),
                None => "all",
            },
            match self.max_tracks {
                Some(cap) => cap.to_string(),
                None => "all".to_string(),
            }
        )
    }

    /// Deterministic selection identity.
    pub fn digest(&self) -> String {
        musicpack_core::format::checksum::sha256_hex(self.canonical().as_bytes())
    }
}

/// One selected training sample: a manifest row plus a patch index.
/// Rows come from [`DatasetManifest`] (already track-id ordered), so
/// selection order is manifest order filtered and capped — never
/// filesystem order, never hash-map order, never random.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleRef {
    /// Stable source-track identifier.
    pub track_id: String,
    /// Patch index within the track's v1 layout.
    pub patch_index: u32,
}

/// Selects samples deterministically: manifest order, optional split
/// filter, optional track cap, patch indices `0..patch_count` per row.
pub fn select_samples(manifest: &DatasetManifest, selection: &SampleSelection) -> Vec<SampleRef> {
    let mut samples = Vec::new();
    let mut tracks_taken = 0usize;
    for row in &manifest.rows {
        if let Some(split) = selection.split_filter {
            if row.split != split {
                continue;
            }
        }
        if let Some(cap) = selection.max_tracks {
            if tracks_taken >= cap {
                break;
            }
        }
        tracks_taken += 1;
        for patch_index in 0..row.patch_count {
            samples.push(SampleRef {
                track_id: row.track_id.clone(),
                patch_index: patch_index as u32,
            });
        }
    }
    samples
}

// ---------------------------------------------------------------------
// sample errors (explicit, diagnosable, PartialEq-testable)
// ---------------------------------------------------------------------

/// What can fail while running one training sample. Wraps upstream
/// detail strings (consistent with the corpus runner's failure style);
/// every variant names the failing track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleError {
    /// Audio ingestion failed.
    Ingest {
        /// Track id.
        track: String,
        /// Cause.
        detail: String,
    },
    /// Patch index exceeds the track's laid-out patches.
    PatchIndexOutOfRange {
        /// Track id.
        track: String,
        /// Requested index.
        index: u32,
        /// Available patches.
        patches: usize,
    },
    /// Reference forward failed.
    Network {
        /// Track id.
        track: String,
        /// Cause.
        detail: String,
    },
    /// Synthetic target rejected (defensive; cannot happen by
    /// construction, but the wiring stays explicit).
    Target {
        /// Track id.
        track: String,
        /// Cause.
        detail: String,
    },
    /// Loss oracle rejected the pair.
    Loss {
        /// Track id.
        track: String,
        /// Cause.
        detail: String,
    },
    /// Patch-vector aggregation failed (non-empty input only; zero
    /// patches short-circuit to no aggregate before this runs).
    Aggregation {
        /// Track id.
        track: String,
        /// Cause.
        detail: String,
    },
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SampleError::Ingest { track, detail } => {
                write!(f, "sample {track}: ingest failed: {detail}")
            }
            SampleError::PatchIndexOutOfRange {
                track,
                index,
                patches,
            } => write!(f, "sample {track}: patch {index} of {patches}"),
            SampleError::Network { track, detail } => {
                write!(f, "sample {track}: network failed: {detail}")
            }
            SampleError::Target { track, detail } => {
                write!(f, "sample {track}: target rejected: {detail}")
            }
            SampleError::Loss { track, detail } => {
                write!(f, "sample {track}: loss rejected: {detail}")
            }
            SampleError::Aggregation { track, detail } => {
                write!(f, "sample {track}: aggregation failed: {detail}")
            }
        }
    }
}

impl std::error::Error for SampleError {}

// ---------------------------------------------------------------------
// per-patch forward trace (inspectable, path/host-free)
// ---------------------------------------------------------------------

/// Inspectable record of one training sample's forward evaluation:
/// every identity plus every digest a future training discrepancy
/// could turn on. No absolute paths, no host facts, no timings.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleTrace {
    /// Dataset identity.
    pub dataset_id: String,
    /// Dataset version.
    pub dataset_version: String,
    /// Source-track identity.
    pub track_id: String,
    /// Patch index within the track's v1 layout.
    pub patch_index: u32,
    /// Slice 6 sample identity.
    pub sample_id: String,
    /// SHA-256 over this patch's row-major bytes.
    pub patch_digest: String,
    /// Reference-network research identity.
    pub network_id: String,
    /// Digest over the reference weights in use.
    pub weight_digest: String,
    /// SHA-256 over the 52 pre-sigmoid logits.
    pub logits_digest: String,
    /// SHA-256 over the 52-D sigmoid embedding.
    pub embedding_digest: String,
    /// Synthetic target rule id.
    pub target_id: String,
    /// SHA-256 over the 52-D synthetic target.
    pub target_digest: String,
    /// BCE-with-logits loss for this patch.
    pub loss: f32,
}

impl fmt::Display for SampleTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "dataset: {}@{}", self.dataset_id, self.dataset_version)?;
        writeln!(f, "track: {}", self.track_id)?;
        writeln!(f, "patch: {}", self.patch_index)?;
        writeln!(f, "sample: {}", self.sample_id)?;
        writeln!(f, "patch_digest: {}", self.patch_digest)?;
        writeln!(f, "network: {}", self.network_id)?;
        writeln!(f, "weights: {}", self.weight_digest)?;
        writeln!(f, "logits: {}", self.logits_digest)?;
        writeln!(f, "embedding: {}", self.embedding_digest)?;
        writeln!(f, "target: {} {}", self.target_id, self.target_digest)?;
        writeln!(f, "loss: {:.6}", self.loss)
    }
}

/// SHA-256 hex over 52 little-endian f32s.
fn digest_52(values: &[f32; 52]) -> String {
    musicpack_core::format::checksum::sha256_hex(&serialize_output(values))
}

/// Runs one training sample end to end: ingest → H0 frontend → v1
/// layout → patch bounds-check → reference forward (logits +
/// embedding) → synthetic target → BCE validation → BCE loss.
pub fn run_sample(
    audio_bytes: &[u8],
    dataset_id: &str,
    dataset_version: &str,
    track_id: &str,
    patch_index: u32,
    preprocessing_id: &str,
    network: &Sonic52ReferenceNetwork,
) -> Result<SampleTrace, SampleError> {
    let (input, _) =
        ingest_bytes(audio_bytes, DEFAULT_READ_FRAMES).map_err(|error| SampleError::Ingest {
            track: track_id.to_string(),
            detail: error.to_string(),
        })?;
    let frontend = MelFrontend::process_contiguous(input.samples());
    let patches = layout_patches(&frontend.mel_frames, crate::experiment::PATCH_CONFIG_H0);
    if patch_index as usize >= patches.len() {
        return Err(SampleError::PatchIndexOutOfRange {
            track: track_id.to_string(),
            index: patch_index,
            patches: patches.len(),
        });
    }
    run_sample_on_patches(
        &patches,
        dataset_id,
        dataset_version,
        track_id,
        patch_index,
        preprocessing_id,
        network,
    )
}

/// Shared tail of [`run_sample`] operating on already-laid-out H0
/// patches: bounds-check → forward → target → loss → trace. Split out
/// so tests can drive the mathematics without re-decoding audio.
pub fn run_sample_on_patches(
    patches: &[Sonic52MelPatch],
    dataset_id: &str,
    dataset_version: &str,
    track_id: &str,
    patch_index: u32,
    preprocessing_id: &str,
    network: &Sonic52ReferenceNetwork,
) -> Result<SampleTrace, SampleError> {
    let sample_id = training_sample_id(
        dataset_id,
        dataset_version,
        track_id,
        preprocessing_id,
        patch_index,
    );
    let patch = patches
        .get(patch_index as usize)
        .ok_or(SampleError::PatchIndexOutOfRange {
            track: track_id.to_string(),
            index: patch_index,
            patches: patches.len(),
        })?;
    let detail = network
        .forward_detailed(patch)
        .map_err(|error| SampleError::Network {
            track: track_id.to_string(),
            detail: error.to_string(),
        })?;
    let target = synthetic_target(&sample_id);
    validate_target(&V1_OBJECTIVE, &target).map_err(|error| SampleError::Target {
        track: track_id.to_string(),
        detail: error.to_string(),
    })?;
    let loss = bce_with_logits(&detail.logits, &target).map_err(|error| SampleError::Loss {
        track: track_id.to_string(),
        detail: error.to_string(),
    })?;
    Ok(SampleTrace {
        dataset_id: dataset_id.to_string(),
        dataset_version: dataset_version.to_string(),
        track_id: track_id.to_string(),
        patch_index,
        sample_id,
        patch_digest: digest_patches(std::slice::from_ref(patch)),
        network_id: SONIC52_REFERENCE_NETWORK_ID.to_string(),
        weight_digest: reference_weight_digest(network),
        logits_digest: digest_52(&detail.logits),
        embedding_digest: digest_52(&detail.embedding),
        target_id: SYNTHETIC_TARGET_RULE_ID.to_string(),
        target_digest: digest_52(&target),
        loss,
    })
}

// ---------------------------------------------------------------------
// whole-track training trace (per-patch traces + mean loss)
// ---------------------------------------------------------------------

/// One track's training view: per-patch traces, the v1 Mean aggregate
/// embedding (None for zero patches — the Slice 5 null-policy holds),
/// and the mean per-patch BCE loss (None with no patches to score).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackTrainingTrace {
    /// Stable source-track identifier.
    pub track_id: String,
    /// One trace per laid-out patch, in order.
    pub traces: Vec<SampleTrace>,
    /// Mean-aggregated 52-D sigmoid embedding, if any patches exist.
    pub aggregate: Option<[f32; 52]>,
    /// Mean per-patch BCE loss, if any patches exist.
    pub mean_loss: Option<f32>,
}

/// Runs every laid-out patch of one track's audio and aggregates per
/// the v1 contract (Mean over sigmoid embeddings; loss averaged over
/// patch losses — both orders documented in the module docs).
pub fn run_track_training(
    audio_bytes: &[u8],
    dataset_id: &str,
    dataset_version: &str,
    track_id: &str,
    preprocessing_id: &str,
    network: &Sonic52ReferenceNetwork,
) -> Result<TrackTrainingTrace, SampleError> {
    let (input, _) =
        ingest_bytes(audio_bytes, DEFAULT_READ_FRAMES).map_err(|error| SampleError::Ingest {
            track: track_id.to_string(),
            detail: error.to_string(),
        })?;
    let frontend = MelFrontend::process_contiguous(input.samples());
    let patches = layout_patches(&frontend.mel_frames, PATCH_CONFIG_H0);
    let mut traces = Vec::with_capacity(patches.len());
    for (index, _) in patches.iter().enumerate() {
        traces.push(run_sample_on_patches(
            &patches,
            dataset_id,
            dataset_version,
            track_id,
            index as u32,
            preprocessing_id,
            network,
        )?);
    }
    // Recompute embeddings through the canonical forward_many path so
    // the aggregate matches the corpus runner bit for bit.
    let mut patch_vectors: Vec<[f32; 52]> = Vec::with_capacity(patches.len());
    for patch in &patches {
        patch_vectors.push(
            network
                .forward(patch)
                .map_err(|error| SampleError::Network {
                    track: track_id.to_string(),
                    detail: error.to_string(),
                })?,
        );
    }
    // Zero patches short-circuit to the Slice 5 null-policy outcome
    // (no aggregate, no loss) instead of calling the aggregator on an
    // empty vector set, which correctly rejects empty input.
    let aggregate = if patch_vectors.is_empty() {
        None
    } else {
        aggregate_reference_vectors(&patch_vectors, crate::experiment::Aggregation::Mean).map_err(
            |error| SampleError::Aggregation {
                track: track_id.to_string(),
                detail: error.to_string(),
            },
        )?
    };
    let mean_loss = if traces.is_empty() {
        None
    } else {
        Some(traces.iter().map(|trace| trace.loss).sum::<f32>() / traces.len() as f32)
    };
    Ok(TrackTrainingTrace {
        track_id: track_id.to_string(),
        traces,
        aggregate,
        mean_loss,
    })
}

// ---------------------------------------------------------------------
// training-example identity composition
// ---------------------------------------------------------------------

/// Inputs to [`training_example_id`]: one field per scientific axis.
/// Grouped as a struct (rather than nine positional arguments) so the
/// call sites stay readable and the axis list stays explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleIdentityInput<'a> {
    /// Dataset identity.
    pub dataset_id: &'a str,
    /// Dataset version.
    pub dataset_version: &'a str,
    /// Sample-selection digest.
    pub selection_digest: &'a str,
    /// Slice 6 sample identity.
    pub sample_id: &'a str,
    /// Preprocessing contract text.
    pub preprocessing_id: &'a str,
    /// Model name (e.g. `"sonic52-52"`).
    pub model_id: &'a str,
    /// Weight-set digest.
    pub weight_digest: &'a str,
    /// Objective identity digest.
    pub objective_digest: &'a str,
    /// Target rule id.
    pub target_id: &'a str,
}

/// Full identity of one future training example: every scientific axis
/// that must change the identity when it changes. Timing, host,
/// filesystem path, and incidental ordering have no fields here.
pub fn training_example_id(input: &ExampleIdentityInput<'_>) -> String {
    musicpack_core::format::checksum::sha256_hex(
        format!(
            "example-v1|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            input.dataset_id,
            input.dataset_version,
            input.selection_digest,
            input.sample_id,
            input.preprocessing_id,
            input.model_id,
            input.weight_digest,
            input.objective_digest,
            input.target_id
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::RESEARCH_V1_CONTRACT;
    use crate::network::{reference_network, REFERENCE_SEED};
    use crate::training::{DatasetManifest, LicenseClass, ManifestTrack, Split};

    fn preprocessing_id() -> String {
        RESEARCH_V1_CONTRACT.to_string()
    }

    fn manifest_row(track_id: &str, split: Split, patch_count: usize) -> ManifestTrack {
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
            patch_count,
            split,
            exclusion: None,
        }
    }

    fn fixture_manifest() -> DatasetManifest {
        DatasetManifest::build(
            "bank".to_string(),
            "v3".to_string(),
            preprocessing_id(),
            vec![
                manifest_row("b-track", Split::Train, 3),
                manifest_row("a-track", Split::Test, 1),
                manifest_row("c-track", Split::Train, 0),
            ],
        )
        .expect("valid fixture manifest")
    }

    // Minimal hand-rolled 16-bit mono WAV (test-only bytes).
    fn wav_pcm16(rate: u32, frames: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + frames.len() * 2) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&((rate as usize * 2) as u32).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&((frames.len() * 2) as u32).to_le_bytes());
        for sample in frames {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    fn sine_16k(seconds: u32) -> Vec<u8> {
        let frames: Vec<i16> = (0..(16_000 * seconds) as usize)
            .map(|i| {
                (0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 32767.0)
                    as i16
            })
            .collect();
        wav_pcm16(16_000, &frames)
    }

    // -- selection --------------------------------------------------------

    #[test]
    fn selection_is_deterministic_and_configured() {
        let manifest = fixture_manifest();
        let all = select_samples(&manifest, &SAMPLE_SELECTION_V1);
        // Manifest (sorted) order, patch indices expanded: a(1), b(3), c(0).
        assert_eq!(
            all.iter()
                .map(|sample| (sample.track_id.as_str(), sample.patch_index))
                .collect::<Vec<_>>(),
            vec![
                ("a-track", 0),
                ("b-track", 0),
                ("b-track", 1),
                ("b-track", 2)
            ]
        );
        // Same manifest rebuilt from shuffled rows selects identically.
        let mut rows = vec![
            manifest_row("c-track", Split::Train, 0),
            manifest_row("a-track", Split::Test, 1),
            manifest_row("b-track", Split::Train, 3),
        ];
        rows.reverse();
        let rebuilt = DatasetManifest::build(
            "bank".to_string(),
            "v3".to_string(),
            preprocessing_id(),
            rows,
        )
        .unwrap();
        assert_eq!(select_samples(&rebuilt, &SAMPLE_SELECTION_V1), all);
        // Split filter + track cap change the selection and its identity.
        let train_only = SampleSelection {
            split_filter: Some(Split::Train),
            max_tracks: None,
        };
        assert_eq!(
            select_samples(&manifest, &train_only)
                .iter()
                .map(|sample| sample.track_id.as_str())
                .collect::<Vec<_>>(),
            vec!["b-track", "b-track", "b-track"]
        );
        let capped = SampleSelection {
            split_filter: None,
            max_tracks: Some(1),
        };
        assert_eq!(select_samples(&manifest, &capped).len(), 1);
        assert_ne!(SAMPLE_SELECTION_V1.digest(), train_only.digest());
        assert_ne!(SAMPLE_SELECTION_V1.digest(), capped.digest());
        assert_eq!(SAMPLE_SELECTION_V1.digest().len(), 64);
    }

    // -- synthetic target ----------------------------------------------------

    #[test]
    fn synthetic_target_is_deterministic_and_valid() {
        let first = synthetic_target("sample-abc");
        assert_eq!(first.len(), 52);
        assert_eq!(first, synthetic_target("sample-abc"));
        assert!(first.iter().all(|v| *v == 0.0 || *v == 1.0));
        assert!(first.contains(&1.0));
        assert!(first.contains(&0.0));
        // Different sample → different target; rule id is explicit.
        assert_ne!(first, synthetic_target("sample-abd"));
        assert_eq!(SYNTHETIC_TARGET_RULE_ID, "synthetic-rule-v1");
        // Always valid BCE input under the v1 objective.
        validate_target(&V1_OBJECTIVE, &first).expect("synthetic target is valid");
    }

    // -- patch identity ---------------------------------------------------------

    #[test]
    fn patch_identity_follows_the_slice6_chain() {
        // Stable; path-free (no path participates — same ids twice agree
        // regardless of any location the caller holds them at); patch-,
        // preprocessing-, version-, and track-sensitive.
        let id = |track: &str, patch: u32, preproc: &str, version: &str| {
            crate::training::training_sample_id("bank", version, track, preproc, patch)
        };
        let preproc = preprocessing_id();
        assert_eq!(id("a", 0, &preproc, "v3"), id("a", 0, &preproc, "v3"));
        assert_ne!(id("a", 0, &preproc, "v3"), id("a", 1, &preproc, "v3"));
        assert_ne!(
            id("a", 0, &preproc, "v3"),
            id("a", 0, "other-preproc", "v3")
        );
        assert_ne!(id("a", 0, &preproc, "v3"), id("a", 0, &preproc, "v4"));
        assert_ne!(id("a", 0, &preproc, "v3"), id("b", 0, &preproc, "v3"));
    }

    // -- end-to-end golden (3 s sine: 188 frames, 1 patch) -----------------------------

    #[test]
    fn end_to_end_single_patch_golden() {
        let network = reference_network(REFERENCE_SEED);
        let bytes = sine_16k(3);
        let trace = run_sample(
            &bytes,
            "bank",
            "v3",
            "sine-3s",
            0,
            &preprocessing_id(),
            &network,
        )
        .expect("golden sample runs");
        assert_eq!(trace.patch_index, 0);
        assert!(trace.loss.is_finite());
        assert!(trace.to_string().contains("loss: "));
        // Intermediate check 1: patch digest matches an independent
        // ingest→frontend→layout computation (same modules, separate
        // call path from the sample runner).
        let (input, _) = crate::ingest::ingest_bytes(&bytes, 512).unwrap();
        let frontend = MelFrontend::process_contiguous(input.samples());
        assert_eq!(frontend.mel_frames.len(), 188);
        let patches = layout_patches(&frontend.mel_frames, PATCH_CONFIG_H0);
        assert_eq!(patches.len(), 1);
        assert_eq!(trace.patch_digest, digest_patches(&patches));
        // Intermediate check 2: loss recomputed straight from the
        // oracle over re-derived logits equals the trace loss.
        let detail = network.forward_detailed(&patches[0]).unwrap();
        let target = synthetic_target(&trace.sample_id);
        let direct = crate::training::bce_with_logits(&detail.logits, &target).unwrap();
        assert_eq!(trace.loss, direct);
        // Pinned end-to-end values (debug ≡ release ≡ MSRV verified at
        // validation time; intermediates checked above first).
        assert!((trace.loss - 0.822_640_1).abs() < 1e-6, "{}", trace.loss);
        assert_eq!(
            trace.logits_digest,
            "a65acddb988d1864cea8d21254b3875c201dff5a6925c92fa482328ed5525758"
        );
        assert_eq!(
            trace.embedding_digest,
            "0ba5e92b0d71b5584bbebfdf18472c82b410cc718960789d6cf57c41d9630d07"
        );
    }

    // -- multi-patch track -----------------------------------------------------

    #[test]
    fn multipatch_track_aggregates_and_averages_loss() {
        // 6 s sine: 96000 samples = 375 frames = 3 patches.
        let network = reference_network(REFERENCE_SEED);
        let bytes = sine_16k(6);
        let track = run_track_training(
            &bytes,
            "bank",
            "v3",
            "sine-6s",
            &preprocessing_id(),
            &network,
        )
        .expect("multi-patch track runs");
        assert_eq!(track.track_id, "sine-6s");
        assert_eq!(track.traces.len(), 3);
        assert_eq!(
            track
                .traces
                .iter()
                .map(|trace| trace.patch_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        // Aggregate equals the Slice 3 Mean over the three embeddings;
        // mean loss equals the arithmetic mean of the patch losses.
        let aggregate = track.aggregate.expect("three patches aggregate");
        assert!(aggregate.iter().all(|v| v.is_finite()));
        let mean_loss = track.mean_loss.expect("three patches score");
        assert!(mean_loss.is_finite());
        let manual =
            track.traces.iter().map(|trace| trace.loss).sum::<f32>() / track.traces.len() as f32;
        assert_eq!(mean_loss, manual);
        // Zero-patch audio keeps the null policy: no aggregate, no loss.
        let short = run_track_training(
            &wav_pcm16(16_000, &[0i16; 100]),
            "bank",
            "v3",
            "blip",
            &preprocessing_id(),
            &network,
        )
        .expect("short audio runs");
        assert!(short.traces.is_empty());
        assert_eq!(short.aggregate, None);
        assert_eq!(short.mean_loss, None);
    }

    // -- errors ---------------------------------------------------------------

    #[test]
    fn sample_errors_are_explicit() {
        let network = reference_network(REFERENCE_SEED);
        let bytes = sine_16k(3);
        // Patch index beyond the single laid-out patch.
        assert_eq!(
            run_sample(
                &bytes,
                "bank",
                "v3",
                "sine-3s",
                7,
                &preprocessing_id(),
                &network
            )
            .unwrap_err(),
            SampleError::PatchIndexOutOfRange {
                track: "sine-3s".to_string(),
                index: 7,
                patches: 1
            }
        );
        // Garbage bytes fail at ingest with the track named.
        match run_sample(
            &[0u8, 1, 2, 3],
            "bank",
            "v3",
            "garbage",
            0,
            &preprocessing_id(),
            &network,
        )
        .unwrap_err()
        {
            SampleError::Ingest { track, detail } => {
                assert_eq!(track, "garbage");
                assert!(!detail.is_empty());
            }
            other => panic!("expected ingest failure, got {other:?}"),
        }
    }

    // -- training-example identity axes -----------------------------------------------

    #[test]
    fn example_identity_separates_every_axis() {
        // Lifetimes: owned Strings live outside the closures.
        let selection = SAMPLE_SELECTION_V1.digest();
        let preproc = preprocessing_id();
        let objective = V1_OBJECTIVE.digest();
        let base = || ExampleIdentityInput {
            dataset_id: "bank",
            dataset_version: "v3",
            selection_digest: &selection,
            sample_id: "sample-9",
            preprocessing_id: &preproc,
            model_id: "sonic52-52",
            weight_digest: "weight-digest",
            objective_digest: &objective,
            target_id: SYNTHETIC_TARGET_RULE_ID,
        };
        let base_id = || training_example_id(&base());
        assert_eq!(base_id(), base_id());
        assert_eq!(base_id().len(), 64);
        let mut variant = base();
        variant.dataset_id = "other";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.dataset_version = "v4";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.selection_digest = "other-selection";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.sample_id = "sample-10";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.preprocessing_id = "other-preproc";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.model_id = "sonic52-128";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.weight_digest = "other-weights";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.objective_digest = "other-objective";
        assert_ne!(base_id(), training_example_id(&variant));
        let mut variant = base();
        variant.target_id = "other-rule";
        assert_ne!(base_id(), training_example_id(&variant));
    }

    // -- trace display ----------------------------------------------------------

    #[test]
    fn trace_display_carries_no_paths_or_host_facts() {
        let network = reference_network(REFERENCE_SEED);
        let trace = run_sample(
            &sine_16k(3),
            "bank",
            "v3",
            "sine-3s",
            0,
            &preprocessing_id(),
            &network,
        )
        .unwrap();
        let text = trace.to_string();
        for field in [
            "dataset: bank@v3",
            "track: sine-3s",
            "patch: 0",
            "network: musicpack-similarity-sonic52-reference-v1",
            "target: synthetic-rule-v1",
        ] {
            assert!(text.contains(field), "{field}");
        }
        assert!(!text.contains("/"));
    }
}
