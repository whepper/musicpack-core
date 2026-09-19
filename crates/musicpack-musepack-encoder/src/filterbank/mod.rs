//! Musepack SV8 analysis filterbank (scalar reference port).
//!
//! This is a faithful, scalar-only port of the reference encoder's analysis
//! path (`codec/libmpcenc/analy_filter.c`): the [`Klemm`](tables) prototype
//! window and modulation matrix, [`AnalysisFilterbank::init`]
//! (`Analyse_Init`) and [`AnalysisFilterbank::process`] (`Analyse_Filter`),
//! together with the vectoring and matrixing kernels.
//!
//! # What it does
//!
//! ```text
//! PCM (L/R, ANABUFFER samples)
//!    │  sliding 32-sample windows, 36 per frame
//!    ▼
//! vectoring  → 32 partial values Y
//!    │
//!    ▼
//! matrixing  → 32 subbands × 36 samples
//! ```
//!
//! The filterbank consumes `ANABUFFER` (= 1600) PCM samples per channel and
//! produces `32 × 36` coefficients per channel. It is independent of sample
//! rate: the reference filterbank uses no rate, only the fixed 1152-sample
//! frame and 480-sample overlap.
//!
//! # Numerical compatibility
//!
//! The compatibility contract is **exact IEEE-754 bit patterns** against the
//! reference, not a tolerance. To preserve the reference operation order this
//! port:
//!
//! * evaluates each eight-term dot product left-to-right and never
//!   reassociates;
//! * sums the two partial products in vectoring in the reference's grouping;
//! * uses `f32` throughout the hot path with no `f64` intermediates and no FMA;
//! * takes the modulation matrix from frozen reference bit patterns because the
//!   reference derives it from libm `cos`.
//!
//! # State
//!
//! All state is per-instance ([`AnalysisFilterbank`] owns both channel
//! histories); there is no global mutable state, so instances are independent.

mod tables;

use crate::error::EncoderError;

pub use tables::{CI_OPT, MODULATION};

/// Number of subbands produced per frame.
pub const SUBBANDS: usize = 32;
/// Number of coefficients per subband per frame.
pub const SUBBAND_SAMPLES: usize = 36;
/// PCM samples per channel consumed by one analysis frame (`ANABUFFER`).
pub const PCM_BLOCK: usize = 1600;
/// Offset at which the reference begins reading the PCM block (`CENTER`).
pub const CENTER: usize = 448;
/// Maximum subband index the reference matrixing can address.
pub const MAX_BAND: usize = SUBBANDS - 1;

/// History length (`X_MEM + 480`), matching the reference `X_L`/`X_R`.
const STATE_LEN: usize = 1632;
/// `X_MEM`: where the previous overlap is copied to.
const X_MEM: usize = 1152;

/// One subband's coefficients for a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Subband {
    /// Left-channel coefficients.
    pub left: [f32; SUBBAND_SAMPLES],
    /// Right-channel coefficients.
    pub right: [f32; SUBBAND_SAMPLES],
}

impl Subband {
    /// A zeroed subband.
    pub const ZERO: Self = Self {
        left: [0.0; SUBBAND_SAMPLES],
        right: [0.0; SUBBAND_SAMPLES],
    };
}

#[derive(Clone, Copy)]
enum Channel {
    Left,
    Right,
}

/// The scalar Musepack analysis filterbank.
///
/// One instance owns the overlap/history state for one stereo stream. Create
/// with [`AnalysisFilterbank::new`], optionally [`reset`](Self::reset), call
/// [`init`](Self::init) once, then [`process`](Self::process) once per frame.
#[derive(Clone, Debug)]
pub struct AnalysisFilterbank {
    left: [f32; STATE_LEN],
    right: [f32; STATE_LEN],
}

impl Default for AnalysisFilterbank {
    fn default() -> Self {
        Self::new()
    }
}

