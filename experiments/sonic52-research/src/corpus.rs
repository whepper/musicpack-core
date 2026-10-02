//! Sonic52 Slice 5: corpus-scale research runner.
//!
//! Exercises the complete v1 pipeline one track at a time —
//! ingest → H0 frontend → v1 layout → reference network → Mean
//! aggregation — over a small pinned fixture corpus plus synthetic
//! sanity tracks, recording a deterministic manifest with digests,
//! stride-ablation statistics, and diagnostic timings.
//!
//! ```text
//! pinned corpus (sorted by stable id, one track at a time)
//!   → run_experiment (Slice 3/4 canonical implementations, reused)
//!   → TrackRecord { facts, counts, digests, timings }
//!   → CorpusManifest { per-track records, stats, collection digest }
//! ```
//!
//! Reference-vector digests here demonstrate reproducible plumbing,
//! not embedding quality: the reference weights are synthetic. No
//! model-quality metrics, no Discogs-EffNet comparison, no production
//! integration. Timings use a monotonic clock and are diagnostic only —
//! they never enter any digest.

#![forbid(unsafe_code)]

use std::fmt;
use std::time::Instant;

use crate::experiment::{run_experiment, Aggregation, ExperimentConfig};
use crate::network::{
    aggregate_reference_vectors, reference_network, Sonic52ReferenceNetwork, REFERENCE_SEED,
};

// ---------------------------------------------------------------------
// reference-network identity (research-only, never a model release)
// ---------------------------------------------------------------------

/// Research identity of the Slice 4 reference network: topology +
/// weight scheme + experiment contract generation. Displayed in every
/// manifest; never a production `SimilarityProfile` id.
pub const SONIC52_REFERENCE_NETWORK_ID: &str = "musicpack-similarity-sonic52-reference-v1";

/// Deterministic digest over every reference weight and bias, in fixed
/// order: conv weights, conv bias, dense-200 weights, dense-200 bias,
///
/// dense-52 weights, dense-52 bias (little-endian bytes). The per-track
/// digests are meaningless if these silently change, so the manifest
/// pins both the network id and this digest. Reference weights are
/// synthetic formulas, not trained weights.
pub fn reference_weight_digest(network: &Sonic52ReferenceNetwork) -> String {
    let mut bytes = Vec::new();
    for value in network
        .conv_weights
        .iter()
        .chain(network.conv_bias.iter())
        .chain(network.dense_hidden.weights.iter())
        .chain(network.dense_hidden.bias.iter())
        .chain(network.dense_output.weights.iter())
        .chain(network.dense_output.bias.iter())
    {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    musicpack_core::format::checksum::sha256_hex(&bytes)
}

/// The canonical reference network instance for corpus runs.
pub fn corpus_reference_network() -> Sonic52ReferenceNetwork {
    reference_network(REFERENCE_SEED)
}

// ---------------------------------------------------------------------
// corpus definition: pinned tracks, sorted by stable id
// ---------------------------------------------------------------------

/// Supported container extensions (ASCII-lowercase match). Anything
/// else is an explicit per-track discovery failure, never silent.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "flac", "mpc"];

/// One pinned corpus track: stable id plus repo-relative path. The id
/// (not the path, not filesystem order) is the identity used in
/// digests, so checkouts at different absolute locations agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusTrack {
    /// Stable identifier, unique within a corpus.
    pub id: &'static str,
    /// Repo-relative path (e.g. `fixtures/reference/audio/x.wav`).
    pub path: &'static str,
}

/// The pinned real-audio corpus: small, multi-format, deliberately
/// sub-patch except where noted (patch-bearing real coverage comes
/// from the spliced case in Slice 2/3 and the synthetic long case
/// below). Read-only reuse; no fixtures copied, no music added.
pub const REAL_CORPUS: &[CorpusTrack] = &[
    CorpusTrack {
        id: "flac-mono-44k",
        path: "fixtures/reference/audio/flac-mono-44k.flac",
    },
    CorpusTrack {
        id: "flac24-96k",
        path: "fixtures/reference/audio/flac24-96k.flac",
    },
    CorpusTrack {
        id: "mpc-short-head",
        path: "crates/musicpack-mpc-tools/tests/data/cut/short-head.mpc",
    },
    CorpusTrack {
        id: "mpc-multi-full",
        path: "crates/musicpack-mpc-tools/tests/data/cut/multi-full.mpc",
    },
    CorpusTrack {
        id: "wav16-44k",
        path: "fixtures/reference/audio/wav16-44k.wav",
    },
];

