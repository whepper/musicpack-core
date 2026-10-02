//! Sonic52 Slice 1: deterministic research log-mel frontend (H0).
//!
//! This module implements the H0 frontend frozen in `FORENSICS.md` §11 as
//! **our experimental configuration** — lineage-derived hypotheses plus
//! explicit experimental choices, never Plex facts:
//!
//! ```text
//! 16 kHz mono f32 PCM (caller-provided; see [`FrontendInput`])
//! → 512-sample centered frames, 256-sample hop, zero-padded edges
//! → symmetric Hann window, zero-phase, unnormalized
//! → 512-point power spectrum (bins 0..=256)
//! → 96 Slaney mel bands, linear weighting, unit-triangle, 0–8000 Hz
//! → log10(10000 * x + 1)
//! → 187-frame patches, stride 93, partial final patch discarded
//! ```
//!
//! Deliberately absent: audio decoding, resampling, model inference,
//! weights, embeddings, ranking, and any production wiring. Decoding and
//! any resampling/downmixing stay caller-side; a future slice wires
//! `musicpack-core::audio` plus an explicitly chosen resampler. The only
//! input this module accepts is mono 16 kHz `f32` PCM (see
//! [`FrontendInput::mono_16k`]); anything else is an explicit
//! [`InputError`], never a silent conversion.
//!
//! # Determinism
//!
//! Same platform + same toolchain ⇒ bit-identical patches (all
//! arithmetic has fixed evaluation order; DSP state never depends on
//! chunk boundaries — see [`MelFrontend`] and the chunk-invariance
//! test). Cross-platform bit-identity is **not** claimed: `cos`/`sin`/
//! `log10`/`ln` come from the platform libm and may differ in the last
//! ulp elsewhere. Tests therefore use exact equality for serialization
//! and chunk invariance, and documented tolerances for independently
//! calculated DSP goldens.

#![forbid(unsafe_code)]

use std::f64::consts::PI;

// ---------------------------------------------------------------------
// H0 constants (FORENSICS.md §11; B = lineage, C = our choice)
// ---------------------------------------------------------------------

/// H0 analysis sample rate in Hz (B: lineage-unanimous, Plex-unattested).
pub const SAMPLE_RATE_HZ: u32 = 16_000;
/// H0 frame size in samples (B).
pub const FRAME_SIZE: usize = 512;
/// H0 frame hop in samples (B).
pub const FRAME_HOP: usize = 256;
/// H0 mel band count (B).
pub const MEL_BANDS: usize = 96;
/// H0 mel range lower bound in Hz (B).
pub const MEL_LOW_HZ: f64 = 0.0;
/// H0 mel range upper bound in Hz (B: `sampleRate / 2`, carried exactly).
pub const MEL_HIGH_HZ: f64 = 8_000.0;
/// H0 patch length in mel frames: 187 × 256 / 16000 ≈ 2.992 s (B).
pub const PATCH_FRAMES: usize = 187;
/// H0 patch stride in mel frames: 93 × 256 / 16000 ≈ 1.488 s
/// (B: lineage default; ablation axis #1).
pub const PATCH_STRIDE: usize = 93;
/// H0 log-compression scale: `log10(10000 * x + 1)` (B, two-sided:
/// identical in training code and inference frontend).
pub const LOG_SCALE: f64 = 10_000.0;
/// H0 log-compression shift (B, same provenance as [`LOG_SCALE`]).
pub const LOG_SHIFT: f64 = 1.0;
/// Non-negative-frequency power bins kept per frame: 0..=256 (B: mel
/// filterbank spans DC..Nyquist inclusive).
pub const SPECTRUM_BINS: usize = FRAME_SIZE / 2 + 1;
/// Serialized bytes of one patch: 187 × 96 × 4.
pub const PATCH_F32LE_BYTES: usize = PATCH_FRAMES * MEL_BANDS * 4;

// ---------------------------------------------------------------------
// input contract
// ---------------------------------------------------------------------

/// What is wrong with audio offered to the H0 frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    /// Sample rate is not [`SAMPLE_RATE_HZ`]. Resampling is a future
    /// explicit choice, never a silent conversion (C).
    WrongSampleRate {
        /// Offered rate in Hz.
        found: u32,
    },
    /// Channel count is not mono. Downmixing is a future explicit
    /// choice, never a silent conversion (C).
    NotMono {
        /// Offered channel count.
        found: u16,
    },
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::WrongSampleRate { found } => write!(
                f,
                "H0 requires {SAMPLE_RATE_HZ} Hz mono PCM, found {found} Hz (resampling is not performed here)"
            ),
            InputError::NotMono { found } => write!(
                f,
                "H0 requires mono PCM, found {found} channels (downmixing is not performed here)"
            ),
        }
    }
}

impl std::error::Error for InputError {}

/// Validated H0 frontend input: mono [`SAMPLE_RATE_HZ`] `f32` PCM.
///
/// The samples are the caller's responsibility (decoded, resampled, and
/// downmixed upstream by explicitly chosen, explicitly recorded means).
/// This type only *checks* the contract; it never converts. Empty input
/// is valid and yields zero frames and zero patches.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontendInput {
    samples: Vec<f32>,
}

