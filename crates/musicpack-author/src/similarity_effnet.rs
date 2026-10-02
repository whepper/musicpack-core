//! Discogs-EffNet similarity profiles: the first concrete
//! [`SimilarityProducer`](crate::similarity::SimilarityProducer)
//! implementations.
//!
//! Two distinct profiles, never mixed (vectors from different fingerprints
//! must never share an index):
//!
//! - [`multi_profile`] — 1280-D (`musicpack-similarity-discogs-effnet-multi-v1`);
//! - [`release_profile`] — 512-D (`musicpack-similarity-discogs-effnet-release-v1`).
//!
//! # Licensing posture (read before enabling)
//!
//! Discogs-EffNet is **not legally cleared**. The research established:
//! conflicting license notices on MTG's public model pages; an unconfirmed
//! artifact-level governing license; unresolved status of generated
//! embeddings/vectors; no commercial clearance; no redistribution clearance
//! for weights. Consequences enforced by this module:
//!
//! - Model weights are **operator-supplied**: [`EffNetProducer`] takes a
//!   filesystem path, verifies its SHA-256 before opening it, and never
//!   downloads, fetches, or resolves anything over the network.
//! - Nothing here claims any model is commercially usable, open source, or
//!   unrestricted, and no vector/output license is invented. Users are
//!   responsible for complying with the applicable model license.
//! - The profile is optional and experimental until G-1…G-4 are resolved by
//!   qualified review. Similarity itself stays optional end to end.
//! - G-7 (human listening review) is closed as a technical release gate
//!   (ADR 0017 §10.5): technical completeness claims nothing about musical
//!   usefulness, and MusicPack does not claim to have independently validated
//!   the model's scientific quality. Optional human listening may still be
//!   performed as product-quality feedback.
//!
//! # Provenance
//!
//! Preprocessing, tensor layout, batching, aggregation, and normalization
//! are ported exactly from the validated experiment
//! (`experiments/music-similarity-eval`: `audio.rs`, `mel.rs`, `model.rs`,
//! `eval.rs`; REPORT.md model table). Constants below cite their evidence.
//! The experiment crate is deliberately not a dependency (isolation
//! policy); this is a port, and the port is verified by the regression
//! procedure documented on [`EffNetProducer`].
//!
//! # Precision and determinism
//!
//! Output is deterministic `f32le`, L2-normalized (G-6 closed as KEEP
//! F32LE — there is no `f16` path here). Single rten thread, sequential
//! batches in input order, fixed accumulation order throughout. The
//! DSP port preserves the experiment's exact `f32`/`f64` mixing.
//!
//! # Toolchain note
//!
//! `rten 0.26.0` declares `rust-version 1.94` while the workspace MSRV is
//! 1.85 (G-5 PASS WITH SCOPED EXCEPTION): inference lives behind the
//! default-off `discogs-effnet` feature, so default builds never compile
//! rten and keep the 1.85 promise; enabling the feature requires Rust 1.94
//! and fails loudly otherwise. Profile metadata, DSP, writer, and cache
//! behavior carry no such requirement.

#[cfg(feature = "discogs-effnet")]
use std::cell::RefCell;
use std::f64::consts::PI;
use std::path::Path;
#[cfg(feature = "discogs-effnet")]
use std::path::PathBuf;

use musicpack_core::format::checksum;

use crate::similarity::SimilarityProfile;
#[cfg(feature = "discogs-effnet")]
use crate::similarity::{ProducerInput, TrackSimilarity};

// ---------------------------------------------------------------------
// model artifact identity (REPORT.md:20-21; FINDINGS.md model table)
// ---------------------------------------------------------------------

/// Operator-supplied multi artifact.
///
/// `discogs_multi_embeddings-effnet-bs64-1.onnx`, 15,998,047 bytes,
/// SHA-256 `65cfde30…76339d8e`. The file itself is never committed,
/// downloaded, or bundled: the operator provides it and this module
/// verifies it before opening.
pub const MULTI_MODEL_SHA256: &str =
    "65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e";
/// Raw digest bytes of [`MULTI_MODEL_SHA256`] (generated, not transcribed).
pub const MULTI_MODEL_SHA256_BYTES: [u8; 32] = [
    0x65, 0xcf, 0xde, 0x30, 0x65, 0x5a, 0x93, 0x9d, 0xe4, 0x20, 0xe5, 0xc0, 0x9a, 0x49, 0xa4, 0x36,
    0x48, 0x33, 0x6f, 0xdb, 0xbf, 0x79, 0xeb, 0x3a, 0xf6, 0xd3, 0xb4, 0x01, 0x76, 0x33, 0x9d, 0x8e,
];

/// Operator-supplied release artifact.
///
/// `discogs_release_embeddings-effnet-bs64-1.onnx`, 18,621,961 bytes,
/// SHA-256 `fb49bd4e…062530e7`. Same supply-and-verify discipline as above.
pub const RELEASE_MODEL_SHA256: &str =
    "fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7";
/// Raw digest bytes of [`RELEASE_MODEL_SHA256`] (generated, not transcribed).
pub const RELEASE_MODEL_SHA256_BYTES: [u8; 32] = [
    0xfb, 0x49, 0xbd, 0x4e, 0x90, 0x66, 0x24, 0xbb, 0xc5, 0xee, 0x09, 0xe6, 0x48, 0xe8, 0x8d, 0x53,
    0xb2, 0x8c, 0x23, 0x87, 0x0a, 0x8a, 0x6b, 0xed, 0x14, 0x56, 0x24, 0xfd, 0x06, 0x25, 0x30, 0xe7,
];