impl AnalysisFilterbank {
    /// Creates a filterbank with zeroed history (the reference's initial state).
    pub fn new() -> Self {
        Self {
            left: [0.0; STATE_LEN],
            right: [0.0; STATE_LEN],
        }
    }

    /// Clears the history, reproducing `mpc_enc_reset_filter`.
    pub fn reset(&mut self) {
        self.left = [0.0; STATE_LEN];
        self.right = [0.0; STATE_LEN];
    }

    /// Runs `Analyse_Init`: primes the history with constant values and writes
    /// the first frame's subbands.
    ///
    /// `max_band` selects how many subbands are written (`0..=31`); higher
    /// indices in `out` are left unchanged, as in the reference.
    pub fn init(
        &mut self,
        left: f32,
        right: f32,
        out: &mut [Subband; SUBBANDS],
        max_band: usize,
    ) -> Result<(), EncoderError> {
        validate_max_band(max_band)?;
        analyse_constant(&mut self.left, left, out, max_band, Channel::Left);
        analyse_constant(&mut self.right, right, out, max_band, Channel::Right);
        Ok(())
    }

    /// Runs `Analyse_Filter` for one frame.
    ///
    /// `pcm_left` and `pcm_right` must each contain at least [`PCM_BLOCK`]
    /// samples; only indices `CENTER..CENTER + 1152` are read, matching the
    /// reference.
    pub fn process(
        &mut self,
        pcm_left: &[f32],
        pcm_right: &[f32],
        out: &mut [Subband; SUBBANDS],
        max_band: usize,
    ) -> Result<(), EncoderError> {
        validate_max_band(max_band)?;
        if pcm_left.len() < PCM_BLOCK {
            return Err(EncoderError::PcmBlockTooShort {
                needed: PCM_BLOCK,
                got: pcm_left.len(),
            });
        }
        if pcm_right.len() < PCM_BLOCK {
            return Err(EncoderError::PcmBlockTooShort {
                needed: PCM_BLOCK,
                got: pcm_right.len(),
            });
        }
        analyse_pcm(&mut self.left, pcm_left, out, max_band, Channel::Left);
        analyse_pcm(&mut self.right, pcm_right, out, max_band, Channel::Right);
        Ok(())
    }
}

fn validate_max_band(max_band: usize) -> Result<(), EncoderError> {
    if max_band > MAX_BAND {
        return Err(EncoderError::InvalidMaxBand(max_band));
    }
    Ok(())
}

/// `Analyse_Init` for one channel.
fn analyse_constant(
    state: &mut [f32; STATE_LEN],
    value: f32,
    out: &mut [Subband; SUBBANDS],
    max_band: usize,
    channel: Channel,
) {
    state.copy_within(0..480, X_MEM);
    for n in 0..SUBBAND_SAMPLES {
        let base = 1120 - 32 * n;
        for slot in &mut state[base..base + 32] {
            *slot = value;
        }
        let mut y = [0.0f32; 32];
        vectoring(&tables::CI_OPT, &state[base..], &mut y);
        matrixing(max_band, &tables::MODULATION, &y, out, n, channel);
    }
}

/// `Analyse_Filter` for one channel.
fn analyse_pcm(
    state: &mut [f32; STATE_LEN],
    pcm: &[f32],
    out: &mut [Subband; SUBBANDS],
    max_band: usize,
    channel: Channel,
) {
    state.copy_within(0..480, X_MEM);
    for n in 0..SUBBAND_SAMPLES {
        let base = 1120 - 32 * n;
        let pcm_base = 479 + 32 * n;
        // The reference's FASTER loop reads the 32 PCM samples in two
        // descending passes: x[0..16] get the first sixteen reads and
        // x[16..32] the next sixteen. The result is an interleaved layout,
        // not a simple reversal.
        for j in 0..16 {
            state[base + j] = pcm[pcm_base - j];
        }
        for j in 16..32 {
            state[base + j] = pcm[pcm_base - 47 + j];
        }
        let mut y = [0.0f32; 32];
        vectoring(&tables::CI_OPT, &state[base..], &mut y);
        matrixing(max_band, &tables::MODULATION, &y, out, n, channel);
    }
}