impl FrontendInput {
    /// Validates sample rate and channel count, then takes ownership of
    /// already-conditioned samples.
    pub fn mono_16k(
        sample_rate_hz: u32,
        channels: u16,
        samples: Vec<f32>,
    ) -> Result<Self, InputError> {
        if sample_rate_hz != SAMPLE_RATE_HZ {
            return Err(InputError::WrongSampleRate {
                found: sample_rate_hz,
            });
        }
        if channels != 1 {
            return Err(InputError::NotMono { found: channels });
        }
        Ok(Self { samples })
    }

    /// The conditioned samples.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }
}

// ---------------------------------------------------------------------
// output representation
// ---------------------------------------------------------------------

/// One H0 frontend patch: exactly 187 × 96 `f32` log-mel values in
/// row-major (frame-major) order: `values[frame][band]`.
///
/// This is a *frontend representation*, not a model input tensor and
/// not an embedding: it must never be confused with the production
/// `.msim` format (which this crate neither reads nor writes) or with a
/// future 52-D Sonic52 vector. Tensor layout decisions belong to a
/// later slice; `[187, 96]` here is the H0 *patch shape*, not a proven
/// Plex tensor contract (FORENSICS.md §10).
#[derive(Debug, Clone, PartialEq)]
pub struct Sonic52MelPatch {
    values: [[f32; MEL_BANDS]; PATCH_FRAMES],
}

impl Sonic52MelPatch {
    /// Assembles a patch from 187 frames in order. Additive Slice 3 API
    /// for the behavioural harness; the H0 DSP path is unchanged.
    pub fn from_frames(frames: [[f32; MEL_BANDS]; PATCH_FRAMES]) -> Self {
        Self { values: frames }
    }

    /// Patch frames in order; each frame holds 96 band values.
    pub fn frames(&self) -> &[[f32; MEL_BANDS]; PATCH_FRAMES] {
        &self.values
    }

    /// Deterministic little-endian serialization: 187 × 96 × 4 = 71,808
    /// bytes, frame-major. Equal patches yield equal bytes; no
    /// timestamps, no paths, no padding.
    pub fn to_f32le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PATCH_F32LE_BYTES);
        for frame in &self.values {
            for value in frame {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out
    }
}

/// Frontend result: the mel frames plus the patch pack.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontendOutput {
    /// One 96-band vector per centered frame, in order.
    pub mel_frames: Vec<[f32; MEL_BANDS]>,
    /// Non-overlapping-stride patches over `mel_frames`
    /// (stride 93, partial tail discarded).
    pub patches: Vec<Sonic52MelPatch>,
}

// ---------------------------------------------------------------------
// stage 1: framing (centered, zero-padded, streaming-safe)
// ---------------------------------------------------------------------

/// Number of centered H0 frames for `sample_count` mono samples.
///
/// Frame `i` (0-based) starts at `i * 256 - 256`, so frame 0 straddles
/// the signal start (left half zero-padded) — the lineage centered
/// convention (Essentia streaming `FrameCutter` default), adopted here
/// as an explicit experimental choice (C). A frame exists for every
/// start position before the signal end; the tail is zero-padded, and a
/// trailing run of samples too short to start a frame yields nothing.
pub fn frame_count_for_samples(sample_count: usize) -> usize {
    if sample_count <= FRAME_SIZE / 2 {
        return 0;
    }
    1 + sample_count
        .saturating_sub(FRAME_SIZE / 2)
        .div_ceil(FRAME_HOP)
}

/// Extracts one centered 512-sample frame, zero-filling outside
/// `samples`. Pure function of `(samples, frame_index)`: no hidden
/// state, so chunking decisions upstream cannot affect it.
pub fn extract_frame(samples: &[f32], frame_index: usize) -> [f32; FRAME_SIZE] {
    let mut frame = [0.0f32; FRAME_SIZE];
    let start = frame_index as isize * FRAME_HOP as isize - (FRAME_SIZE as isize / 2);
    for (i, slot) in frame.iter_mut().enumerate() {
        let source = start + i as isize;
        if source >= 0 && (source as usize) < samples.len() {
            let value = samples[source as usize];
            // Analysis-boundary sanitization (C, mirrors the
            // `musicpack-core::audio` precedent documented in
            // FORENSICS.md lineage notes): NaN -> 0, ±Inf -> ±1.
            // Finite out-of-range values pass through untouched.
            *slot = sanitize(value);
        }
    }
    frame
}

fn sanitize(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else if value == f32::INFINITY {
        1.0
    } else if value == f32::NEG_INFINITY {
        -1.0
    } else {
        value
    }
}

// ---------------------------------------------------------------------
// stage 2: symmetric Hann window, zero-phase, unnormalized
// ---------------------------------------------------------------------