/// Expected ONNX input tensor (FINDINGS.md model table; `model.rs`).
pub const INPUT_TENSOR_NAME: &str = "serving_default_melspectrogram";
/// Input tensor shape `[batch, frames, bands]` (REPORT.md:20).
pub const INPUT_FRAMES: usize = 128;
/// Patch hop in frontend frames: 61 is profile-defining (0/45 identical
/// hashes between hop 61 and 62 runs — a different hop is a different
/// profile, not defined here).
pub const PATCH_HOP: usize = 61;
/// Fixed model batch (zero-padded, padding discarded — `model.rs`).
pub const BATCH_SIZE: usize = 64;

// ---------------------------------------------------------------------
// profile definitions (ADR 0017 §5.4; FORMAT_SPEC Appendix A, 14 tags)
// ---------------------------------------------------------------------

/// Preprocessing identity, frozen from the recorded evidence (`main.rs`
/// metadata strings, `mel.rs`/`audio.rs` constants). Every segment cites
/// observable behavior; the string is opaque downstream — what matters is
/// that it is fixed, so a preprocessing change is a new fingerprint.
pub const PREPROCESSING_VERSION: &str = "mono-16khz/rubato-0.16.2-fft1024-os2/slaney-96-unit-tri/log10-1e-30-10000/frame-512-hop-256-centered/hann-symmetric-unnorm-zero-phase/patch-128-hop-61";
/// Pooling rule (recorded `pooling` metadata verbatim).
pub const POOLING: &str = "per-window L2 mean, then track L2";
/// Normalization declaration (recorded `normalization` metadata verbatim).
pub const NORMALIZATION: &str = "L2";
/// Metric declaration.
pub const METRIC: &str = "cosine";
/// Inference runtime identity (`runtime` metadata verbatim).
pub const RUNTIME_ID: &str = "rten";
/// Inference runtime version (`runtime` metadata verbatim).
pub const RUNTIME_VERSION: &str = "0.26.0";
/// Numeric policy: single rten thread, sequential batches in input order,
/// scalar `f64` accumulation in the Author-owned stages. This names the
/// deterministic envelope this module controls; rten's internal kernels
/// are version-pinned by the runtime id/version above.
pub const NUMERIC_POLICY: &str = "rten-0.26.0-single-thread";

/// Canonical TLV field list of a profile (FORMAT_SPEC Appendix A).
#[derive(Debug, Clone)]
pub struct ProfileFields<'a> {
    /// Tag 1: stable semantic id.
    pub profile_id: &'a str,
    /// Tag 2: model family in MusicPack's namespace.
    pub model_family: &'static str,
    /// Tag 3: checkpoint variant within the family.
    pub model_variant: &'static str,
    /// Tag 4: exact weights digest (raw 32 bytes), or absent for
    /// model-free profiles.
    pub model_sha256: Option<[u8; 32]>,
    /// Tag 5: frozen preprocessing identity.
    pub preprocessing_version: &'static str,
    /// Tag 6: patch hop in frontend frames.
    pub patch_hop: u32,
    /// Tag 7: pooling rule.
    pub pooling: &'static str,
    /// Tag 8: normalization declaration.
    pub normalization: &'static str,
    /// Tag 9: metric.
    pub metric: &'static str,
    /// Tag 10: dimensions.
    pub dimensions: u32,
    /// Tag 11: output encoding.
    pub output_encoding: &'static str,
    /// Tag 13: runtime id + version, or absent for runtime-free profiles.
    pub runtime: Option<(&'static str, &'static str)>,
    /// Tag 14: numeric policy.
    pub numeric_policy: &'static str,
}

/// Field list of the multi / 1280-D profile. Tag 12 (album aggregation) is
/// absent: v1.0 defines none (FORMAT_SPEC §9).
pub fn multi_fields() -> ProfileFields<'static> {
    ProfileFields {
        profile_id: "musicpack-similarity-discogs-effnet-multi-v1",
        model_family: "discogs-effnet",
        model_variant: "multi",
        model_sha256: Some(MULTI_MODEL_SHA256_BYTES),
        preprocessing_version: PREPROCESSING_VERSION,
        patch_hop: PATCH_HOP as u32,
        pooling: POOLING,
        normalization: NORMALIZATION,
        metric: METRIC,
        dimensions: 1280,
        output_encoding: "f32le",
        runtime: Some((RUNTIME_ID, RUNTIME_VERSION)),
        numeric_policy: NUMERIC_POLICY,
    }
}

/// Field list of the release / 512-D profile. A distinct profile with a
/// distinct fingerprint: dimension collisions never imply comparability.
pub fn release_fields() -> ProfileFields<'static> {
    ProfileFields {
        profile_id: "musicpack-similarity-discogs-effnet-release-v1",
        model_family: "discogs-effnet",
        model_variant: "release",
        model_sha256: Some(RELEASE_MODEL_SHA256_BYTES),
        preprocessing_version: PREPROCESSING_VERSION,
        patch_hop: PATCH_HOP as u32,
        pooling: POOLING,
        normalization: NORMALIZATION,
        metric: METRIC,
        dimensions: 512,
        output_encoding: "f32le",
        runtime: Some((RUNTIME_ID, RUNTIME_VERSION)),
        numeric_policy: NUMERIC_POLICY,
    }
}