/// The reference `EXPR`: eight taps stride 64, evaluated left-to-right.
#[inline]
fn expr(ci: &[f32; 512], c: usize, x: &[f32], xo: usize) -> f32 {
    ci[c] * x[xo]
        + ci[c + 1] * x[xo + 64]
        + ci[c + 2] * x[xo + 128]
        + ci[c + 3] * x[xo + 192]
        + ci[c + 4] * x[xo + 256]
        + ci[c + 5] * x[xo + 320]
        + ci[c + 6] * x[xo + 384]
        + ci[c + 7] * x[xo + 448]
}

/// `Vectoring_scalar` (the `FASTER` formulation), written as the reference's
/// three passes. The loop bounds are the reference control flow unfolded:
/// `y[0]`, then fifteen `y[m]` whose pointers are derived from `m`, then
/// `y[16]`, then fifteen more.
/// `Vectoring_scalar` (the `FASTER` formulation), written as the reference's
/// three passes. The loop bounds are the reference control flow unfolded:
/// `y[0]`, then fifteen `y[m]` whose pointers are derived from `m`, then
/// `y[16]`, then fifteen more.
///
/// Performance note (15H.3, validated as experiment A2 in 15H.2): the four
/// independent per-output accumulator chains are interleaved so the CPU can
/// overlap them (instruction-level parallelism). This is purely structural:
/// every output still accumulates its taps `k = 0..7` left-to-right and then
/// adds `expr_a + expr_b`, exactly as the scalar reference does, so the
/// floating-point bits are identical. In particular `y[16]` keeps the two
/// 8-tap chains separate (`e1 + e2`); fusing them into one 16-term chain
/// would reassociate across the `expr` boundary and change rounding. See
/// `vectoring_scalar` (tests) for the reference formulation the production
/// path is gated against.
fn vectoring(ci: &[f32; 512], x: &[f32], y: &mut [f32; 32]) {
    #[inline]
    fn tap(ci: &[f32], c: usize, x: &[f32], xo: usize, k: usize) -> f32 {
        ci[c + k] * x[xo + 64 * k]
    }
    y[0] = tap(ci, 128, x, 31, 0)
        + tap(ci, 128, x, 31, 1)
        + tap(ci, 128, x, 31, 2)
        + tap(ci, 128, x, 31, 3)
        + tap(ci, 128, x, 31, 4)
        + tap(ci, 128, x, 31, 5)
        + tap(ci, 128, x, 31, 6)
        + tap(ci, 128, x, 31, 7);

    let mut m = 1usize;
    while m + 4 <= 16 {
        let mut a0 = tap(ci, 8 * (m - 1), x, 16 - m, 0);
        let mut a1 = tap(ci, 8 * m, x, 15 - m, 0);
        let mut a2 = tap(ci, 8 * (m + 1), x, 14 - m, 0);
        let mut a3 = tap(ci, 8 * (m + 2), x, 13 - m, 0);
        let mut b0 = tap(ci, 136 + 8 * (m - 1), x, 31 - m, 0);
        let mut b1 = tap(ci, 136 + 8 * m, x, 30 - m, 0);
        let mut b2 = tap(ci, 136 + 8 * (m + 1), x, 29 - m, 0);
        let mut b3 = tap(ci, 136 + 8 * (m + 2), x, 28 - m, 0);
        for k in 1..8 {
            a0 += tap(ci, 8 * (m - 1), x, 16 - m, k);
            a1 += tap(ci, 8 * m, x, 15 - m, k);
            a2 += tap(ci, 8 * (m + 1), x, 14 - m, k);
            a3 += tap(ci, 8 * (m + 2), x, 13 - m, k);
            b0 += tap(ci, 136 + 8 * (m - 1), x, 31 - m, k);
            b1 += tap(ci, 136 + 8 * m, x, 30 - m, k);
            b2 += tap(ci, 136 + 8 * (m + 1), x, 29 - m, k);
            b3 += tap(ci, 136 + 8 * (m + 2), x, 28 - m, k);
        }
        y[m] = a0 + b0;
        y[m + 1] = a1 + b1;
        y[m + 2] = a2 + b2;
        y[m + 3] = a3 + b3;
        m += 4;
    }
    while m < 16 {
        y[m] = expr(ci, 8 * (m - 1), x, 16 - m) + expr(ci, 136 + 8 * (m - 1), x, 31 - m);
        m += 1;
    }

    let e1 = tap(ci, 120, x, 0, 0)
        + tap(ci, 120, x, 0, 1)
        + tap(ci, 120, x, 0, 2)
        + tap(ci, 120, x, 0, 3)
        + tap(ci, 120, x, 0, 4)
        + tap(ci, 120, x, 0, 5)
        + tap(ci, 120, x, 0, 6)
        + tap(ci, 120, x, 0, 7);
    let e2 = tap(ci, 256, x, 32, 0)
        + tap(ci, 256, x, 32, 1)
        + tap(ci, 256, x, 32, 2)
        + tap(ci, 256, x, 32, 3)
        + tap(ci, 256, x, 32, 4)
        + tap(ci, 256, x, 32, 5)
        + tap(ci, 256, x, 32, 6)
        + tap(ci, 256, x, 32, 7);
    y[16] = e1 + e2;

    let mut m = 17usize;
    while m + 4 <= 32 {
        let mut a0 = tap(ci, 384 + 8 * (m - 17), x, 48 + (m - 17), 0);
        let mut a1 = tap(ci, 384 + 8 * (m - 16), x, 49 + (m - 17), 0);
        let mut a2 = tap(ci, 384 + 8 * (m - 15), x, 50 + (m - 17), 0);
        let mut a3 = tap(ci, 384 + 8 * (m - 14), x, 51 + (m - 17), 0);
        let mut b0 = tap(ci, 264 + 8 * (m - 17), x, 33 + (m - 17), 0);
        let mut b1 = tap(ci, 264 + 8 * (m - 16), x, 34 + (m - 17), 0);
        let mut b2 = tap(ci, 264 + 8 * (m - 15), x, 35 + (m - 17), 0);
        let mut b3 = tap(ci, 264 + 8 * (m - 14), x, 36 + (m - 17), 0);
        for k in 1..8 {
            a0 += tap(ci, 384 + 8 * (m - 17), x, 48 + (m - 17), k);
            a1 += tap(ci, 384 + 8 * (m - 16), x, 49 + (m - 17), k);
            a2 += tap(ci, 384 + 8 * (m - 15), x, 50 + (m - 17), k);
            a3 += tap(ci, 384 + 8 * (m - 14), x, 51 + (m - 17), k);
            b0 += tap(ci, 264 + 8 * (m - 17), x, 33 + (m - 17), k);
            b1 += tap(ci, 264 + 8 * (m - 16), x, 34 + (m - 17), k);
            b2 += tap(ci, 264 + 8 * (m - 15), x, 35 + (m - 17), k);
            b3 += tap(ci, 264 + 8 * (m - 14), x, 36 + (m - 17), k);
        }
        y[m] = a0 + b0;
        y[m + 1] = a1 + b1;
        y[m + 2] = a2 + b2;
        y[m + 3] = a3 + b3;
        m += 4;
    }
    while m < 32 {
        y[m] = expr(ci, 384 + 8 * (m - 17), x, 48 + (m - 17))
            + expr(ci, 264 + 8 * (m - 17), x, 33 + (m - 17));
        m += 1;
    }
}