/// What is wrong with a corpus definition (before any audio runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusError {
    /// Two tracks share one id.
    DuplicateId {
        /// The repeated identifier.
        id: String,
    },
}

impl fmt::Display for CorpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CorpusError::DuplicateId { id } => write!(f, "duplicate corpus track id: {id}"),
        }
    }
}

impl std::error::Error for CorpusError {}

/// Sorts a corpus by stable id and rejects duplicate ids. Filesystem
/// ordering never influences processing order.
pub fn discover(mut tracks: Vec<CorpusTrack>) -> Result<Vec<CorpusTrack>, CorpusError> {
    tracks.sort_by(|a, b| a.id.cmp(b.id));
    for pair in tracks.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(CorpusError::DuplicateId {
                id: pair[0].id.to_string(),
            });
        }
    }
    Ok(tracks)
}

/// Lowercase extension of a repo-relative path, if any.
pub fn extension_of(path: &str) -> Option<String> {
    path.rsplit('.')
        .next()
        .map(|extension| extension.to_ascii_lowercase())
}

// ---------------------------------------------------------------------
// per-track result: deterministic record + diagnostic timings
// ---------------------------------------------------------------------

/// One track's deterministic result. The canonical string (see
/// [`TrackRecord::canonical`]) carries every digest-relevant field in
/// fixed order; timings are deliberately excluded.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecord {
    /// Stable track id.
    pub id: String,
    /// Lowercase container extension.
    pub format: String,
    /// Input byte size.
    pub input_bytes: usize,
    /// Source sample rate in Hz.
    pub source_rate: u32,
    /// Source channel count.
    pub source_channels: u8,
    /// Decoded source frames.
    pub decoded_frames: usize,
    /// Mono frames at 16 kHz entering the frontend.
    pub resampled_frames: usize,
    /// Mel frames out of the H0 frontend.
    pub mel_frames: usize,
    /// laid-out patch count (== reference-vector count).
    pub patch_count: usize,
    /// Aggregate dimension (52 when aggregation ran, else `None`).
    ///
    /// A track with zero patches records `None` here — an explicit
    /// "insufficient audio" outcome, never a fabricated zero vector
    /// (ADR 0017: null results must not become zero vectors).
    pub aggregate_dim: Option<usize>,
    /// SHA-256 over concatenated row-major patch bytes.
    pub patch_digest: String,
    /// SHA-256 over the 52-D aggregate bytes (`None` with no aggregate).
    pub aggregate_digest: Option<String>,
    /// Stage timings in milliseconds (diagnostic only, not digested).
    pub timings_ms: StageTimings,
}

/// Per-stage wall-clock milliseconds for one track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageTimings {
    /// Ingest: decode + sanitize + downmix + resample.
    pub ingest: f64,
    /// H0 frontend through patch layout.
    pub frontend: f64,
    /// Reference network over all patches.
    pub network: f64,
    /// Aggregation to one vector.
    pub aggregation: f64,
}

impl StageTimings {
    /// Sum of stages (per-track pipeline time).
    pub fn total(&self) -> f64 {
        self.ingest + self.frontend + self.network + self.aggregation
    }
}

/// Explicit per-track failure: stage, detail, and the id that failed.
/// A bad fixture is recorded, never swallowed, and never aborts its
/// neighbours (whether the corpus as a whole passes is the caller's
/// assertion to make — see the tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackFailure {
    /// Stable track id.
    pub id: String,
    /// Pipeline stage that failed (`discover`, `ingest`, `network`…).
    pub stage: String,
    /// Human-readable cause.
    pub detail: String,
}

/// One track's outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackOutcome {
    /// Deterministic success record.
    Ok(TrackRecord),
    /// Explicit recorded failure.
    Failed(TrackFailure),
}

impl TrackRecord {
    /// Canonical deterministic representation: fixed field order, no
    /// paths, no timings, no host facts. Same input + same contract +
    /// same reference network ⇒ same string.
    pub fn canonical(
        &self,
        contract: &ExperimentConfig,
        network_id: &str,
        weight_digest: &str,
    ) -> String {
        format!(
            "v1|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}",
            self.id,
            self.format,
            self.input_bytes,
            self.source_rate,
            self.source_channels,
            self.decoded_frames,
            self.resampled_frames,
            self.mel_frames,
            self.patch_count,
            contract.patch_stride,
            contract.patch_frames,
            match contract.boundary {
                crate::experiment::BoundaryPolicy::Discard => "discard",
                crate::experiment::BoundaryPolicy::ZeroPad => "zeropad",
                crate::experiment::BoundaryPolicy::Repeat => "repeat",
                crate::experiment::BoundaryPolicy::Reflect => "reflect",
            },
            contract.aggregation,
            network_id,
            weight_digest,
            self.patch_digest,
            match (&self.aggregate_dim, &self.aggregate_digest) {
                (Some(dim), Some(digest)) => format!("{dim}:{digest}"),
                _ => "none".to_string(),
            }
        )
    }
}