/// Canonical TLV encoding of a profile field list (FORMAT_SPEC Appendix A):
/// `tag:u8 || len:u32be || value`, ascending tags, absent fields emit
/// nothing, integers exactly four bytes big-endian, tag 4 the raw 32 digest
/// bytes, tag 13 `id NUL version`.
pub fn profile_tlv(fields: &ProfileFields<'_>) -> Vec<u8> {
    fn push(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
        out.push(tag);
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
    }
    let mut out = Vec::new();
    push(&mut out, 1, fields.profile_id.as_bytes());
    push(&mut out, 2, fields.model_family.as_bytes());
    push(&mut out, 3, fields.model_variant.as_bytes());
    if let Some(digest) = fields.model_sha256 {
        push(&mut out, 4, &digest);
    }
    push(&mut out, 5, fields.preprocessing_version.as_bytes());
    push(&mut out, 6, &fields.patch_hop.to_be_bytes());
    push(&mut out, 7, fields.pooling.as_bytes());
    push(&mut out, 8, fields.normalization.as_bytes());
    push(&mut out, 9, fields.metric.as_bytes());
    push(&mut out, 10, &fields.dimensions.to_be_bytes());
    push(&mut out, 11, fields.output_encoding.as_bytes());
    // Tag 12 (album aggregation): absent in v1.0 — emits nothing.
    if let Some((id, version)) = fields.runtime {
        let mut runtime = Vec::with_capacity(id.len() + 1 + version.len());
        runtime.extend_from_slice(id.as_bytes());
        runtime.push(0x00);
        runtime.extend_from_slice(version.as_bytes());
        push(&mut out, 13, &runtime);
    }
    push(&mut out, 14, fields.numeric_policy.as_bytes());
    out
}

/// Profile fingerprint: SHA-256 over the canonical TLV field list.
pub fn profile_fingerprint(fields: &ProfileFields<'_>) -> [u8; 32] {
    let bytes = profile_tlv(fields);
    let hex = checksum::sha256_hex(&bytes);
    let mut out = [0u8; 32];
    for (i, chunk) in out.iter_mut().enumerate() {
        *chunk = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex of digest");
    }
    out
}

/// The multi / 1280-D profile: exact identity plus layout for the neutral
/// boundary. Fingerprint computed from [`multi_fields`], never hardcoded.
pub fn multi_profile() -> SimilarityProfile {
    let fields = multi_fields();
    SimilarityProfile::new(
        fields.profile_id.to_string(),
        profile_fingerprint(&fields),
        1280,
    )
    .expect("multi profile definition is valid")
}

/// The release / 512-D profile. Distinct fingerprint, distinct partition.
pub fn release_profile() -> SimilarityProfile {
    let fields = release_fields();
    SimilarityProfile::new(
        fields.profile_id.to_string(),
        profile_fingerprint(&fields),
        512,
    )
    .expect("release profile definition is valid")
}

// ---------------------------------------------------------------------
// audio decode and preparation (ported from experiment `audio.rs`)
// ---------------------------------------------------------------------

/// Analysis sample rate in Hz (recorded `sample_rate` metadata verbatim).
pub const ANALYSIS_SAMPLE_RATE: u32 = 16_000;
/// Decode chunk size in frames (experiment constant).
const DECODE_CHUNK_FRAMES: usize = 4096;
/// Decoded-sample guard (experiment constant).
const MAX_SAMPLES: usize = 250_000_000;

/// Decoded, mono, 16 kHz track samples plus provenance facts.
#[derive(Debug, Clone)]
pub struct PreparedTrack {
    /// Mono samples at [`ANALYSIS_SAMPLE_RATE`].
    pub samples: Vec<f32>,
    /// Source SHA-256 of the analyzed file bytes.
    pub source_sha256: String,
    /// Source duration in seconds (pre-resample frame count over rate).
    pub duration_seconds: f64,
}

/// Decodes `path` through `musicpack-core::audio`, downmixes to mono by
/// arithmetic f64 mean, and resamples to 16 kHz when needed — exactly the
/// experiment pipeline. Errors are plain strings; the producer maps them
/// to honest per-track outcomes.
pub fn decode_and_prepare(path: &Path) -> Result<PreparedTrack, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read audio: {e}"))?;
    let source_sha256 = checksum::sha256_hex(&bytes);
    let mut decoder = musicpack_core::audio::open(Box::new(std::io::Cursor::new(bytes)))
        .map_err(|e| format!("cannot decode audio: {e}"))?;
    let info = decoder.info().clone();
    let channels = usize::from(info.channels);
    if channels == 0 {
        return Err("decoder returned zero channels".to_string());
    }
    let mut interleaved = Vec::new();
    let mut buffer = vec![0.0f32; DECODE_CHUNK_FRAMES * channels];
    loop {
        let frames = decoder
            .read_f32(&mut buffer)
            .map_err(|e| format!("cannot decode audio: {e}"))?;
        if frames == 0 {
            break;
        }
        let new_len = interleaved
            .len()
            .checked_add(frames * channels)
            .ok_or("decoded sample length overflow")?;
        if new_len > MAX_SAMPLES {
            return Err("decoded sample limit exceeded".to_string());
        }
        interleaved.extend_from_slice(&buffer[..frames * channels]);
    }
    let source_frames = (interleaved.len() / channels) as u64;
    let mono = downmix_mean(&interleaved, channels);
    let samples = if info.sample_rate == ANALYSIS_SAMPLE_RATE {
        mono
    } else {
        resample(&mono, info.sample_rate, ANALYSIS_SAMPLE_RATE)?
    };
    let duration_seconds = source_frames as f64 / f64::from(info.sample_rate.max(1));
    Ok(PreparedTrack {
        samples,
        source_sha256,
        duration_seconds,
    })
}

fn downmix_mean(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| {
            let sum: f64 = frame.iter().map(|sample| f64::from(*sample)).sum();
            (sum / channels as f64) as f32
        })
        .collect()
}