/// H0 Hann coefficients, symmetric (SciPy-style), unnormalized:
///
/// ```text
/// w[n] = 0.5 - 0.5 * cos(2 * pi * n / 511), n = 0..512
/// ```
///
/// Endpoints are exactly zero; the peak straddles bins 255/256
/// (symmetric, *not* periodic — a periodic Hann is a different window
/// and must not be substituted). Computed in `f64`, stored as `f32`.
pub fn hann_window() -> [f32; FRAME_SIZE] {
    let mut window = [0.0f32; FRAME_SIZE];
    for (n, slot) in window.iter_mut().enumerate() {
        *slot = (0.5 - 0.5 * (2.0 * PI * n as f64 / (FRAME_SIZE - 1) as f64).cos()) as f32;
    }
    window
}

/// Applies the Hann window with zero-phase rotation: for the even frame
/// size the two windowed halves are swapped before the FFT
/// (`windowed[0..256] = frame[256..512] * w[256..512]`,
/// `windowed[256..512] = frame[0..256] * w[0..256]`).
/// Because the window is symmetric this equals plain windowing followed
/// by a half-frame rotation; the rotation is what makes it zero-phase.
pub fn apply_window(frame: &[f32; FRAME_SIZE], window: &[f32; FRAME_SIZE]) -> [f32; FRAME_SIZE] {
    let mut out = [0.0f32; FRAME_SIZE];
    for i in 0..FRAME_SIZE / 2 {
        let first = FRAME_SIZE / 2 + i;
        out[i] = frame[first] * window[first];
        out[FRAME_SIZE / 2 + i] = frame[i] * window[i];
    }
    out
}

// ---------------------------------------------------------------------
// stage 3: 512-point power spectrum (local radix-2 FFT, no dependency)
// ---------------------------------------------------------------------

/// Power spectrum of a windowed frame: bins 0..=256 (`|X[k]|²`,
/// DC..Nyquist inclusive).
///
/// The FFT is a local iterative radix-2 decimation-in-time transform
/// with `f64` twiddle factors and fixed evaluation order — chosen over a
/// DSP dependency so the exact arithmetic is auditable and no library
/// default (scaling, packing, normalization) can leak in. No
/// normalization is applied anywhere: impulse/sinusoid goldens pin the
/// absolute scale (see tests).
pub fn power_spectrum(windowed: &[f32; FRAME_SIZE]) -> [f32; SPECTRUM_BINS] {
    let mut re = [0.0f64; FRAME_SIZE];
    let mut im = [0.0f64; FRAME_SIZE];
    for (i, sample) in windowed.iter().enumerate() {
        re[i] = f64::from(*sample);
    }
    fft_in_place(&mut re, &mut im);
    let mut power = [0.0f32; SPECTRUM_BINS];
    for k in 0..SPECTRUM_BINS {
        power[k] = (re[k] * re[k] + im[k] * im[k]) as f32;
    }
    power
}

fn fft_in_place(re: &mut [f64; FRAME_SIZE], im: &mut [f64; FRAME_SIZE]) {
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..FRAME_SIZE {
        let mut bit = FRAME_SIZE >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    // Iterative butterflies, fixed order, libm twiddles.
    let mut len = 2usize;
    while len <= FRAME_SIZE {
        let half = len / 2;
        let angle_step = -2.0 * PI / len as f64;
        for start in (0..FRAME_SIZE).step_by(len) {
            for k in 0..half {
                let angle = angle_step * k as f64;
                let (s, c) = angle.sin_cos();
                let ur = re[start + k];
                let ui = im[start + k];
                let vr = re[start + k + half] * c - im[start + k + half] * s;
                let vi = re[start + k + half] * s + im[start + k + half] * c;
                re[start + k] = ur + vr;
                im[start + k] = ui + vi;
                re[start + k + half] = ur - vr;
                im[start + k + half] = ui - vi;
            }
        }
        len *= 2;
    }
}

// ---------------------------------------------------------------------
// stage 4: 96-band Slaney mel filterbank, linear/unit-triangle
// ---------------------------------------------------------------------

/// Slaney mel warping (B: the published Auditory-Toolbox form used by
/// both lineage ends): linear below 1000 Hz (slope 3/200), logarithmic
/// above with step `ln(6.4) / 27`.
pub fn hz_to_mel_slaney(hz: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN_SLOPE: f64 = 3.0 / 200.0;
    const LOG_STEP: f64 = 0.068_751_846_601_861_77; // ln(6.4) / 27
    if hz < MIN_LOG_HZ {
        hz * LIN_SLOPE
    } else {
        MIN_LOG_HZ * LIN_SLOPE + (hz / MIN_LOG_HZ).ln() / LOG_STEP
    }
}

/// Inverse Slaney warping.
pub fn mel_to_hz_slaney(mel: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN_SLOPE: f64 = 3.0 / 200.0;
    const LOG_STEP: f64 = 0.068_751_846_601_861_77; // ln(6.4) / 27
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ * LIN_SLOPE;
    if mel < MIN_LOG_MEL {
        mel / LIN_SLOPE
    } else {
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * LOG_STEP).exp()
    }
}