// ---------------------------------------------------------------------
// the runner: one track at a time, materialization reported
// ---------------------------------------------------------------------

/// Runs one pinned track's bytes through the v1 pipeline under
/// `config` (stride/boundary/aggregation may vary per ablation arm;
/// DSP stays H0). Materialization per track: decoded PCM + mono +
/// patches + vectors are held while the track runs and released before
/// the next track starts — the runner never holds the corpus. Counts
/// below report the peak materialization observed.
pub fn run_track(
    id: &str,
    audio_bytes: &[u8],
    config: &ExperimentConfig,
    network: &Sonic52ReferenceNetwork,
) -> TrackOutcome {
    let ingest_started = Instant::now();
    let (input, facts) =
        match crate::ingest::ingest_bytes(audio_bytes, crate::ingest::DEFAULT_READ_FRAMES) {
            Ok(pair) => pair,
            Err(error) => {
                return TrackOutcome::Failed(TrackFailure {
                    id: id.to_string(),
                    stage: "ingest".to_string(),
                    detail: error.to_string(),
                });
            }
        };
    let ingest_ms = ingest_started.elapsed().as_secs_f64() * 1000.0;
    let frontend_started = Instant::now();
    let frontend = crate::frontend::MelFrontend::process_contiguous(input.samples());
    let patch_config = crate::experiment::PatchConfig {
        patch_frames: config.patch_frames,
        stride: config.patch_stride,
        boundary: config.boundary,
    };
    let patches = crate::experiment::layout_patches(&frontend.mel_frames, patch_config);
    let frontend_ms = frontend_started.elapsed().as_secs_f64() * 1000.0;
    let network_started = Instant::now();
    let vectors = match network.forward_many(&patches) {
        Ok(vectors) => vectors,
        Err(error) => {
            return TrackOutcome::Failed(TrackFailure {
                id: id.to_string(),
                stage: "network".to_string(),
                detail: error.to_string(),
            });
        }
    };
    let network_ms = network_started.elapsed().as_secs_f64() * 1000.0;
    let aggregation_started = Instant::now();
    // Zero patches is a normal "insufficient audio" outcome, recorded
    // explicitly as no aggregate (see field docs) — not an error, and
    // never a fabricated zero vector.
    let aggregate = if vectors.is_empty() {
        None
    } else {
        match aggregate_reference_vectors(&vectors, config.aggregation) {
            Ok(aggregate) => aggregate,
            Err(error) => {
                return TrackOutcome::Failed(TrackFailure {
                    id: id.to_string(),
                    stage: "aggregation".to_string(),
                    detail: error.to_string(),
                });
            }
        }
    };
    let aggregation_ms = aggregation_started.elapsed().as_secs_f64() * 1000.0;
    let (aggregate_dim, aggregate_digest) = match aggregate {
        Some(values) => {
            let digest = musicpack_core::format::checksum::sha256_hex(
                &crate::network::serialize_output(&values),
            );
            (Some(values.len()), Some(digest))
        }
        None => (None, None),
    };
    let patch_digest = crate::experiment::digest_patches(&patches);
    TrackOutcome::Ok(TrackRecord {
        id: id.to_string(),
        format: String::new(),
        input_bytes: audio_bytes.len(),
        source_rate: facts.source_rate,
        source_channels: facts.source_channels,
        decoded_frames: facts.decoded_frames,
        resampled_frames: facts.output_frames,
        mel_frames: frontend.mel_frames.len(),
        patch_count: patches.len(),
        aggregate_dim,
        patch_digest,
        aggregate_digest,
        timings_ms: StageTimings {
            ingest: ingest_ms,
            frontend: frontend_ms,
            network: network_ms,
            aggregation: aggregation_ms,
        },
    })
}