fn resample(input: &[f32], source_rate: u32, target_rate: u32) -> Result<Vec<f32>, String> {
    use rubato::{FftFixedIn, Resampler};
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if source_rate == target_rate {
        return Ok(input.to_vec());
    }
    let mut resampler =
        FftFixedIn::<f32>::new(source_rate as usize, target_rate as usize, 1024, 2, 1)
            .map_err(|e| format!("cannot create resampler: {e}"))?;
    let delay = resampler.output_delay();
    let mut output = vec![vec![0.0f32; resampler.output_frames_max()]];
    let mut result = Vec::with_capacity(
        (input.len() as f64 * f64::from(target_rate) / f64::from(source_rate)).ceil() as usize
            + delay,
    );
    let mut offset = 0usize;
    while input.len() - offset >= resampler.input_frames_next() {
        let end = offset + resampler.input_frames_next();
        let (_consumed, produced) = resampler
            .process_into_buffer(&[&input[offset..end]], &mut output, None)
            .map_err(|e| format!("cannot resample: {e}"))?;
        result.extend_from_slice(&output[0][..produced]);
        offset = end;
    }
    if offset < input.len() {
        let (_consumed, produced) = resampler
            .process_partial_into_buffer(Some(&[&input[offset..]]), &mut output, None)
            .map_err(|e| format!("cannot resample: {e}"))?;
        result.extend_from_slice(&output[0][..produced]);
    }
    if delay < result.len() {
        result.drain(..delay);
    }
    let expected =
        (input.len() as f64 * f64::from(target_rate) / f64::from(source_rate)).round() as usize;
    result.truncate(expected.min(result.len()));
    Ok(result)
}

// ---------------------------------------------------------------------
// log-mel frontend (ported from experiment `mel.rs`)
// ---------------------------------------------------------------------

/// Frontend frame size in samples (recorded `frame_size` metadata verbatim).
pub const FRAME_SIZE: usize = 512;
/// Frontend frame hop in samples (recorded `frame_hop` metadata verbatim).
pub const FRAME_HOP: usize = 256;
/// Mel band count (recorded `mel_bands` metadata verbatim).
pub const MEL_BANDS: usize = 96;
/// Model input frames per patch (recorded `patch_size` metadata verbatim).
pub const PATCH_SIZE: usize = 128;

/// Frontend output: flattened `[128, 96]` patches plus the frame count.
#[derive(Debug, Clone)]
pub struct FrontendOutput {
    /// One `PATCH_SIZE * MEL_BANDS` vector per patch, in order.
    pub patches: Vec<Vec<f32>>,
    /// Total frontend frames (diagnostic, mirrors the experiment).
    pub frame_count: usize,
}

/// Log-mel frontend with the experiment's exact constants and evaluation
/// order (Slaney bands, symmetric Hann with zero-phase rotation, 512-point
/// real FFT, power spectrum, `log10(max(1e-30, mel * 10000 + 1))`,
/// centered frames, 128-frame patches at [`PATCH_HOP`]).
#[derive(Debug, Clone)]
pub struct MelFrontend {
    filters: Vec<Vec<f32>>,
}

impl Default for MelFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl MelFrontend {
    /// Builds the filterbank. Infallible for the frozen profile (the patch
    /// hop is the profile constant [`PATCH_HOP`]).
    pub fn new() -> Self {
        Self {
            filters: build_filters(),
        }
    }

    /// Splits 16 kHz mono PCM into model patches. Empty patches means the
    /// audio is below the profile minimum (maps to `InsufficientAudio`).
    pub fn process(&self, pcm: &[f32]) -> FrontendOutput {
        let frame_count = frame_count(pcm.len());
        let mut patches = Vec::new();
        if frame_count < PATCH_SIZE {
            return FrontendOutput {
                patches,
                frame_count,
            };
        }
        let mut start = 0usize;
        while start + PATCH_SIZE <= frame_count {
            let mut patch = Vec::with_capacity(PATCH_SIZE * MEL_BANDS);
            for frame_index in start..start + PATCH_SIZE {
                patch.extend_from_slice(&self.mel_frame(pcm, frame_index));
            }
            patches.push(patch);
            start += PATCH_HOP;
        }
        FrontendOutput {
            patches,
            frame_count,
        }
    }

    fn mel_frame(&self, pcm: &[f32], frame_index: usize) -> Vec<f32> {
        let mut frame = [0.0f32; FRAME_SIZE];
        let start = frame_index as isize * FRAME_HOP as isize - (FRAME_SIZE as isize / 2);
        for (index, value) in frame.iter_mut().enumerate() {
            let source = start + index as isize;
            if source >= 0 && (source as usize) < pcm.len() {
                *value = pcm[source as usize];
            }
        }
        // Symmetric Hann, normalized=false, zeroPhase=true: for an even
        // frame the two windowed halves rotate before the FFT.
        let mut windowed = [0.0f32; FRAME_SIZE];
        for index in 0..FRAME_SIZE / 2 {
            let first = FRAME_SIZE / 2 + index;
            windowed[index] = frame[first] * hann(first);
            windowed[FRAME_SIZE / 2 + index] = frame[index] * hann(index);
        }
        let spectrum = microfft::real::rfft_512(&mut windowed);
        let mut power = [0.0f32; FRAME_SIZE / 2 + 1];
        power[0] = spectrum[0].re * spectrum[0].re;
        for index in 1..FRAME_SIZE / 2 {
            power[index] =
                spectrum[index].re * spectrum[index].re + spectrum[index].im * spectrum[index].im;
        }
        // microfft packs the Nyquist coefficient into the imaginary part of
        // the DC bin.
        power[FRAME_SIZE / 2] = spectrum[0].im * spectrum[0].im;

        let mut bands = vec![0.0f32; MEL_BANDS];
        for (band, filter) in self.filters.iter().enumerate() {
            let mut value = 0.0f64;
            for (bin, coefficient) in filter.iter().enumerate() {
                value += f64::from(power[bin]) * f64::from(*coefficient);
            }
            // TensorflowInputMusiCNN: scale=10000, shift=1, then log10.
            bands[band] = ((value * 10_000.0) + 1.0).max(1.0e-30).log10() as f32;
        }
        bands
    }
}

fn hann(index: usize) -> f32 {
    (0.5 - 0.5 * (2.0 * PI * index as f64 / (FRAME_SIZE - 1) as f64).cos()) as f32
}

fn frame_count(sample_count: usize) -> usize {
    if sample_count <= FRAME_SIZE / 2 {
        return 0;
    }
    1 + (sample_count - FRAME_SIZE / 2).div_ceil(FRAME_HOP)
}