/// One mel filter: the FFT bins it touches with their weights.
#[derive(Debug, Clone, PartialEq)]
pub struct MelFilter {
    /// Filter index 0..96.
    pub band: usize,
    /// `(bin, weight)` pairs, ascending bins, all weights in `[0, 1]`.
    pub weights: Vec<(usize, f32)>,
}

/// H0 mel filterbank (B): 96 triangular filters, Slaney-warped,
/// linearly weighted, unit-triangle normalized (each filter divided by
/// the theoretical triangle area `(lower_step + upper_step) / 2` in Hz,
/// *not* by the discrete bin-weight sum), spanning 0–8000 Hz with the
/// Nyquist bin included in the top filter.
///
/// Conventions pinned here (each a potential silent-divergence source
/// elsewhere): bin `k` sits at `k * 8000/256` Hz; a filter covering
/// `[start, center, end]` (Hz, Slaney-spaced) touches bins
/// `ceil(start/scale) ..= min(floor(end/scale), 256)`; triangle weight
/// is `(f - start) / (center - start)` below center and
/// `(end - f) / (end - center)` at/above center.
pub fn mel_filterbank() -> Vec<MelFilter> {
    let bin_hz = MEL_HIGH_HZ / (SPECTRUM_BINS - 1) as f64;
    let low_mel = hz_to_mel_slaney(MEL_LOW_HZ);
    let high_mel = hz_to_mel_slaney(MEL_HIGH_HZ);
    let mut edge_hz = Vec::with_capacity(MEL_BANDS + 2);
    for i in 0..=MEL_BANDS + 1 {
        let mel = low_mel + (high_mel - low_mel) * i as f64 / (MEL_BANDS + 1) as f64;
        edge_hz.push(mel_to_hz_slaney(mel));
    }
    let mut filters = Vec::with_capacity(MEL_BANDS);
    for band in 0..MEL_BANDS {
        let start = edge_hz[band];
        let center = edge_hz[band + 1];
        let end = edge_hz[band + 2];
        let first_step = center - start;
        let second_step = end - center;
        let begin = (start / bin_hz).ceil() as usize;
        let finish = ((end / bin_hz).floor() as usize).min(SPECTRUM_BINS - 1);
        let mut weights = Vec::new();
        if begin <= finish {
            for bin in begin..=finish {
                let frequency = bin as f64 * bin_hz;
                let weight = if frequency < center {
                    (frequency - start) / first_step
                } else {
                    (end - frequency) / second_step
                };
                if weight > 0.0 {
                    weights.push((bin, weight as f32));
                }
            }
            // Unit-triangle normalization by theoretical area (Hz).
            let normalization = (first_step + second_step) / 2.0;
            for (_, weight) in &mut weights {
                *weight = (*weight as f64 / normalization) as f32;
            }
        }
        filters.push(MelFilter { band, weights });
    }
    filters
}

/// Projects one power spectrum through the filterbank with `f64`
/// accumulation (B: same precision mixing the lineage inference path
/// uses), returning 96 band energies.
pub fn mel_energies(power: &[f32; SPECTRUM_BINS], filters: &[MelFilter]) -> [f32; MEL_BANDS] {
    let mut bands = [0.0f32; MEL_BANDS];
    for (filter, slot) in filters.iter().zip(bands.iter_mut()) {
        let mut energy = 0.0f64;
        for (bin, weight) in &filter.weights {
            energy += f64::from(power[*bin]) * f64::from(*weight);
        }
        *slot = energy as f32;
    }
    bands
}

// ---------------------------------------------------------------------
// stage 5: log compression, exactly log10(10000 * x + 1)
// ---------------------------------------------------------------------

/// H0 log compression (B, two-sided): `log10(10000 * x + 1)`.
///
/// Not natural log, not `log1p`, not dB, no epsilon, no output
/// normalization. The `+1` shift makes the floor exactly `log10(1) =
/// 0` for zero energy, so no clamp is needed or applied (a clamp would
/// be a behavioural difference from the lineage chain, however small).
pub fn compress_value(energy: f32) -> f32 {
    ((f64::from(energy) * LOG_SCALE + LOG_SHIFT).log10()) as f32
}

/// Compresses one 96-band energy frame.
pub fn compress_frame(energies: &[f32; MEL_BANDS]) -> [f32; MEL_BANDS] {
    let mut out = [0.0f32; MEL_BANDS];
    for (source, slot) in energies.iter().zip(out.iter_mut()) {
        *slot = compress_value(*source);
    }
    out
}

// ---------------------------------------------------------------------
// stage 6: patch construction (187 frames, stride 93, discard tail)
// ---------------------------------------------------------------------

/// Number of H0 patches for `frame_count` mel frames: one per 93-frame
/// stride position admitting a full 187-frame window; a partial final
/// window is discarded, never padded (C: matches the lineage inference
/// default; training repeat-pads instead — FORENSICS.md §9.4).
pub fn patch_count_for_frames(frame_count: usize) -> usize {
    if frame_count < PATCH_FRAMES {
        return 0;
    }
    1 + (frame_count - PATCH_FRAMES) / PATCH_STRIDE
}