/// `Matrixing_scalar` (the `FASTER` formulation): `y[0] + Σ mi[k]·y[k]`,
/// left-to-right, for each band `0..=max_band`.
fn matrixing(
    max_band: usize,
    m: &[f32; 1024],
    y: &[f32; 32],
    out: &mut [Subband; SUBBANDS],
    n: usize,
    channel: Channel,
) {
    for (band, mi) in m.chunks_exact(32).take(max_band + 1).enumerate() {
        let value = y[0]
            + mi[1] * y[1]
            + mi[2] * y[2]
            + mi[3] * y[3]
            + mi[4] * y[4]
            + mi[5] * y[5]
            + mi[6] * y[6]
            + mi[7] * y[7]
            + mi[8] * y[8]
            + mi[9] * y[9]
            + mi[10] * y[10]
            + mi[11] * y[11]
            + mi[12] * y[12]
            + mi[13] * y[13]
            + mi[14] * y[14]
            + mi[15] * y[15]
            + mi[16] * y[16]
            + mi[17] * y[17]
            + mi[18] * y[18]
            + mi[19] * y[19]
            + mi[20] * y[20]
            + mi[21] * y[21]
            + mi[22] * y[22]
            + mi[23] * y[23]
            + mi[24] * y[24]
            + mi[25] * y[25]
            + mi[26] * y[26]
            + mi[27] * y[27]
            + mi[28] * y[28]
            + mi[29] * y[29]
            + mi[30] * y[30]
            + mi[31] * y[31];
        match channel {
            Channel::Left => out[band].left[n] = value,
            Channel::Right => out[band].right[n] = value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(value: f32) -> Vec<f32> {
        let mut v = vec![0.0; PCM_BLOCK];
        for slot in &mut v[CENTER..] {
            *slot = value;
        }
        v
    }

    #[test]
    #[ignore = "micro-benchmark; run with --release --ignored --nocapture"]
    fn filterbank_throughput_baseline() {
        use std::time::Instant;
        let frames = 20_000usize;
        let mut left = vec![0.0f32; PCM_BLOCK];
        let mut right = vec![0.0f32; PCM_BLOCK];
        for (i, slot) in left[CENTER..].iter_mut().enumerate() {
            *slot = ((i % 512) as f32 - 256.0) / 512.0;
        }
        for (i, slot) in right[CENTER..].iter_mut().enumerate() {
            *slot = ((i % 256) as f32 - 128.0) / 256.0;
        }
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        fb.init(0.0, 0.0, &mut out, MAX_BAND).unwrap();
        let start = Instant::now();
        for _ in 0..frames {
            fb.process(&left, &right, &mut out, MAX_BAND).unwrap();
        }
        let elapsed = start.elapsed();
        println!(
            "{frames} frames in {elapsed:?} -> {:.2} us/frame",
            elapsed.as_secs_f64() * 1e6 / frames as f64
        );
    }

    #[test]
    fn zero_init_yields_zero_output() {
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        fb.init(0.0, 0.0, &mut out, MAX_BAND).unwrap();
        for (band, sb) in out.iter().enumerate() {
            assert_eq!(sb, &Subband::ZERO, "band {band}");
        }
    }

    #[test]
    fn process_of_silence_after_zero_init_is_zero() {
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        fb.init(0.0, 0.0, &mut out, MAX_BAND).unwrap();
        fb.process(&pcm(0.0), &pcm(0.0), &mut out, MAX_BAND)
            .unwrap();
        for (band, sb) in out.iter().enumerate() {
            assert_eq!(sb, &Subband::ZERO, "band {band}");
        }
    }

    #[test]
    fn instances_are_independent() {
        let mut a = AnalysisFilterbank::new();
        let mut b = AnalysisFilterbank::new();
        let mut out_a = [Subband::ZERO; SUBBANDS];
        let mut out_b = [Subband::ZERO; SUBBANDS];

        a.init(1.0, 1.0, &mut out_a, MAX_BAND).unwrap();
        a.process(&pcm(1.0), &pcm(1.0), &mut out_a, MAX_BAND)
            .unwrap();

        b.init(-0.25, 0.75, &mut out_b, MAX_BAND).unwrap();
        b.process(&pcm(-0.25), &pcm(0.75), &mut out_b, MAX_BAND)
            .unwrap();

        assert_ne!(out_a, out_b);
    }

    #[test]
    fn reset_restores_the_initial_state() {
        let mut fb = AnalysisFilterbank::new();
        let mut first = [Subband::ZERO; SUBBANDS];
        fb.init(0.5, 0.5, &mut first, MAX_BAND).unwrap();
        fb.process(&pcm(0.5), &pcm(0.5), &mut first, MAX_BAND)
            .unwrap();

        fb.reset();
        let mut second = [Subband::ZERO; SUBBANDS];
        fb.init(0.5, 0.5, &mut second, MAX_BAND).unwrap();
        fb.process(&pcm(0.5), &pcm(0.5), &mut second, MAX_BAND)
            .unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn repeated_process_evolves_state() {
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        fb.init(0.0, 0.0, &mut out, MAX_BAND).unwrap();
        fb.process(&pcm(0.5), &pcm(0.5), &mut out, MAX_BAND)
            .unwrap();
        let first = out;
        fb.process(&pcm(0.5), &pcm(0.5), &mut out, MAX_BAND)
            .unwrap();
        assert_ne!(first, out, "state must carry across frames");
    }

    #[test]
    fn max_band_limits_written_subbands() {
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        fb.init(0.5, 0.5, &mut out, 3).unwrap();
        assert_ne!(out[0], Subband::ZERO);
        assert_eq!(out[4], Subband::ZERO, "bands above max_band stay untouched");
    }

    #[test]
    fn rejects_out_of_range_arguments() {
        let mut fb = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];
        assert_eq!(
            fb.init(0.0, 0.0, &mut out, 32),
            Err(EncoderError::InvalidMaxBand(32))
        );
        assert_eq!(
            fb.process(&[0.0; 10], &pcm(0.0), &mut out, MAX_BAND),
            Err(EncoderError::PcmBlockTooShort {
                needed: PCM_BLOCK,
                got: 10
            })
        );
        assert_eq!(
            fb.process(&pcm(0.0), &[0.0; 0], &mut out, MAX_BAND),
            Err(EncoderError::PcmBlockTooShort {
                needed: PCM_BLOCK,
                got: 0
            })
        );
    }

    // -- 15H.2 candidate micro-benchmarks (measurement only) -----------------
    //
    // Variant implementations live here (bench-side of the investigation) so
    // no temporary public API is needed. Each A/B test asserts bit-identical
    // outputs before timing, interleaves baseline/variant batches in one run,
    // and reports speedups. Variants evaluated and abandoned stay documented
    // here to prevent re-investigation.
    use std::time::{Duration, Instant};

    fn lcg(state: &mut u32) -> u32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *state
    }

    fn fill_f32(seed: &mut u32, out: &mut [f32]) {
        for slot in out.iter_mut() {
            *slot = (lcg(seed) >> 9) as f32 / 8388608.0 - 1.0;
        }
    }

    fn sink_f32(data: &[f32]) -> f32 {
        let mut acc = 0.0f32;
        for (i, v) in data.iter().enumerate() {
            acc += *v * (i as f32 + 1.0);
        }
        acc
    }

    fn ab_measure(
        warmup_iters: usize,
        iters: usize,
        batch: usize,
        mut base: impl FnMut(),
        mut var: impl FnMut(),
    ) -> (Duration, Duration, Duration, Duration) {
        fn minimum(v: &[Duration]) -> Duration {
            *v.iter().min().unwrap()
        }
        fn median(mut v: Vec<Duration>) -> Duration {
            v.sort_unstable();
            v[v.len() / 2]
        }
        for _ in 0..warmup_iters {
            base();
            var();
        }
        let mut tb = Vec::with_capacity(iters);
        let mut tv = Vec::with_capacity(iters);
        for _ in 0..iters {
            let t0 = Instant::now();
            for _ in 0..batch {
                base();
            }
            tb.push(t0.elapsed());
            let t0 = Instant::now();
            for _ in 0..batch {
                var();
            }
            tv.push(t0.elapsed());
        }
        (minimum(&tb), median(tb), minimum(&tv), median(tv))
    }

    /// EXP-15H.2-A1 (abandoned): matrixing with the channel branch hoisted.
    /// Identical arithmetic order. Measured 0.95x (slower); kept here only
    /// as a documented negative result.
    fn matrixing_hoisted(
        max_band: usize,
        m: &[f32; 1024],
        y: &[f32; 32],
        out: &mut [Subband; SUBBANDS],
        n: usize,
        channel: Channel,
    ) {
        let dot = |mi: &[f32]| {
            y[0] + mi[1] * y[1]
                + mi[2] * y[2]
                + mi[3] * y[3]
                + mi[4] * y[4]
                + mi[5] * y[5]
                + mi[6] * y[6]
                + mi[7] * y[7]
                + mi[8] * y[8]
                + mi[9] * y[9]
                + mi[10] * y[10]
                + mi[11] * y[11]
                + mi[12] * y[12]
                + mi[13] * y[13]
                + mi[14] * y[14]
                + mi[15] * y[15]
                + mi[16] * y[16]
                + mi[17] * y[17]
                + mi[18] * y[18]
                + mi[19] * y[19]
                + mi[20] * y[20]
                + mi[21] * y[21]
                + mi[22] * y[22]
                + mi[23] * y[23]
                + mi[24] * y[24]
                + mi[25] * y[25]
                + mi[26] * y[26]
                + mi[27] * y[27]
                + mi[28] * y[28]
                + mi[29] * y[29]
                + mi[30] * y[30]
                + mi[31] * y[31]
        };
        match channel {
            Channel::Left => {
                for (band, mi) in m.chunks_exact(32).take(max_band + 1).enumerate() {
                    out[band].left[n] = dot(mi);
                }
            }
            Channel::Right => {
                for (band, mi) in m.chunks_exact(32).take(max_band + 1).enumerate() {
                    out[band].right[n] = dot(mi);
                }
            }
        }
    }

    /// Semantic oracle for `vectoring` (15H.3): the original scalar
    /// formulation, one output at a time, exactly as the reference loop
    /// computes it. Production `vectoring` interleaves four independent
    /// per-output accumulators for ILP but must produce bit-identical
    /// results; this function is what the permanent regression test gates
    /// the production path against. Never used in production.
    fn vectoring_scalar(ci: &[f32; 512], x: &[f32], y: &mut [f32; 32]) {
        y[0] = expr(ci, 128, x, 31);

        for (m, slot) in y.iter_mut().enumerate().take(16).skip(1) {
            let c1 = 8 * (m - 1);
            let c2 = 136 + 8 * (m - 1);
            let x1 = 16 - m;
            let x2 = 31 - m;
            *slot = expr(ci, c1, x, x1) + expr(ci, c2, x, x2);
        }

        y[16] = expr(ci, 120, x, 0) + expr(ci, 256, x, 32);

        for (m, slot) in y.iter_mut().enumerate().take(32).skip(17) {
            let c1 = 384 + 8 * (m - 17);
            let c2 = 264 + 8 * (m - 17);
            let x1 = 48 + (m - 17);
            let x2 = 33 + (m - 17);
            *slot = expr(ci, c1, x, x1) + expr(ci, c2, x, x2);
        }
    }

    #[test]
    #[ignore = "benchmark; run with --release --ignored --nocapture"]
    fn bench_matrixing_ab() {
        let mut seed = 0x1234_5678u32;
        let mut y = [0.0f32; 32];
        fill_f32(&mut seed, &mut y);
        let mut out_base = [Subband::ZERO; SUBBANDS];
        let mut out_var = [Subband::ZERO; SUBBANDS];
        matrixing(28, &tables::MODULATION, &y, &mut out_base, 0, Channel::Left);
        matrixing_hoisted(28, &tables::MODULATION, &y, &mut out_var, 0, Channel::Left);
        assert_eq!(out_base, out_var, "hoisted matrixing diverged");

        let (bmin, bmed, vmin, vmed) = ab_measure(
            3,
            15,
            72,
            || {
                matrixing(28, &tables::MODULATION, &y, &mut out_base, 0, Channel::Left);
                std::hint::black_box(sink_f32(&out_base[0].left));
            },
            || {
                matrixing_hoisted(28, &tables::MODULATION, &y, &mut out_var, 0, Channel::Left);
                std::hint::black_box(sink_f32(&out_var[0].left));
            },
        );
        println!("matrixing A/B per call (max_band=28):");
        println!(
            "  baseline {:>10.3?}/call | hoisted {:>10.3?}/call | speedup {:.3}x",
            bmed / 72,
            vmed / 72,
            bmed.as_secs_f64() / vmed.as_secs_f64(),
        );
        println!("  (min: {bmin:?} vs {vmin:?})");
    }

    /// Bit-identity gate: production `vectoring` must match the scalar
    /// oracle bit-for-bit on the given state vector.
    fn check_vectoring_case(x: &[f32], context: &str) {
        let mut y_prod = [0.0f32; 32];
        let mut y_ref = [0.0f32; 32];
        vectoring(&tables::CI_OPT, x, &mut y_prod);
        vectoring_scalar(&tables::CI_OPT, x, &mut y_ref);
        assert_eq!(
            y_prod.map(f32::to_bits),
            y_ref.map(f32::to_bits),
            "production vectoring diverged from scalar oracle ({context})"
        );
    }

    /// Diverse input classes for the identity gate: normal magnitudes, very
    /// small/large values, exact zeros, and DC-like constant input.
    fn check_vectoring_trials() {
        for trial in 0..200u32 {
            let mut seed = 0x1234_5678u32.wrapping_add(trial.wrapping_mul(0x9E37_79B1));
            let mut xt = vec![0.0f32; 512 + 448];
            fill_f32(&mut seed, &mut xt);
            let scale = match trial % 4 {
                0 => 1.0,
                1 => 1.0e-20,
                _ => 1.0e20,
            };
            for slot in xt.iter_mut() {
                *slot *= scale;
            }
            if trial % 7 == 0 {
                for slot in xt.iter_mut() {
                    *slot = 0.0;
                }
            }
            if trial % 11 == 0 {
                for slot in xt.iter_mut() {
                    *slot = 0.5;
                }
            }
            check_vectoring_case(&xt, &format!("trial {trial}"));
        }
    }

    #[test]
    fn vectoring_matches_scalar_reference() {
        check_vectoring_trials();
    }

    #[test]
    #[ignore = "benchmark; run with --release --ignored --nocapture"]
    fn bench_vectoring_ab() {
        let mut seed = 0x1234_5678u32;
        let mut x = vec![0.0f32; 512 + 448];
        fill_f32(&mut seed, &mut x);
        for (i, slot) in x.iter_mut().enumerate() {
            *slot *= 1.0 / (1.0 + (i / 64) as f32);
        }
        // Bit-identity gate over diverse inputs (magnitudes, zeros, DC).
        check_vectoring_trials();

        let mut y_base = [0.0f32; 32];
        let mut y_var = [0.0f32; 32];
        let (vmin_b, vmed_b, vmin_v, vmed_v) = ab_measure(
            3,
            15,
            72,
            || {
                vectoring_scalar(&tables::CI_OPT, &x, &mut y_base);
                std::hint::black_box(sink_f32(&y_base));
            },
            || {
                vectoring(&tables::CI_OPT, &x, &mut y_var);
                std::hint::black_box(sink_f32(&y_var));
            },
        );
        println!("vectoring A/B per call (72 calls/batch):");
        println!(
            "  scalar   {:>10.3?}/call | joint-4 {:>10.3?}/call | speedup {:.3}x",
            vmed_b / 72,
            vmed_v / 72,
            vmed_b.as_secs_f64() / vmed_v.as_secs_f64(),
        );
        println!("  (min: {vmin_b:?} vs {vmin_v:?})");
    }
}