fn build_filters() -> Vec<Vec<f32>> {
    let low_hz = 0.0f64;
    let high_hz = f64::from(ANALYSIS_SAMPLE_RATE) / 2.0;
    let low_mel = hz_to_mel_slaney(low_hz);
    let high_mel = hz_to_mel_slaney(high_hz);
    let mut frequencies = Vec::with_capacity(MEL_BANDS + 2);
    for index in 0..=MEL_BANDS + 1 {
        let mel = low_mel + (high_mel - low_mel) * index as f64 / (MEL_BANDS + 1) as f64;
        frequencies.push(mel_to_hz_slaney(mel));
    }
    let frequency_scale = high_hz / (FRAME_SIZE / 2) as f64;
    let mut filters = vec![vec![0.0f32; FRAME_SIZE / 2 + 1]; MEL_BANDS];
    for band in 0..MEL_BANDS {
        let start = frequencies[band];
        let center = frequencies[band + 1];
        let end = frequencies[band + 2];
        let first_step = center - start;
        let second_step = end - center;
        let begin = (start / frequency_scale).ceil() as usize;
        let finish = (end / frequency_scale).floor() as usize;
        let finish = finish.min(FRAME_SIZE / 2);
        for (bin, coefficient) in filters[band]
            .iter_mut()
            .enumerate()
            .take(finish + 1)
            .skip(begin)
        {
            let frequency = bin as f64 * frequency_scale;
            let weight = if frequency < center {
                (frequency - start) / first_step
            } else {
                (end - frequency) / second_step
            };
            *coefficient = weight as f32;
        }
        // Essentia's unit_tri normalization divides by the theoretical
        // triangle area, not the sum of the discrete-bin weights.
        let normalization = (first_step + second_step) / 2.0;
        for coefficient in filters[band].iter_mut().take(finish + 1).skip(begin) {
            *coefficient /= normalization as f32;
        }
    }
    filters
}

fn hz_to_mel_slaney(hz: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN_SLOPE: f64 = 3.0 / 200.0;
    if hz < MIN_LOG_HZ {
        hz * LIN_SLOPE
    } else {
        let min_log_mel = MIN_LOG_HZ * LIN_SLOPE;
        let log_step = (6.4f64).ln() / 27.0;
        min_log_mel + (hz / MIN_LOG_HZ).ln() / log_step
    }
}

fn mel_to_hz_slaney(mel: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN_SLOPE: f64 = 3.0 / 200.0;
    let min_log_mel = MIN_LOG_HZ * LIN_SLOPE;
    if mel < min_log_mel {
        mel / LIN_SLOPE
    } else {
        let log_step = (6.4f64).ln() / 27.0;
        MIN_LOG_HZ * ((mel - min_log_mel) * log_step).exp()
    }
}

// ---------------------------------------------------------------------
// window pooling (ported from experiment `eval.rs`)
// ---------------------------------------------------------------------

/// L2 norm with `f64` accumulation, returned as `f32` (experiment-exact).
pub fn norm(values: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    for value in values {
        sum += f64::from(*value) * f64::from(*value);
    }
    sum.sqrt() as f32
}

/// Pools per-window embeddings into one L2-normalized track vector:
/// per-window L2, arithmetic mean, then track L2 (recorded `pooling` and
/// `normalization` metadata verbatim). Zero or non-finite embeddings are
/// errors, never clamped.
pub fn pool_mean_norm(
    window_embeddings: &[Vec<f32>],
    embedding_dim: usize,
) -> Result<Vec<f32>, String> {
    if window_embeddings.is_empty() {
        return Err("cannot pool an empty window set".to_string());
    }
    let mut sum = vec![0.0f64; embedding_dim];
    for embedding in window_embeddings {
        if embedding.len() != embedding_dim {
            return Err(format!(
                "window embedding has {} values; expected {embedding_dim}",
                embedding.len()
            ));
        }
        let norm = f64::from(norm(embedding));
        if norm == 0.0 || !norm.is_finite() {
            return Err("model returned a zero or non-finite embedding".to_string());
        }
        for (index, value) in embedding.iter().enumerate() {
            sum[index] += f64::from(*value) / norm;
        }
    }
    let mean = sum
        .into_iter()
        .map(|value| (value / window_embeddings.len() as f64) as f32)
        .collect::<Vec<_>>();
    let mean_norm = norm(&mean);
    if mean_norm == 0.0 || !mean_norm.is_finite() {
        return Err("pooled embedding is zero or non-finite".to_string());
    }
    Ok(mean
        .into_iter()
        .map(|value| (value / mean_norm) as f32)
        .collect())
}

// ---------------------------------------------------------------------
// model artifact verification and loading
// ---------------------------------------------------------------------

/// What is wrong with a supplied model artifact. Every variant fails
/// closed: no vectors, no crash, no fallback to another file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// The artifact path does not exist or cannot be read.
    MissingArtifact {
        /// The supplied path, echoed for diagnosability.
        path: String,
    },
    /// The file's SHA-256 differs from the profile's recorded digest.
    ShaMismatch {
        /// Expected digest (the profile constant).
        expected: String,
        /// Observed digest.
        actual: String,
    },
    /// The bytes are not a loadable ONNX model.
    LoadError {
        /// rten's report, verbatim.
        detail: String,
    },
    /// The model lacks the expected input tensor or embedding output
    /// (wrong file for this profile, or an unsupported graph revision).
    UnexpectedLayout {
        /// What was expected and what was found.
        detail: String,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::MissingArtifact { path } => {
                write!(f, "model artifact not found: {path}")
            }
            ModelError::ShaMismatch { expected, actual } => {
                write!(
                    f,
                    "model SHA-256 mismatch: expected {expected}, got {actual}"
                )
            }
            ModelError::LoadError { detail } => {
                write!(f, "cannot load model artifact: {detail}")
            }
            ModelError::UnexpectedLayout { detail } => {
                write!(f, "model layout not supported by this profile: {detail}")
            }
        }
    }
}