/// Cross-check entry: runs the same bytes through the Slice 3
/// `run_experiment` API with aggregation disabled and asserts the
/// patch-level pieces agree with `run_track` (patch count, patch
/// digest). There is one canonical patch implementation; this
/// test-visible hook proves the runner uses it rather than duplicating
/// it. Aggregation is excluded here on purpose: the harness and the
/// runner share `apply_aggregation` by construction, and empty-vector
/// behaviour is covered by the aggregation unit tests.
pub fn cross_check_slice3(
    audio_bytes: &[u8],
    config: &ExperimentConfig,
    record: &TrackRecord,
) -> Result<(), String> {
    let patch_only = ExperimentConfig {
        aggregation: Aggregation::None,
        ..*config
    };
    let report = run_experiment(&patch_only, audio_bytes).map_err(|error| error.to_string())?;
    if report.patch_count != record.patch_count {
        return Err(format!(
            "patch count diverged: runner {} vs harness {}",
            record.patch_count, report.patch_count
        ));
    }
    if report.patch_digest != record.patch_digest {
        return Err("patch digest diverged from the Slice 3 harness".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------
// collection manifest
// ---------------------------------------------------------------------

/// Whole-corpus deterministic manifest. Timings are reported but never
/// digested; the collection digest covers the sorted canonical track
/// records plus contract and network identity.
#[derive(Debug, Clone, PartialEq)]
pub struct CorpusManifest {
    /// Stable textual experiment configuration.
    pub contract: String,
    /// Patch stride of this run.
    pub stride: usize,
    /// Reference-network research identity.
    pub network_id: String,
    /// Digest over all reference weights/biases.
    pub weight_digest: String,
    /// Per-track outcomes in id order.
    pub tracks: Vec<TrackOutcome>,
    /// Successful track count.
    pub succeeded: usize,
    /// Failed track count.
    pub failed: usize,
    /// Total laid-out patches (== total reference vectors).
    pub total_patches: usize,
    /// Minimum patches on one successful track.
    pub min_patches: usize,
    /// Maximum patches on one successful track.
    pub max_patches: usize,
    /// Mean patches over successful tracks.
    pub mean_patches: f64,
    /// Peak patches held for one track.
    pub max_patches_one_track: usize,
    /// Peak vectors held for one track.
    pub max_vectors_one_track: usize,
    /// Total patch bytes processed (patches × 71,808).
    pub patch_bytes_total: usize,
    /// Total reference-vector bytes (vectors × 208).
    pub vector_bytes_total: usize,
    /// Sum of per-track pipeline times in milliseconds.
    pub total_time_ms: f64,
    /// Deterministic collection digest (no timings inside).
    pub collection_digest: String,
}

impl CorpusManifest {
    /// Canonical deterministic representation (digested, not displayed).
    pub fn canonical(&self, contract: &ExperimentConfig) -> String {
        let mut out = format!(
            "corpus-v1|{contract}|{}|{}|{}|{}",
            self.network_id, self.weight_digest, self.succeeded, self.failed
        );
        for outcome in &self.tracks {
            match outcome {
                TrackOutcome::Ok(record) => {
                    out.push_str("\nok|");
                    out.push_str(&record.canonical(
                        contract,
                        &self.network_id,
                        &self.weight_digest,
                    ));
                }
                TrackOutcome::Failed(failure) => {
                    out.push_str("\nfailed|");
                    out.push_str(&failure.id);
                    out.push('|');
                    out.push_str(&failure.stage);
                    out.push('|');
                    out.push_str(&failure.detail);
                }
            }
        }
        out
    }
}

/// Runs every track of a discovered corpus under `config`, in id
/// order, one track at a time. `loader` maps a track to its bytes
/// (file reads in tests; the runner itself is I/O-agnostic).
pub fn run_corpus(
    tracks: &[CorpusTrack],
    config: &ExperimentConfig,
    network: &Sonic52ReferenceNetwork,
    loader: &dyn Fn(&CorpusTrack) -> Result<Vec<u8>, String>,
) -> CorpusManifest {
    let weight_digest = reference_weight_digest(network);
    let mut outcomes = Vec::with_capacity(tracks.len());
    for track in tracks {
        let extension = extension_of(track.path).unwrap_or_default();
        if !SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
            outcomes.push(TrackOutcome::Failed(TrackFailure {
                id: track.id.to_string(),
                stage: "discover".to_string(),
                detail: format!("unsupported extension in {}", track.path),
            }));
            continue;
        }
        let bytes = match loader(track) {
            Ok(bytes) => bytes,
            Err(detail) => {
                outcomes.push(TrackOutcome::Failed(TrackFailure {
                    id: track.id.to_string(),
                    stage: "load".to_string(),
                    detail,
                }));
                continue;
            }
        };
        match run_track(track.id, &bytes, config, network) {
            TrackOutcome::Ok(mut record) => {
                record.format = extension;
                outcomes.push(TrackOutcome::Ok(record));
            }
            failed => outcomes.push(failed),
        }
    }
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut total_patches = 0usize;
    let mut min_patches = usize::MAX;
    let mut max_patches = 0usize;
    let mut total_time_ms = 0.0;
    for outcome in &outcomes {
        if let TrackOutcome::Ok(record) = outcome {
            succeeded += 1;
            total_patches += record.patch_count;
            min_patches = min_patches.min(record.patch_count);
            max_patches = max_patches.max(record.patch_count);
            total_time_ms += record.timings_ms.total();
        } else {
            failed += 1;
        }
    }
    if succeeded == 0 {
        min_patches = 0;
    }
    let mean_patches = if succeeded > 0 {
        total_patches as f64 / succeeded as f64
    } else {
        0.0
    };
    let manifest = CorpusManifest {
        contract: config.to_string(),
        stride: config.patch_stride,
        network_id: SONIC52_REFERENCE_NETWORK_ID.to_string(),
        weight_digest: weight_digest.clone(),
        tracks: outcomes,
        succeeded,
        failed,
        total_patches,
        min_patches,
        max_patches,
        mean_patches,
        max_patches_one_track: max_patches,
        max_vectors_one_track: max_patches,
        patch_bytes_total: total_patches * crate::frontend::PATCH_F32LE_BYTES,
        vector_bytes_total: total_patches * 208,
        total_time_ms,
        collection_digest: String::new(),
    };
    let digest =
        musicpack_core::format::checksum::sha256_hex(manifest.canonical(config).as_bytes());
    CorpusManifest {
        collection_digest: digest,
        ..manifest
    }
}

/// Per-stride ablation statistics (patch_frames, boundary, and
/// aggregation held constant by the caller).
#[derive(Debug, Clone, PartialEq)]
pub struct StrideStats {
    /// Patch stride of this arm.
    pub stride: usize,
    /// Successful tracks.
    pub tracks: usize,
    /// Total laid-out patches.
    pub total_patches: usize,
    /// Minimum patches on one track.
    pub min_patches: usize,
    /// Maximum patches on one track.
    pub max_patches: usize,
    /// Mean patches per track.
    pub mean_patches: f64,
    /// Total reference vectors (== total patches).
    pub total_vectors: usize,
    /// Collection digest of this arm.
    pub collection_digest: String,
    /// Sum of per-track pipeline times in milliseconds.
    pub total_time_ms: f64,
}

/// Runs the corpus once per stride in `strides`, holding every other
/// configuration field constant. Digest/timing differences between arms
/// reflect layout coverage only — the reference weights are synthetic,
/// so no arm says anything about model quality.
pub fn stride_ablation(
    tracks: &[CorpusTrack],
    base: &ExperimentConfig,
    strides: &[usize],
    network: &Sonic52ReferenceNetwork,
    loader: &dyn Fn(&CorpusTrack) -> Result<Vec<u8>, String>,
) -> Vec<(StrideStats, CorpusManifest)> {
    let mut arms = Vec::with_capacity(strides.len());
    for stride in strides {
        let config = ExperimentConfig {
            patch_stride: *stride,
            ..*base
        };
        let manifest = run_corpus(tracks, &config, network, loader);
        arms.push((
            StrideStats {
                stride: *stride,
                tracks: manifest.succeeded,
                total_patches: manifest.total_patches,
                min_patches: manifest.min_patches,
                max_patches: manifest.max_patches,
                mean_patches: manifest.mean_patches,
                total_vectors: manifest.total_patches,
                collection_digest: manifest.collection_digest.clone(),
                total_time_ms: manifest.total_time_ms,
            },
            manifest,
        ));
    }
    arms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::RESEARCH_V1_CONTRACT;

    fn repo_path(relative: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative)
    }

    fn file_loader(track: &CorpusTrack) -> Result<Vec<u8>, String> {
        std::fs::read(repo_path(track.path)).map_err(|e| e.to_string())
    }

    fn discovered_real_corpus() -> Vec<CorpusTrack> {
        discover(REAL_CORPUS.to_vec()).expect("pinned corpus is valid")
    }

    // -- synthetic WAV bytes (test-only, hand-rolled) --------------------------

    fn wav_pcm16(rate: u32, channels: u16, frames: &[Vec<i16>]) -> Vec<u8> {
        let count = frames[0].len();
        let block = channels as usize * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + count * block) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&((rate as usize * block) as u32).to_le_bytes());
        out.extend_from_slice(&(block as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&((count * block) as u32).to_le_bytes());
        for i in 0..count {
            for channel in frames {
                out.extend_from_slice(&channel[i].to_le_bytes());
            }
        }
        out
    }

    fn sine_i16(rate: u32, seconds: u32, freq: f32, amplitude: f32) -> Vec<i16> {
        (0..(rate * seconds) as usize)
            .map(|i| {
                (amplitude
                    * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin()
                    * 32767.0) as i16
            })
            .collect()
    }

    // -- discovery ---------------------------------------------------------

    #[test]
    fn discovery_sorts_and_rejects_duplicates() {
        let tracks = discover(vec![
            CorpusTrack {
                id: "b",
                path: "b.wav",
            },
            CorpusTrack {
                id: "a",
                path: "a.flac",
            },
            CorpusTrack {
                id: "c",
                path: "c.mpc",
            },
        ])
        .unwrap();
        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            discover(vec![
                CorpusTrack {
                    id: "a",
                    path: "a.wav"
                },
                CorpusTrack {
                    id: "a",
                    path: "other.wav"
                },
            ])
            .unwrap_err(),
            CorpusError::DuplicateId {
                id: "a".to_string()
            }
        );
        assert_eq!(extension_of("x.WAV"), Some("wav".to_string()));
        assert_eq!(
            extension_of("no-extension"),
            Some("no-extension".to_string())
        );
    }

    #[test]
    fn unsupported_files_fail_explicitly() {
        let network = corpus_reference_network();
        let manifest = run_corpus(
            &discover(vec![
                CorpusTrack {
                    id: "good",
                    path: "good.wav",
                },
                CorpusTrack {
                    id: "bad",
                    path: "notes.txt",
                },
            ])
            .unwrap(),
            &RESEARCH_V1_CONTRACT,
            &network,
            &|track| {
                if track.id == "good" {
                    Ok(wav_pcm16(16_000, 1, &[vec![0i16; 1000]]))
                } else {
                    Ok(vec![0u8; 8])
                }
            },
        );
        assert_eq!(manifest.succeeded, 1);
        assert_eq!(manifest.failed, 1);
        match &manifest.tracks[0] {
            TrackOutcome::Failed(failure) => {
                assert_eq!(failure.id, "bad");
                assert_eq!(failure.stage, "discover");
            }
            TrackOutcome::Ok(_) => panic!("bad track must fail"),
        }
        // Sorted order: "bad" sorts before "good".
        assert!(matches!(&manifest.tracks[1], TrackOutcome::Ok(_)));
    }

    // -- identity ----------------------------------------------------------

    #[test]
    fn reference_identity_is_stable() {
        assert_eq!(
            SONIC52_REFERENCE_NETWORK_ID,
            "musicpack-similarity-sonic52-reference-v1"
        );
        let network = corpus_reference_network();
        let first = reference_weight_digest(&network);
        assert_eq!(first.len(), 64);
        assert_eq!(first, reference_weight_digest(&corpus_reference_network()));
        // A different seed changes the weights (the digest binds them).
        let other = crate::network::reference_network(crate::network::REFERENCE_SEED + 1);
        assert_ne!(first, reference_weight_digest(&other));
    }

    // -- synthetic sanity corpus -------------------------------------------

    fn synthetic_corpus() -> Vec<(&'static str, Vec<u8>)> {
        // Silence, DC, impulse, sine, and stereo with a known L/R
        // relationship (L = 2R exactly, so mono must equal 1.5R).
        let dc = vec![8192i16; 48_000];
        let mut impulse = vec![0i16; 48_000];
        impulse[24_000] = 16384;
        let stereo_left: Vec<i16> = sine_i16(16_000, 3, 440.0, 0.5);
        let stereo_right: Vec<i16> = stereo_left.iter().map(|v| v / 2).collect();
        vec![
            ("silence", wav_pcm16(16_000, 1, &[vec![0i16; 48_000]])),
            ("dc", wav_pcm16(16_000, 1, &[dc])),
            ("impulse", wav_pcm16(16_000, 1, &[impulse])),
            (
                "sine",
                wav_pcm16(16_000, 1, &[sine_i16(16_000, 3, 440.0, 0.5)]),
            ),
            ("stereo", wav_pcm16(16_000, 2, &[stereo_left, stereo_right])),
        ]
    }

    #[test]
    fn synthetic_sanity_corpus_completes() {
        let network = corpus_reference_network();
        for (id, bytes) in synthetic_corpus() {
            let outcome = run_track(id, &bytes, &RESEARCH_V1_CONTRACT, &network);
            let TrackOutcome::Ok(record) = outcome else {
                panic!("synthetic track {id} must succeed");
            };
            // 3 s @16 kHz = 48000 samples = 188 frames = 1 patch.
            assert_eq!(record.patch_count, 1, "{id}");
            assert_eq!(record.aggregate_dim, Some(52), "{id}");
            let digest = record
                .aggregate_digest
                .as_deref()
                .expect("aggregate digest");
            assert_eq!(digest.len(), 64);
            assert!(record.timings_ms.total() >= 0.0);
        }
        // Stereo mixing check, replicated exactly: integer PCM halves
        // (Rust truncating division, like the fixture builder), f32
        // scaling by 2^-15 (like the decoder), f64 mean (like downmix).
        let stereo = &synthetic_corpus()[4].1;
        let (input, _) = crate::ingest::ingest_bytes(stereo, 4096).unwrap();
        let left: Vec<i16> = sine_i16(16_000, 3, 440.0, 0.5);
        for (mono, l) in input.samples().iter().zip(left.iter()) {
            let r = l / 2;
            let expected =
                ((f64::from(*l as f32 / 32768.0) + f64::from(r as f32 / 32768.0)) / 2.0) as f32;
            assert_eq!(*mono, expected, "{mono} vs {expected}");
        }
        // Silence aggregate is network-defined, not zeros: biases
        // propagate through every layer, so the pinned digest below is
        // a determinism anchor for the full path, not a claim about
        // "silence means nothing". (A zero aggregate here would have
        // been a fabricated null — explicitly not what this records.)
        let silence = run_track(
            "silence",
            &synthetic_corpus()[0].1,
            &RESEARCH_V1_CONTRACT,
            &network,
        );
        let TrackOutcome::Ok(record) = silence else {
            panic!("silence must succeed");
        };
        assert_eq!(
            record.aggregate_digest.as_deref(),
            Some("dc84bb3dbeede9cd468c28fbc07df6a53b96dfe32698ead8932eaeaa487f3819")
        );
    }

    #[test]
    fn long_synthetic_sine_is_multipatch() {
        // 6 s mono sine: 96000 samples = 375 frames = 3 patches at
        // stride 93 — the multi-patch plumbing path on real WAV bytes.
        let network = corpus_reference_network();
        let bytes = wav_pcm16(16_000, 1, &[sine_i16(16_000, 6, 440.0, 0.5)]);
        let outcome = run_track("sine-6s", &bytes, &RESEARCH_V1_CONTRACT, &network);
        let TrackOutcome::Ok(record) = outcome else {
            panic!("long sine must succeed");
        };
        assert_eq!(record.patch_count, 3);
        assert_eq!(record.aggregate_dim, Some(52));
    }

    // -- real corpus manifest ----------------------------------------------------

    #[test]
    fn real_corpus_manifest_is_deterministic() {
        let tracks = discovered_real_corpus();
        // Sorted by stable id, independent of the declared order.
        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![
                "flac-mono-44k",
                "flac24-96k",
                "mpc-multi-full",
                "mpc-short-head",
                "wav16-44k"
            ]
        );
        let network = corpus_reference_network();
        let first = run_corpus(&tracks, &RESEARCH_V1_CONTRACT, &network, &file_loader);
        let second = run_corpus(&tracks, &RESEARCH_V1_CONTRACT, &network, &file_loader);
        assert_eq!(first.succeeded, 5);
        assert_eq!(first.failed, 0);
        // Twice-run identity over every deterministic field (timings
        // excluded by construction — compare canonical strings).
        assert_eq!(
            first.canonical(&RESEARCH_V1_CONTRACT),
            second.canonical(&RESEARCH_V1_CONTRACT)
        );
        // Materialization accounting is consistent.
        assert_eq!(first.max_patches_one_track, first.max_patches);
        assert_eq!(first.max_vectors_one_track, first.max_patches);
        assert_eq!(first.patch_bytes_total, first.total_patches * 71_808);
        assert_eq!(first.vector_bytes_total, first.total_patches * 208);
        assert!(first.total_time_ms >= 0.0);
    }

    #[test]
    fn real_corpus_collection_digest_pending() {
        let tracks = discovered_real_corpus();
        let network = corpus_reference_network();
        let manifest = run_corpus(&tracks, &RESEARCH_V1_CONTRACT, &network, &file_loader);
        // Per-track facts pinned alongside the collection digest so a
        // drift is diagnosable without recomputing history.
        let mut summary = Vec::new();
        for outcome in &manifest.tracks {
            match outcome {
                TrackOutcome::Ok(record) => summary.push(format!(
                    "{}:{}:{}:{}:{}:{}",
                    record.id,
                    record.format,
                    record.decoded_frames,
                    record.resampled_frames,
                    record.patch_count,
                    record.aggregate_digest.as_deref().unwrap_or("none")
                )),
                TrackOutcome::Failed(failure) => {
                    summary.push(format!("{}:FAILED:{}", failure.id, failure.stage))
                }
            }
        }
        eprintln!("CORPUS_SUMMARY {summary:?}");
        // Pinned collection digest (debug ≡ release ≡ MSRV verified at
        // validation time). Covers the five rows above plus contract
        // and reference-weight identity.
        assert_eq!(
            manifest.collection_digest,
            "b148f826af247b352925ceadca65af0249d6eb81fff3e46cab269a62ae2b57cc"
        );
    }

    // -- stride ablation -----------------------------------------------------

    #[test]
    fn stride_ablation_counts_and_digests() {
        let tracks = discovered_real_corpus();
        let network = corpus_reference_network();
        let arms = stride_ablation(
            &tracks,
            &RESEARCH_V1_CONTRACT,
            &[46, 93, 187],
            &network,
            &file_loader,
        );
        assert_eq!(arms.len(), 3);
        // The pinned 5-track corpus is sub-patch everywhere (see Slice
        // 2/3 corpus rows), so every arm yields zero patches — and the
        // three collection digests pin exactly that layout fact (plus
        // contract/weight identity, which differ per stride).
        for (stats, manifest) in &arms {
            assert_eq!(stats.total_patches, 0);
            assert_eq!(stats.total_vectors, 0);
            assert_eq!(manifest.succeeded, 5);
        }
        assert_eq!(
            arms[0].1.collection_digest,
            "979b32f7ffc3e9a908e156640b0db3df49bf80741427bf2c067e86f1ccef909f"
        );
        assert_eq!(
            arms[1].1.collection_digest,
            "b148f826af247b352925ceadca65af0249d6eb81fff3e46cab269a62ae2b57cc"
        );
        assert_eq!(
            arms[2].1.collection_digest,
            "a5ca1233ee6c1116eef12148a75554ce190d2b2c4649521368ee656f6d8904dc"
        );
    }

    // -- cross-checks ------------------------------------------------------

    #[test]
    fn corpus_runner_matches_slice3_harness() {
        // Same bytes through run_experiment (Slice 3 API): patch count
        // and digest must agree — one canonical implementation.
        let tracks = discovered_real_corpus();
        let network = corpus_reference_network();
        let manifest = run_corpus(&tracks, &RESEARCH_V1_CONTRACT, &network, &file_loader);
        for (track, outcome) in tracks.iter().zip(manifest.tracks.iter()) {
            let bytes = file_loader(track).unwrap();
            match outcome {
                TrackOutcome::Ok(record) => {
                    cross_check_slice3(&bytes, &RESEARCH_V1_CONTRACT, record).unwrap();
                }
                TrackOutcome::Failed(failure) => panic!("fixture failed: {failure:?}"),
            }
        }
    }

    #[test]
    fn failing_fixture_is_recorded_not_swallowed() {
        let network = corpus_reference_network();
        let manifest = run_corpus(
            &discover(vec![CorpusTrack {
                id: "garbage",
                path: "garbage.wav",
            }])
            .unwrap(),
            &RESEARCH_V1_CONTRACT,
            &network,
            &|_| Ok(vec![0u8, 1, 2, 3]),
        );
        assert_eq!(manifest.succeeded, 0);
        assert_eq!(manifest.failed, 1);
        match &manifest.tracks[0] {
            TrackOutcome::Failed(failure) => {
                assert_eq!(failure.id, "garbage");
                assert!(!failure.detail.is_empty());
            }
            TrackOutcome::Ok(_) => panic!("garbage must fail"),
        }
        // A failing track still yields a deterministic digest.
        assert_eq!(manifest.collection_digest.len(), 64);
    }
}