/// Packs mel frames into patches. Frame `PATCH_FRAMES + k * PATCH_STRIDE
/// .. +187` forms patch `k`. Pure indexing over the frame vector.
pub fn build_patches(mel_frames: &[[f32; MEL_BANDS]]) -> Vec<Sonic52MelPatch> {
    let mut patches = Vec::with_capacity(patch_count_for_frames(mel_frames.len()));
    let mut start = 0usize;
    while start + PATCH_FRAMES <= mel_frames.len() {
        let mut values = [[0.0f32; MEL_BANDS]; PATCH_FRAMES];
        values.copy_from_slice(&mel_frames[start..start + PATCH_FRAMES]);
        patches.push(Sonic52MelPatch { values });
        start += PATCH_STRIDE;
    }
    patches
}

/// Runs one windowed frame through spectrum → mel → compression.
/// Pure function of its inputs; the streaming frontend calls it after
/// ensuring its cached tables.
pub fn process_frame(
    frame: &[f32; FRAME_SIZE],
    window: &[f32; FRAME_SIZE],
    filters: &[MelFilter],
) -> [f32; MEL_BANDS] {
    let windowed = apply_window(frame, window);
    let power = power_spectrum(&windowed);
    let energies = mel_energies(&power, filters);
    compress_frame(&energies)
}

// ---------------------------------------------------------------------
// streaming frontend: chunk-invariant by construction
// ---------------------------------------------------------------------

/// Streaming H0 frontend: accepts arbitrarily chunked PCM and produces
/// output identical to a single contiguous pass.
///
/// Chunk invariance holds because framing is a pure function of
/// (complete sample history, frame index): `push_chunk` appends to a
/// sample buffer and emits every frame whose 512 samples are all
/// present; `finish` emits the remaining zero-padded tail frames per
/// [`frame_count_for_samples`] and discards any sub-frame tail. No DSP
/// state crosses a chunk boundary except buffered raw samples.
#[derive(Debug, Default)]
pub struct MelFrontend {
    filters: Option<Vec<MelFilter>>,
    window: Option<[f32; FRAME_SIZE]>,
    buffered: Vec<f32>,
    emitted_frames: usize,
    mel_frames: Vec<[f32; MEL_BANDS]>,
}

impl MelFrontend {
    /// An empty frontend (filterbank and window built lazily on first
    /// use so construction never fails).
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one PCM chunk (any length, including empty). Only appends
    /// raw samples and emits newly completable frames.
    pub fn push_chunk(&mut self, chunk: &[f32]) {
        self.buffered.extend_from_slice(chunk);
        self.emit_available();
    }

    /// Ends the stream: emits zero-padded tail frames, packs patches,
    /// and returns the output. Frames are counted by
    /// [`frame_count_for_samples`] over the total buffered samples, so
    /// the result equals contiguous processing for any chunking.
    pub fn finish(mut self) -> FrontendOutput {
        let total = self.buffered.len();
        let target = frame_count_for_samples(total);
        while self.emitted_frames < target {
            let frame = extract_frame(&self.buffered, self.emitted_frames);
            self.push_processed(frame);
            self.emitted_frames += 1;
        }
        let patches = build_patches(&self.mel_frames);
        FrontendOutput {
            mel_frames: self.mel_frames,
            patches,
        }
    }

    /// Convenience: processes one contiguous slice (exactly one
    /// `push_chunk` + `finish`).
    pub fn process_contiguous(samples: &[f32]) -> FrontendOutput {
        let mut frontend = Self::new();
        frontend.push_chunk(samples);
        frontend.finish()
    }

    fn emit_available(&mut self) {
        // A frame is emittable exactly when it is both admitted by the
        // frame-count function for the samples buffered so far *and*
        // fully present (no padding yet). Fully present frames never
        // change as more data arrives, so eager emission is safe; the
        // zero-padded tail is emitted only by `finish`, identically for
        // every chunking. Together this makes chunking unobservable.
        while self.emitted_frames < frame_count_for_samples(self.buffered.len())
            && self.emitted_frames * FRAME_HOP + FRAME_SIZE / 2 <= self.buffered.len()
        {
            let frame = extract_frame(&self.buffered, self.emitted_frames);
            self.push_processed(frame);
            self.emitted_frames += 1;
        }
    }

    fn push_processed(&mut self, frame: [f32; FRAME_SIZE]) {
        self.ensure_tables();
        let mel = process_frame(
            &frame,
            self.window.as_ref().expect("window built"),
            self.filters.as_ref().expect("filters built"),
        );
        self.mel_frames.push(mel);
    }

    fn ensure_tables(&mut self) {
        if self.window.is_none() {
            self.window = Some(hann_window());
        }
        if self.filters.is_none() {
            self.filters = Some(mel_filterbank());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers ----------------------------------------------------

    /// Deterministic pseudo-signal: no RNG dependency, no fixtures.
    fn multitones(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE_HZ as f32;
                0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.25 * (2.0 * std::f32::consts::PI * 880.0 * t).sin()
                    + 0.125 * (2.0 * std::f32::consts::PI * 617.0 * t).sin()
            })
            .collect()
    }