impl std::error::Error for ModelError {}

/// Verified model facts for diagnostics and profile documentation. Holds
/// no weights: callers that need inference keep their own loaded runner
/// (see [`EffNetProducer`]). Only available with the `discogs-effnet`
/// feature (verifying loadability needs the runtime).
#[cfg(feature = "discogs-effnet")]
#[derive(Debug, Clone)]
pub struct VerifiedModel {
    /// Exact input tensor name found.
    pub input_name: String,
    /// Input shape as reported (`[64, 128, 96]`).
    pub input_shape: String,
    /// Selected embedding output node name.
    pub output_name: String,
    /// Output shape as reported.
    pub output_shape: String,
    /// Embedding dimension count (profile-declared value, confirmed).
    pub embedding_dim: usize,
}

/// Verifies an operator-supplied artifact's file identity *without* any
/// model runtime: presence plus SHA-256 against the profile's recorded
/// digest. Always available (no feature needed); hosts use it to fail fast
/// on a missing or substituted file before any inference setup.
pub fn verify_artifact_sha(path: &Path, expected_sha256: &str) -> Result<(), ModelError> {
    read_and_verify(path, expected_sha256)?;
    Ok(())
}

fn read_and_verify(path: &Path, expected_sha256: &str) -> Result<Vec<u8>, ModelError> {
    let bytes = std::fs::read(path).map_err(|_| ModelError::MissingArtifact {
        path: path.display().to_string(),
    })?;
    let actual = checksum::sha256_hex(&bytes);
    if actual != expected_sha256 {
        return Err(ModelError::ShaMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(bytes)
}

/// Verifies an operator-supplied artifact against a profile *without*
/// running inference: file presence, SHA-256 identity, loadability, and
/// the pinned input/output layout. Used by hosts (and tests) to pre-check
/// an artifact; [`EffNetProducer`] applies the same checks lazily.
/// Requires the `discogs-effnet` feature (model loading needs rten).
#[cfg(feature = "discogs-effnet")]
pub fn verify_model_artifact(
    path: &Path,
    expected_sha256: &str,
    expected_dims: usize,
) -> Result<VerifiedModel, ModelError> {
    let bytes = read_and_verify(path, expected_sha256)?;
    let runner = ModelRunner::load_from_bytes(&bytes, expected_dims)?;
    Ok(VerifiedModel {
        input_name: runner.input_name(),
        input_shape: runner.input_shape(),
        output_name: runner.output_name(),
        output_shape: runner.output_shape(),
        embedding_dim: runner.embedding_dim(),
    })
}

/// rten session over one loaded artifact (ported from experiment
/// `model.rs`): pinned input tensor, pinned embedding output, fixed
/// batch of 64 with zero-padded tail, one inference thread. Requires the
/// `discogs-effnet` feature.
#[cfg(feature = "discogs-effnet")]
struct ModelRunner {
    model: rten::Model,
    input_id: rten::NodeId,
    output_id: rten::NodeId,
    embedding_dim: usize,
    thread_pool: std::sync::Arc<rten::ThreadPool>,
}

#[cfg(feature = "discogs-effnet")]
impl ModelRunner {
    fn load_from_bytes(bytes: &[u8], expected_dims: usize) -> Result<Self, ModelError> {
        let model = rten::Model::load(bytes.to_vec()).map_err(|e| ModelError::LoadError {
            detail: e.to_string(),
        })?;
        let input_id =
            model
                .node_id(INPUT_TENSOR_NAME)
                .map_err(|_| ModelError::UnexpectedLayout {
                    detail: format!("model has no input tensor {INPUT_TENSOR_NAME:?}"),
                })?;
        let (output_id, embedding_dim) = select_embedding_output(&model, expected_dims)?;
        let thread_pool = std::sync::Arc::new(rten::ThreadPool::with_num_threads(1));
        Ok(Self {
            model,
            input_id,
            output_id,
            embedding_dim,
            thread_pool,
        })
    }

    fn input_name(&self) -> String {
        self.model
            .node_info(self.input_id)
            .and_then(|info| info.name().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn input_shape(&self) -> String {
        self.model
            .input_shape(0)
            .map(|shape| format!("{shape:?}"))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn output_shape(&self) -> String {
        self.model
            .node_info(self.output_id)
            .and_then(|info| format!("{:?}", info.shape()).into())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn output_name(&self) -> String {
        self.model
            .node_info(self.output_id)
            .and_then(|info| info.name().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    fn run_patches(&self, patches: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, String> {
        use rten::{RunOptions, ValueView};
        if patches.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::with_capacity(patches.len());
        for batch in patches.chunks(BATCH_SIZE) {
            let mut data = vec![0.0f32; BATCH_SIZE * INPUT_FRAMES * MEL_BANDS];
            for (batch_index, patch) in batch.iter().enumerate() {
                if patch.len() != INPUT_FRAMES * MEL_BANDS {
                    return Err(format!(
                        "patch {} has {} values; expected {}",
                        batch_index,
                        patch.len(),
                        INPUT_FRAMES * MEL_BANDS
                    ));
                }
                let start = batch_index * INPUT_FRAMES * MEL_BANDS;
                data[start..start + patch.len()].copy_from_slice(patch);
            }
            let input = ValueView::from_shape([BATCH_SIZE, INPUT_FRAMES, MEL_BANDS], &data)
                .map_err(|e| format!("cannot shape model input: {e}"))?;
            let options = RunOptions::default().with_thread_pool(Some(self.thread_pool.clone()));
            let [output] = self
                .model
                .run_n(
                    vec![(self.input_id, input.into())],
                    [self.output_id],
                    Some(options),
                )
                .map_err(|e| format!("model inference failed: {e}"))?;
            let (shape, values) = output
                .into_shape_vec::<f32, 2>()
                .map_err(|e| format!("model output is not f32 rows: {e}"))?;
            if shape[0] != BATCH_SIZE || shape[1] != self.embedding_dim {
                return Err(format!(
                    "unexpected embedding output shape: {shape:?}; expected [{BATCH_SIZE}, {}]",
                    self.embedding_dim
                ));
            }
            results.extend(
                values
                    .chunks_exact(self.embedding_dim)
                    .take(batch.len())
                    .map(|chunk| chunk.to_vec()),
            );
        }
        Ok(results)
    }
}

/// Selects the embedding output: the node whose name contains "embedding"
/// **and** whose trailing dimension equals the profile's. Both conditions
/// are required — the multi artifact also carries a 512-wide predictions
/// output that name-only selection could confuse, and dimension-only
/// selection could accept a wrong graph revision. Anything else fails
/// closed (stricter than the experiment's fallback chain, which existed
/// for exploration, not production). Requires the `discogs-effnet` feature.
#[cfg(feature = "discogs-effnet")]
fn select_embedding_output(
    model: &rten::Model,
    expected_dims: usize,
) -> Result<(rten::NodeId, usize), ModelError> {
    for id in model.output_ids().iter().copied() {
        let Some(info) = model.node_info(id) else {
            continue;
        };
        let name = info.name().unwrap_or_default().to_ascii_lowercase();
        if !name.contains("embedding") {
            continue;
        }
        let Some(shape) = info.shape() else { continue };
        if shape.len() != 2 {
            continue;
        }
        let rten::Dimension::Fixed(size) = shape[1] else {
            continue;
        };
        if size == expected_dims {
            return Ok((id, size));
        }
    }
    Err(ModelError::UnexpectedLayout {
        detail: format!("model has no embedding output with {expected_dims} dimensions"),
    })
}

// ---------------------------------------------------------------------
// concrete producer
// ---------------------------------------------------------------------

/// Load state of one producer: the artifact loads lazily on first analysis
/// (so missing/invalid weights surface as honest per-track `Failed`
/// outcomes, never as package failures) and memoizes afterwards.
#[cfg(feature = "discogs-effnet")]
enum LoadState {
    Unloaded,
    Loaded(Box<ModelRunner>),
    Failed(String),
}

/// Discogs-EffNet [`SimilarityProducer`](crate::similarity::SimilarityProducer).
///
/// Constructed with an operator-supplied model path and a profile
/// ([`multi_profile`] / [`release_profile`]); the artifact is verified
/// (presence, SHA-256, layout) on first use, never downloaded, never
/// bundled. Single-threaded by construction (`RefCell` state, one rten
/// thread): deterministic for identical input and profile.
///
/// Regression procedure (no model in the repository): with the artifact
/// supplied out of band, run this producer and the experiment binary over
/// the same fixture audio and compare `embedding_sha256` digests. The
/// DSP below is a line-faithful port, so any digest divergence is an
/// implementation regression to investigate — never a tolerance to add.
///
/// Requires the `discogs-effnet` feature: without it there is no runtime
/// to run, so the whole producer (construction included) is unavailable
/// and similarity stays a metadata/DSP-only crate.
#[cfg(feature = "discogs-effnet")]
pub struct EffNetProducer {
    profile: SimilarityProfile,
    model_path: PathBuf,
    expected_sha256: &'static str,
    expected_dims: usize,
    frontend: MelFrontend,
    state: RefCell<LoadState>,
}

#[cfg(feature = "discogs-effnet")]
impl EffNetProducer {
    /// Multi / 1280-D producer over an operator-supplied artifact path.
    /// Infallible: verification happens on first analysis, so an absent
    /// artifact degrades to per-track failures instead of aborting setup.
    pub fn multi(model_path: PathBuf) -> Self {
        Self::for_profile(multi_profile(), model_path, MULTI_MODEL_SHA256, 1280)
    }

    /// Release / 512-D producer. A distinct profile, never interchangeable
    /// with [`EffNetProducer::multi`].
    pub fn release(model_path: PathBuf) -> Self {
        Self::for_profile(release_profile(), model_path, RELEASE_MODEL_SHA256, 512)
    }

    fn for_profile(
        profile: SimilarityProfile,
        model_path: PathBuf,
        expected_sha256: &'static str,
        expected_dims: usize,
    ) -> Self {
        Self {
            profile,
            model_path,
            expected_sha256,
            expected_dims,
            frontend: MelFrontend::new(),
            state: RefCell::new(LoadState::Unloaded),
        }
    }

    /// The profile this producer implements.
    pub fn producer_profile(&self) -> &SimilarityProfile {
        &self.profile
    }

    fn ensure_loaded(&self) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        match &*state {
            LoadState::Loaded(_) => Ok(()),
            LoadState::Failed(detail) => Err(detail.clone()),
            LoadState::Unloaded => {
                let bytes = std::fs::read(&self.model_path)
                    .map_err(|_| "model artifact not found".to_string())?;
                let actual = checksum::sha256_hex(&bytes);
                if actual != self.expected_sha256 {
                    let detail = "model SHA-256 mismatch".to_string();
                    *state = LoadState::Failed(detail.clone());
                    return Err(detail);
                }
                match ModelRunner::load_from_bytes(&bytes, self.expected_dims) {
                    Ok(runner) => {
                        *state = LoadState::Loaded(Box::new(runner));
                        Ok(())
                    }
                    Err(error) => {
                        let detail = error.to_string();
                        *state = LoadState::Failed(detail.clone());
                        Err(detail)
                    }
                }
            }
        }
    }
}

#[cfg(feature = "discogs-effnet")]
impl crate::similarity::SimilarityProducer for EffNetProducer {
    fn profile(&self) -> &SimilarityProfile {
        &self.profile
    }

    fn analyze(&self, input: &ProducerInput<'_>) -> TrackSimilarity {
        if self.ensure_loaded().is_err() {
            return TrackSimilarity::Failed;
        }
        let state = self.state.borrow();
        let runner = match &*state {
            LoadState::Loaded(runner) => runner,
            // ensure_loaded succeeded, so only Loaded is reachable; any
            // other state is an internal invariant violation, reported as
            // failure rather than panicking.
            LoadState::Unloaded | LoadState::Failed(_) => return TrackSimilarity::Failed,
        };
        let prepared = match decode_and_prepare(input.audio_path) {
            Ok(prepared) => prepared,
            Err(_) => return TrackSimilarity::Failed,
        };
        let frontend_output = self.frontend.process(&prepared.samples);
        if frontend_output.patches.is_empty() {
            return TrackSimilarity::InsufficientAudio;
        }
        let windows = match runner.run_patches(&frontend_output.patches) {
            Ok(windows) => windows,
            Err(_) => return TrackSimilarity::Failed,
        };
        match pool_mean_norm(&windows, self.expected_dims) {
            Ok(vector) => TrackSimilarity::Ok { vector },
            Err(_) => TrackSimilarity::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::similarity::VectorEncoding;

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// The TLV encoder reproduces the specification's golden vector for the
    /// fixture profile byte for byte (FORMAT_SPEC Appendix A, 191 bytes).
    /// This pins the port independently of any profile defined here.
    #[test]
    fn tlv_matches_the_specification_golden_vector() {
        let fields = ProfileFields {
            profile_id: "musicpack-similarity-fixture-v1",
            model_family: "synthetic-fixture",
            model_variant: "none",
            model_sha256: None,
            preprocessing_version: "synthetic-linear-v1",
            patch_hop: 32,
            pooling: "mean-of-l2-unit-then-l2",
            normalization: "l2",
            metric: "cosine",
            dimensions: 4,
            output_encoding: "f32le",
            runtime: None,
            numeric_policy: "scalar-no-contraction",
        };
        let golden = concat!(
            "010000001f6d757369637061636b2d73696d696c61726974792d666978747572",
            "652d7631020000001173796e7468657469632d6669787475726503000000046e",
            "6f6e65050000001373796e7468657469632d6c696e6561722d76310600000004",
            "0000002007000000176d65616e2d6f662d6c322d756e69742d7468656e2d6c32",
            "08000000026c320900000006636f73696e650a00000004000000040b00000005",
            "6633326c650e000000157363616c61722d6e6f2d636f6e7472616374696f6e",
        );
        let encoded = profile_tlv(&fields);
        assert_eq!(encoded.len(), 191);
        assert_eq!(encoded, hex_bytes(golden));
    }

    #[test]
    fn profile_definitions_are_complete_and_distinct() {
        let multi = multi_profile();
        let release = release_profile();
        assert_eq!(
            multi.profile_id,
            "musicpack-similarity-discogs-effnet-multi-v1"
        );
        assert_eq!(
            release.profile_id,
            "musicpack-similarity-discogs-effnet-release-v1"
        );
        assert_eq!(multi.dimensions, 1280);
        assert_eq!(release.dimensions, 512);
        assert_eq!(multi.encoding, VectorEncoding::F32Le);
        assert_eq!(release.encoding, VectorEncoding::F32Le);
        // Distinct fingerprints: dimension collisions never imply
        // comparability, and neither fingerprint is zero.
        assert_ne!(multi.fingerprint, release.fingerprint);
        assert_ne!(multi.fingerprint, [0u8; 32]);
        assert_ne!(release.fingerprint, [0u8; 32]);
        // Deterministic: recomputation from the field lists agrees.
        assert_eq!(multi.fingerprint, profile_fingerprint(&multi_fields()));
        assert_eq!(release.fingerprint, profile_fingerprint(&release_fields()));
        // The embedded digest bytes match the documented SHA strings.
        use musicpack_core::format::checksum::sha256_hex_to_bytes;
        assert_eq!(
            sha256_hex_to_bytes(MULTI_MODEL_SHA256),
            Some(MULTI_MODEL_SHA256_BYTES)
        );
        assert_eq!(
            sha256_hex_to_bytes(RELEASE_MODEL_SHA256),
            Some(RELEASE_MODEL_SHA256_BYTES)
        );
    }

    #[test]
    fn frontend_has_expected_shape_and_is_deterministic() {
        let frontend = MelFrontend::new();
        let zeros = vec![0.0f32; 160_000];
        let first = frontend.process(&zeros);
        let second = frontend.process(&zeros);
        assert_eq!(first.frame_count, 625);
        assert_eq!(first.patches.len(), 9);
        assert!(
            first
                .patches
                .iter()
                .all(|patch| patch.len() == PATCH_SIZE * MEL_BANDS)
        );
        assert_eq!(first.patches, second.patches);
        // Below one patch of frames: no patches (maps to InsufficientAudio).
        let short = frontend.process(&vec![0.0f32; 16_000]);
        assert!(short.patches.is_empty());
    }

    #[test]
    fn pooling_rejects_degenerate_windows() {
        assert!(pool_mean_norm(&[], 4).is_err());
        assert!(pool_mean_norm(&[vec![0.0; 4]], 4).is_err());
        assert!(pool_mean_norm(&[vec![f32::NAN; 4]], 4).is_err());
        assert!(pool_mean_norm(&[vec![1.0, 0.0]], 4).is_err());
        let pooled = pool_mean_norm(&[vec![3.0, 4.0, 0.0, 0.0]], 4).unwrap();
        assert_eq!(pooled.len(), 4);
        let norm: f64 = pooled.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
        assert!((norm.sqrt() - 1.0).abs() < 1e-6);
    }
}