    fn sine_312_5_hz(samples: usize) -> Vec<f32> {
        // Exactly FFT bin 10 at 512/16kHz: 10 * 16000 / 512 = 312.5.
        (0..samples)
            .map(|i| (2.0 * std::f32::consts::PI * 312.5 * i as f32 / 16000.0).sin())
            .collect()
    }

    // -- input contract ----------------------------------------------

    #[test]
    fn input_contract_rejects_non_h0_audio() {
        assert!(FrontendInput::mono_16k(16_000, 1, vec![0.0; 100]).is_ok());
        assert!(FrontendInput::mono_16k(16_000, 1, vec![]).is_ok());
        assert_eq!(
            FrontendInput::mono_16k(44_100, 1, vec![0.0; 100]).unwrap_err(),
            InputError::WrongSampleRate { found: 44_100 }
        );
        assert_eq!(
            FrontendInput::mono_16k(16_000, 2, vec![0.0; 100]).unwrap_err(),
            InputError::NotMono { found: 2 }
        );
    }

    // -- framing -----------------------------------------------------

    #[test]
    fn frame_counts_and_starts() {
        assert_eq!(frame_count_for_samples(0), 0);
        assert_eq!(frame_count_for_samples(256), 0);
        // 257 samples admit frames starting at -256 and 0.
        assert_eq!(frame_count_for_samples(257), 2);
        assert_eq!(frame_count_for_samples(512), 2);
        // Exact 187-frame PCM: 256 + 186 * 256 = 47872 samples.
        assert_eq!(frame_count_for_samples(47_872), 187);
        assert_eq!(frame_count_for_samples(47_873), 188);
        // Frame 0 straddles the start (left half zero-padded).
        let samples = vec![1.0f32; 1024];
        let first = extract_frame(&samples, 0);
        assert_eq!(&first[0..256], &[0.0f32; 256]);
        assert_eq!(&first[256..512], &[1.0f32; 256]);
        // Frame 1 starts at sample 0: fully covered.
        let second = extract_frame(&samples, 1);
        assert_eq!(&second[..], &[1.0f32; 512]);
        // Hostile values are sanitized at the boundary, finite values pass.
        // (Frame 1 starts at sample 0, so the 4-sample signal lands at
        // frame[0..4]; everything else is zero padding.)
        let hostile = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.5];
        let frame = extract_frame(&hostile, 1);
        assert_eq!(frame[0], 0.0);
        assert_eq!(frame[1], 1.0);
        assert_eq!(frame[2], -1.0);
        assert_eq!(frame[3], 2.5);
        assert_eq!(frame[4], 0.0);
    }

    // -- Hann --------------------------------------------------------

    #[test]
    fn hann_is_symmetric_unnormalized_zero_at_endpoints() {
        let window = hann_window();
        // Endpoints are exactly zero (sin/cos of 0 and 2π path).
        assert_eq!(window[0], 0.0);
        assert_eq!(window[511], 0.0);
        // Symmetric: w[n] == w[511-n] within float evaluation order.
        for n in 0..FRAME_SIZE {
            assert!(
                (window[n] - window[FRAME_SIZE - 1 - n]).abs() < 1e-7,
                "asymmetry at {n}"
            );
        }
        // Peak straddles the middle pair (symmetric, not periodic: the
        // periodic variant would peak at a single central bin instead).
        assert!(window[255] > 0.999_9);
        assert!(window[256] > 0.999_9);
        assert!(window[255] > window[254]);
        assert!(window[1] > 0.0 && window[1] < 0.001);
        // Spot coefficient, hand-computed in f64:
        // w[128] = 0.5 - 0.5*cos(2π*128/511).
        let expected = 0.5 - 0.5 * (2.0 * PI * 128.0 / 511.0).cos();
        assert!((f64::from(window[128]) - expected).abs() < 1e-7);
        // Area is 255.5, not 256 (unnormalized): the symmetric Hann sums
        // to N/2 - 1/2 (the cosine terms sum to exactly 1 over the
        // symmetric sampling) while a periodic Hann would sum to N/2.
        // This guards both against a normalized (area-1) window slipping
        // in (which would read ~0.002 here) and against a periodic
        // variant (which would read 256.0).
        let area: f64 = window.iter().map(|v| f64::from(*v)).sum();
        assert!((area - 255.5).abs() < 0.01, "area {area}");
    }

    #[test]
    fn window_applies_zero_phase_rotation() {
        let window = hann_window();
        // A frame that is nonzero only in its first half must appear in
        // the second half after zero-phase rotation.
        let mut frame = [0.0f32; FRAME_SIZE];
        frame[0] = 1.0;
        let out = apply_window(&frame, &window);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[256], window[0]);
        assert_eq!(out[257], 0.0);
    }

    // -- FFT / power ---------------------------------------------------

    #[test]
    fn impulse_gives_flat_power() {
        // Impulse at the window peak (sample 256): every bin sees the
        // same energy, scaled by the squared window peak.
        let mut frame = [0.0f32; FRAME_SIZE];
        frame[256] = 1.0;
        let window = hann_window();
        let power = power_spectrum(&apply_window(&frame, &window));
        assert_eq!(power.len(), 257);
        let peak = window[256];
        let expected = peak * peak;
        for (k, value) in power.iter().enumerate() {
            assert!(
                (*value - expected).abs() / expected < 1e-5,
                "bin {k} deviates"
            );
        }
    }

    #[test]
    fn sinusoid_peaks_at_its_bin() {
        // 312.5 Hz lands exactly on bin 10; Hann leakage stays local.
        let signal = sine_312_5_hz(16_000);
        let frame = extract_frame(&signal, 4);
        let window = hann_window();
        let power = power_spectrum(&apply_window(&frame, &window));
        let argmax = power
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
            .expect("nonempty")
            .0;
        assert_eq!(argmax, 10);
        assert!(power[10] > 1_000.0 * power[50]);
        assert!(power[10] > 1_000.0 * power[0]);
    }

    #[test]
    fn parseval_holds_for_deterministic_signal() {
        // Independent cross-check of the FFT: time-domain energy equals
        // (P0 + P256 + 2 * ΣP1..255) / 512.
        let signal = multitones(4096);
        let frame = extract_frame(&signal, 4);
        let window = hann_window();
        let windowed = apply_window(&frame, &window);
        let power = power_spectrum(&windowed);
        let time_energy: f64 = windowed.iter().map(|v| f64::from(*v).powi(2)).sum();
        let mut freq_energy = f64::from(power[0]) + f64::from(power[256]);
        for value in power.iter().take(256).skip(1) {
            freq_energy += 2.0 * f64::from(*value);
        }
        freq_energy /= 512.0;
        assert!(
            (time_energy - freq_energy).abs() / time_energy < 1e-6,
            "time {time_energy} vs freq {freq_energy}"
        );
    }

    // -- mel filterbank --------------------------------------------------

    #[test]
    fn mel_bank_has_96_wellformed_filters() {
        let filters = mel_filterbank();
        assert_eq!(filters.len(), 96);
        for filter in &filters {
            // Band 0 genuinely carries a single nonzero weight (its
            // triangle spans DC..~62 Hz, i.e. bins 0..1, with the DC
            // weight exactly zero): this matches the lineage
            // construction, narrowness is not a defect.
            assert!(!filter.weights.is_empty(), "band {} empty", filter.band);
            for (bin, weight) in &filter.weights {
                assert!(*bin < SPECTRUM_BINS);
                assert!(*weight >= 0.0 && *weight <= 1.0, "band {}", filter.band);
            }
        }
        // Lowest filter: the DC bin carries an exactly-zero triangle
        // weight ((0-0)/width), so the first *stored* weight is bin 1.
        // Highest filter: reaches the Nyquist region (bin 255 or 256 —
        // the exact survivor depends on sub-ulp Slaney round-trip error
        // at 8000 Hz, so the test pins coverage, not the float dust).
        assert_eq!(filters[0].weights[0].0, 1);
        assert!(filters[95].weights.last().expect("nonempty").0 >= 255);
        // Center frequencies strictly increase (no folded/empty bands).
        let peaks: Vec<usize> = filters
            .iter()
            .map(|filter| {
                filter
                    .weights
                    .iter()
                    .max_by(|a, b| a.1.partial_cmp(&b.1).expect("finite"))
                    .expect("nonempty")
                    .0
            })
            .collect();
        for pair in peaks.windows(2) {
            assert!(pair[0] < pair[1]);
        }
    }

    #[test]
    fn mel_unit_triangle_spot_check() {
        // Independently recompute one coefficient in f64: band 48's
        // weight at its peak bin must equal triangle-peak / area.
        let filters = mel_filterbank();
        let band = 48usize;
        let low_mel = hz_to_mel_slaney(MEL_LOW_HZ);
        let high_mel = hz_to_mel_slaney(MEL_HIGH_HZ);
        let edge = |i: usize| mel_to_hz_slaney(low_mel + (high_mel - low_mel) * i as f64 / 97.0);
        let (start, center, end) = (edge(band), edge(band + 1), edge(band + 2));
        let area = (center - start + end - center) / 2.0;
        let bin_hz = MEL_HIGH_HZ / 256.0;
        let peak_bin = (center / bin_hz).round() as usize;
        let frequency = peak_bin as f64 * bin_hz;
        let triangle = if frequency < center {
            (frequency - start) / (center - start)
        } else {
            (end - frequency) / (end - center)
        };
        let expected = (triangle / area) as f32;
        let stored = filters[band]
            .weights
            .iter()
            .find(|(bin, _)| *bin == peak_bin)
            .map(|(_, weight)| *weight);
        match stored {
            Some(weight) => assert!(
                (weight - expected).abs() < 1e-6,
                "band {band} bin {peak_bin}: {weight} vs {expected}"
            ),
            None => panic!("band {band} unexpectedly skips its peak bin {peak_bin}"),
        }
    }

    // -- log compression ---------------------------------------------------

    #[test]
    fn log_compression_matches_the_lineage_formula() {
        // Exact scalar goldens of log10(10000x + 1), computed in f64.
        assert_eq!(compress_value(0.0), 0.0);
        let cases = [0.000_1f32, 0.5, 1.0, 123.456];
        for energy in cases {
            let expected = ((f64::from(energy) * 10_000.0 + 1.0).log10()) as f32;
            assert!(
                (compress_value(energy) - expected).abs() < 1e-6,
                "energy {energy}"
            );
        }
        // Known value: log10(2) for x = 0.0001.
        assert!((compress_value(0.000_1) - std::f32::consts::LOG10_2).abs() < 1e-5);
        // Silence stays silence: zero energy frames compress to zeros.
        assert_eq!(compress_frame(&[0.0; MEL_BANDS]), [0.0; MEL_BANDS]);
    }

    // -- patches -------------------------------------------------------

    #[test]
    fn patch_threshold_stride_and_discard() {
        assert_eq!(patch_count_for_frames(0), 0);
        assert_eq!(patch_count_for_frames(186), 0);
        assert_eq!(patch_count_for_frames(187), 1);
        assert_eq!(patch_count_for_frames(188), 1);
        assert_eq!(patch_count_for_frames(279), 1);
        assert_eq!(patch_count_for_frames(280), 2);
        assert_eq!(patch_count_for_frames(373), 3);
        // End to end: exact 187-frame PCM yields exactly one patch.
        let one = MelFrontend::process_contiguous(&vec![0.0f32; 47_872]);
        assert_eq!(one.mel_frames.len(), 187);
        assert_eq!(one.patches.len(), 1);
        assert_eq!(one.patches[0].frames().len(), 187);
        // 279 frames: the 92-frame tail is discarded, not padded.
        let tail = MelFrontend::process_contiguous(&vec![0.0f32; 256 + 278 * 256]);
        assert_eq!(tail.mel_frames.len(), 279);
        assert_eq!(tail.patches.len(), 1);
    }

    #[test]
    fn patch_serialization_is_deterministic() {
        let output = MelFrontend::process_contiguous(&multitones(100_000));
        assert!(!output.patches.is_empty());
        for patch in &output.patches {
            let first = patch.to_f32le_bytes();
            assert_eq!(first.len(), PATCH_F32LE_BYTES);
            assert_eq!(first.len(), 71_808);
            assert_eq!(first, patch.to_f32le_bytes());
        }
        // Distinct content serializes distinctly.
        assert_ne!(
            output.patches[0].to_f32le_bytes(),
            MelFrontend::process_contiguous(&vec![0.0f32; 47_872]).patches[0].to_f32le_bytes()
        );
    }

    // -- chunk invariance ---------------------------------------------------

    #[test]
    fn chunking_does_not_change_output() {
        let pcm = multitones(200_000);
        let reference = MelFrontend::process_contiguous(&pcm);
        assert!(!reference.patches.is_empty());
        let reference_bytes: Vec<Vec<u8>> = reference
            .patches
            .iter()
            .map(Sonic52MelPatch::to_f32le_bytes)
            .collect();
        // Deliberately awkward chunkings: sub-frame, frame-unaligned,
        // prime sizes, empty chunks, and a trailing dribble.
        let chunkings: &[&[usize]] = &[
            &[200_000],
            &[1],
            &[511, 513, 7],
            &[4096],
            &[1000, 1, 999, 3, 17, 12_345],
            &[13, 29, 101],
        ];
        for sizes in chunkings {
            let mut frontend = MelFrontend::new();
            let mut offset = 0usize;
            let mut size_index = 0usize;
            while offset < pcm.len() {
                let size = sizes[size_index % sizes.len()];
                size_index += 1;
                let end = (offset + size).min(pcm.len());
                frontend.push_chunk(&pcm[offset..end]);
                offset = end;
            }
            // Empty chunks must be harmless.
            frontend.push_chunk(&[]);
            frontend.push_chunk(&[]);
            let output = frontend.finish();
            assert_eq!(output.mel_frames.len(), reference.mel_frames.len());
            assert_eq!(output.patches.len(), reference.patches.len());
            for (patch, expected) in output.patches.iter().zip(reference_bytes.iter()) {
                assert_eq!(&patch.to_f32le_bytes(), expected);
            }
        }
    }

    #[test]
    fn short_and_empty_inputs_yield_no_patches() {
        let empty = MelFrontend::process_contiguous(&[]);
        assert!(empty.mel_frames.is_empty());
        assert!(empty.patches.is_empty());
        let sub_patch = MelFrontend::process_contiguous(&multitones(1000));
        assert!(!sub_patch.mel_frames.is_empty());
        assert!(sub_patch.patches.is_empty());
    }
}
