//! BS.1770-5 integrated loudness and true peak.
//!
//! Port of the subset of the reference's vendored **libebur128 1.2.6**
//! (MIT-licensed; `core/libmusicpack/vendor/ebur128`, copyright © 2011 Jan
//! Kokemüller) that `core/libmusicpack/src/loudness.c` actually exercises:
//!
//! - 5th-order K-weighting (a high-shelf biquad cascaded with the RLB
//!   high-pass biquad), `f64` state, rate-dependent coefficients computed
//!   with the reference's `tan`/`pow` formulas and the reference's state
//!   update order;
//! - 400 ms first gating block, then a 100 ms hop, with state persisting
//!   across [`LoudnessMeter::process`] calls;
//! - absolute gating at −70 LUFS via the reference energy threshold
//!   `10^((−70 + 0.691)/10)`, then the −10 LU relative gate;
//! - integrated loudness `10·log10(mean gated energy) − 0.691`;
//! - 49-tap polyphase true-peak interpolation at 4× (< 96 kHz), 2×
//!   (96 kHz ≤ rate < 192 kHz) or none (≥ 192 kHz), per-channel running
//!   maxima merged with the sample peak; dBTP floored at −70.
//!
//! Momentary, short-term, LRA, histogram, window/history and
//! `change_parameters` machinery are deliberately **not** ported — MusicPack
//! never calls them.
//!
//! # Scope and caller contract
//!
//! Only channels 1–2 and sample rates 16…2 822 400 Hz are accepted (the
//! reference's `ebur128_init` bounds; mono is a single LEFT channel, stereo
//! is LEFT/RIGHT). Album loudness is **caller policy**: create one meter and
//! feed every track in manifest order into it; never average per-track
//! values. The reference silently mismeasures an album whose tracks differ
//! in sample rate or channel count (the first track's configuration wins);
//! Phase 9 documents this as a caller contract rather than detecting it.
//!
//! # Numerical equivalence
//!
//! Loudness is compared to the reference within ±0.05 LU / ±0.05 dBTP, not
//! bit-for-bit: libm (`tan`/`pow`/`log10`) and C `long double` vary by
//! platform. The algorithm structure listed above is reproduced exactly.
//!
//! # Memory
//!
//! Like the reference (default `history = ULONG_MAX`), the meter retains one
//! gating-block energy per 100 ms for the whole program (8 bytes per 100 ms,
//! ≈ 288 KiB/hour). The running true-peak and filter state are constant.
//! This is intentional: it is the price of a gated integrated measurement.

use std::f64::consts::PI;

use super::sanitize_sample;
use crate::{Error, Result};

/// The BS.1770 revision string the reference writes into an album
/// `loudness.algorithm` field (`MUSICPACK_LOUDNESS_STANDARD`).
pub const STANDARD: &str = "ITU-R BS.1770-5";

/// Below this magnitude a sinc/interpolation coefficient is treated as zero
/// (ebur128 `ALMOST_ZERO`).
const ALMOST_ZERO: f64 = 0.000_001;

/// Absolute gating floor in LUFS (the reference energy threshold uses this).
const ABSOLUTE_GATE_DB: f64 = -70.0;

/// The BS.1770 offset applied when converting energy to LUFS.
const ENERGY_OFFSET: f64 = 0.691;

/// Relative gate below the ungated mean (LU), from ebur128's global state.
const RELATIVE_GATE_DB: f64 = -10.0;

/// Output floor in dB applied to loudness and true peak.
const FLOOR_DB: f64 = -70.0;

/// Minimum accepted sample rate (`ebur128_init`).
const MIN_SAMPLE_RATE: u32 = 16;

/// Maximum accepted sample rate (`ebur128_init`).
const MAX_SAMPLE_RATE: u32 = 2_822_400;

/// Polyphase interpolator tap count.
const TAPS: usize = 49;

/// Measured loudness of a program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated (gated) loudness in LUFS, floored at −70.
    pub lufs: f64,
    /// Running maximum true peak in dBTP, floored at −70.
    pub true_peak_db_tp: f64,
}

/// A streaming BS.1770-5 meter (integrated loudness + true peak).
///
/// See the [module documentation](self) for scope, channel/rate limits and
/// numerical equivalence.
pub struct LoudnessMeter {
    channels: usize,
    /// `(rate + 5) / 10`, ebur128's 100 ms sample count.
    samples_in_100ms: usize,
    /// 400 ms of samples: the first block size and the gating window.
    frames_per_block: usize,
    /// K-weighted sample ring (channel-interleaved), length `ring_frames × ch`.
    ring: Vec<f64>,
    /// Flat write index into `ring` (a multiple of `channels`).
    ring_index: usize,
    /// Per-channel 5-tap filter state (`v[0]` newest).
    state: Vec<[f64; 5]>,
    /// Combined K-weighting denominator coefficients.
    a: [f64; 5],
    /// Combined K-weighting numerator coefficients.
    b: [f64; 5],
    /// Frames still needed to complete the next gating block.
    needed_frames: usize,
    /// Absolute-gated block energies (one per 100 ms).
    blocks: Vec<f64>,
    /// Absolute gate energy threshold.
    abs_threshold: f64,
    /// Running maximum |sample|.
    sample_peak: f64,
    /// Running maximum interpolated true peak.
    interp_peak: f64,
    /// Polyphase interpolator, absent at ≥ 192 kHz.
    interp: Option<Interpolator>,
    /// Scratch input for the interpolator, `frames_per_block × channels`.
    resampler_input: Vec<f32>,
}

impl LoudnessMeter {
    /// Creates a meter for a mono or stereo stream.
    ///
    /// Rejects channel counts other than 1–2 and sample rates outside
    /// `16..=2_822_400` with [`Error::Invalid`] (the reference's
    /// `ebur128_init` constraints).
    pub fn new(channels: u8, sample_rate: u32) -> Result<Self> {
        if channels == 0 || channels > 2 {
            return Err(Error::Invalid {
                detail: "loudness meter requires 1 or 2 channels".to_string(),
            });
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(Error::Invalid {
                detail: format!(
                    "loudness meter sample rate {sample_rate} outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}"
                ),
            });
        }
        let channels = channels as usize;
        let samples_in_100ms = ((sample_rate + 5) / 10) as usize;
        let frames_per_block = samples_in_100ms * 4;

        // ebur128 sizes its ring to `rate·window/1000` rounded up to a
        // multiple of `samples_in_100ms`; the reference always lands on
        // `frames_per_block` for any usable rate. The `max` keeps
        // pathological low rates (where the C ring would be smaller than one
        // block) safe instead of out-of-bounds.
        let mut ring_frames = (sample_rate as usize * 400) / 1000;
        if ring_frames % samples_in_100ms != 0 {
            ring_frames = ring_frames + samples_in_100ms - (ring_frames % samples_in_100ms);
        }
        if ring_frames < frames_per_block {
            ring_frames = frames_per_block;
        }

        let (a, b) = k_weighting_coefficients(sample_rate);

        let interp = if sample_rate < 96_000 {
            Some(Interpolator::new(4, channels))
        } else if sample_rate < 192_000 {
            Some(Interpolator::new(2, channels))
        } else {
            None
        };
        let resampler_input = if interp.is_some() {
            vec![0.0f32; frames_per_block * channels]
        } else {
            Vec::new()
        };

        Ok(Self {
            channels,
            samples_in_100ms,
            frames_per_block,
            ring: vec![0.0; ring_frames * channels],
            ring_index: 0,
            state: vec![[0.0; 5]; channels],
            a,
            b,
            needed_frames: frames_per_block,
            blocks: Vec::new(),
            abs_threshold: 10f64.powf((ABSOLUTE_GATE_DB + ENERGY_OFFSET) / 10.0),
            sample_peak: 0.0,
            interp_peak: 0.0,
            interp,
            resampler_input,
        })
    }

    /// Feeds interleaved `f32` PCM (see the module contract).
    ///
    /// `interleaved.len()` must be a multiple of the channel count, else
    /// [`Error::Invalid`]. Empty input is valid and a no-op. Block and
    /// filter state persist across calls, so results never depend on the
    /// caller's buffer size.
    pub fn process(&mut self, interleaved: &[f32]) -> Result<()> {
        let channels = self.channels;
        if interleaved.len() % channels != 0 {
            return Err(Error::Invalid {
                detail: "loudness input length is not a multiple of the channel count".to_string(),
            });
        }
        let mut remaining = interleaved.len() / channels;
        if remaining == 0 {
            return Ok(());
        }
        let mut frame = 0usize;
        while remaining > 0 {
            if remaining >= self.needed_frames {
                let n = self.needed_frames;
                self.filter_chunk(&interleaved[frame * channels..(frame + n) * channels], n);
                frame += n;
                remaining -= n;
                self.ring_index += n * channels;
                self.calc_gating_block();
                self.needed_frames = self.samples_in_100ms;
                if self.ring_index == self.ring.len() {
                    self.ring_index = 0;
                }
            } else {
                let n = remaining;
                self.filter_chunk(&interleaved[frame * channels..], n);
                self.ring_index += n * channels;
                self.needed_frames -= n;
                remaining = 0;
            }
        }
        Ok(())
    }

    /// Returns the integrated loudness and running true peak. Repeatable.
    pub fn result(&self) -> Loudness {
        let lufs = self.integrated_lufs();
        let peak_linear = self.sample_peak.max(self.interp_peak);
        let true_peak_db_tp = if peak_linear <= 0.0 {
            FLOOR_DB
        } else {
            let db = 20.0 * peak_linear.log10();
            if db < FLOOR_DB { FLOOR_DB } else { db }
        };
        Loudness {
            lufs,
            true_peak_db_tp,
        }
    }

    /// Applies the reference's two-stage gating to the block energies.
    fn integrated_lufs(&self) -> f64 {
        if self.blocks.is_empty() {
            return FLOOR_DB;
        }
        let count = self.blocks.len() as f64;
        let sum: f64 = self.blocks.iter().sum();
        let relative_threshold = (sum / count) * 10f64.powf(RELATIVE_GATE_DB / 10.0);

        let mut included = 0usize;
        let mut energy_sum = 0.0f64;
        for &energy in &self.blocks {
            if energy >= relative_threshold {
                included += 1;
                energy_sum += energy;
            }
        }
        if included == 0 {
            return FLOOR_DB;
        }
        let mean = energy_sum / included as f64;
        let lufs = 10.0 * mean.log10() - ENERGY_OFFSET;
        if !lufs.is_finite() || lufs < FLOOR_DB {
            FLOOR_DB
        } else {
            lufs
        }
    }

    /// Filters one chunk of up to `frames_per_block` frames and updates the
    /// sample/true peaks and the K-weighted ring.
    fn filter_chunk(&mut self, src: &[f32], frames: usize) {
        let channels = self.channels;

        // Sample peak (running max; the reference's per-call reset+merge is
        // equivalent because max is associative).
        for i in 0..frames {
            let base = i * channels;
            for c in 0..channels {
                let value = sanitize_sample(src[base + c]).abs() as f64;
                if value > self.sample_peak {
                    self.sample_peak = value;
                }
            }
        }

        // True peak.
        if let Some(interp) = self.interp.as_mut() {
            for i in 0..frames {
                let base = i * channels;
                for c in 0..channels {
                    self.resampler_input[base + c] = sanitize_sample(src[base + c]);
                }
            }
            let peak = interp.max_abs(&self.resampler_input[..frames * channels]);
            if peak > self.interp_peak {
                self.interp_peak = peak;
            }
        }

        // K-weighting, written into the ring in channel-interleaved order.
        let index = self.ring_index;
        for c in 0..channels {
            let state = &mut self.state[c];
            for i in 0..frames {
                let x = sanitize_sample(src[i * channels + c]) as f64;
                state[0] = x
                    - self.a[1] * state[1]
                    - self.a[2] * state[2]
                    - self.a[3] * state[3]
                    - self.a[4] * state[4];
                self.ring[index + i * channels + c] = self.b[0] * state[0]
                    + self.b[1] * state[1]
                    + self.b[2] * state[2]
                    + self.b[3] * state[3]
                    + self.b[4] * state[4];
                state[4] = state[3];
                state[3] = state[2];
                state[2] = state[1];
                state[1] = state[0];
            }
        }
    }

    /// Computes the energy of the most recent 400 ms and applies the
    /// absolute gate, mirroring `ebur128_calc_gating_block`.
    fn calc_gating_block(&mut self) {
        let channels = self.channels;
        let frames_per_block = self.frames_per_block;
        let ring_frames = self.ring.len() / channels;
        let write_frame = self.ring_index / channels;
        let mut sum = 0.0f64;
        for c in 0..channels {
            let mut channel_sum = 0.0f64;
            for k in 0..frames_per_block {
                let frame = (write_frame + ring_frames - frames_per_block + k) % ring_frames;
                let sample = self.ring[frame * channels + c];
                channel_sum += sample * sample;
            }
            sum += channel_sum;
        }
        sum /= frames_per_block as f64;
        if sum >= self.abs_threshold {
            self.blocks.push(sum);
        }
    }
}

/// Reference K-weighting coefficients (`ebur128_init_filter`).
///
/// The high-shelf (preamplification) biquad `pb`/`pa` and the RLB high-pass
/// biquad `rb`/`ra` are convolved into the single 5th-order `b`/`a` used by
/// the direct-form filter.
fn k_weighting_coefficients(sample_rate: u32) -> ([f64; 5], [f64; 5]) {
    let rate = sample_rate as f64;

    // High shelf (BS.1770 pre-filter).
    let f0 = 1_681.974_450_955_533;
    let g = 3.999_843_853_973_347;
    let q = 0.707_175_236_955_419_6;
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let pb = [
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
    ];
    let pa = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];

    // RLB high-pass.
    let f0 = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (PI * f0 / rate).tan();
    let rb = [1.0, -2.0, 1.0];
    let ra = [
        1.0,
        2.0 * (k * k - 1.0) / (1.0 + k / q + k * k),
        (1.0 - k / q + k * k) / (1.0 + k / q + k * k),
    ];

    // Convolve the two biquads.
    let mut b = [0.0f64; 5];
    let mut a = [0.0f64; 5];
    for i in 0..3 {
        for j in 0..3 {
            b[i + j] += pb[i] * rb[j];
            a[i + j] += pa[i] * ra[j];
        }
    }
    (a, b)
}

/// One polyphase sub-filter: tap delays and coefficients.
struct SubFilter {
    index: Vec<usize>,
    coeff: Vec<f64>,
}

/// 49-tap polyphase interpolator (`interp_create` / `interp_process`).
///
/// z buffers are `f32` and accumulators `f64`, matching ebur128.
struct Interpolator {
    delay: usize,
    channels: usize,
    filters: Vec<SubFilter>,
    z: Vec<f32>,
    zi: usize,
}

impl Interpolator {
    fn new(factor: usize, channels: usize) -> Self {
        let delay = TAPS.div_ceil(factor);
        let mut filters: Vec<SubFilter> = (0..factor)
            .map(|_| SubFilter {
                index: Vec::with_capacity(delay),
                coeff: Vec::with_capacity(delay),
            })
            .collect();
        for j in 0..TAPS {
            // Sinc with a Hann window; zero coefficients are dropped.
            let m = j as f64 - (TAPS as f64 - 1.0) / 2.0;
            let mut c = 1.0f64;
            if m.abs() > ALMOST_ZERO {
                c = (m * PI / factor as f64).sin() / (m * PI / factor as f64);
            }
            c *= 0.5 * (1.0 - (2.0 * PI * j as f64 / (TAPS as f64 - 1.0)).cos());
            if c.abs() > ALMOST_ZERO {
                let f = j % factor;
                filters[f].coeff.push(c);
                filters[f].index.push(j / factor);
            }
        }
        Self {
            delay,
            channels,
            filters,
            z: vec![0.0f32; channels * delay],
            zi: 0,
        }
    }

    /// Runs the interpolator over `input` (interleaved, `frames × channels`)
    /// and returns the maximum absolute interpolated value.
    fn max_abs(&mut self, input: &[f32]) -> f64 {
        let mut peak = 0.0f64;
        // Channels are always 1..=2 (constructor), so this cannot divide by 0.
        let frames = input.len() / self.channels;
        let mut source = 0usize;
        for _ in 0..frames {
            for c in 0..self.channels {
                let base = c * self.delay;
                self.z[base + self.zi] = input[source];
                source += 1;
                for filter in &self.filters {
                    let mut acc = 0.0f64;
                    for t in 0..filter.coeff.len() {
                        let mut i = self.zi as isize - filter.index[t] as isize;
                        if i < 0 {
                            i += self.delay as isize;
                        }
                        acc += (self.z[base + i as usize] as f64) * filter.coeff[t];
                    }
                    let value = (acc as f32) as f64;
                    let abs = if value < 0.0 { -value } else { value };
                    if abs > peak {
                        peak = abs;
                    }
                }
            }
            self.zi += 1;
            if self.zi == self.delay {
                self.zi = 0;
            }
        }
        peak
    }
}

/// Derives the playback gain for a target integrated loudness.
///
/// ```text
/// gain_db = target_lufs - measured_lufs
/// ```
///
/// Pure measurement arithmetic: no target is hard-coded (playback policy is
/// a later phase) and gain is never applied to samples here.
pub fn gain_db(measured_lufs: f64, target_lufs: f64) -> f64 {
    target_lufs - measured_lufs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, channels: usize, rate: u32, freq: f64, amp: f64) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let v = (amp * (2.0 * PI * freq * i as f64 / rate as f64).sin()) as f32;
            for _ in 0..channels {
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn rejects_invalid_configuration() {
        assert!(LoudnessMeter::new(0, 44100).is_err());
        assert!(LoudnessMeter::new(3, 44100).is_err());
        assert!(LoudnessMeter::new(2, 0).is_err());
        assert!(LoudnessMeter::new(2, 15).is_err());
        assert!(LoudnessMeter::new(2, 2_822_401).is_err());
        assert!(LoudnessMeter::new(1, 44100).is_ok());
        let mut m = LoudnessMeter::new(2, 44100).unwrap();
        assert!(m.process(&[0.0, 0.0, 0.0]).is_err());
    }

    #[test]
    fn silence_floors_both() {
        let mut m = LoudnessMeter::new(2, 44100).unwrap();
        m.process(&vec![0.0f32; 44100 * 2]).unwrap();
        let r = m.result();
        assert_eq!(r.lufs, FLOOR_DB);
        assert_eq!(r.true_peak_db_tp, FLOOR_DB);
    }

    #[test]
    fn full_scale_sine_is_near_zero() {
        let pcm = sine(44100 * 3, 2, 44100, 1000.0, 1.0);
        let mut m = LoudnessMeter::new(2, 44100).unwrap();
        m.process(&pcm).unwrap();
        let r = m.result();
        assert!(r.lufs.abs() < 0.5, "lufs {}", r.lufs);
        assert!(r.true_peak_db_tp.abs() < 0.5, "tp {}", r.true_peak_db_tp);
    }

    #[test]
    fn short_input_keeps_true_peak() {
        let pcm = sine(1000, 1, 44100, 1000.0, 1.0);
        let mut m = LoudnessMeter::new(1, 44100).unwrap();
        m.process(&pcm).unwrap();
        let r = m.result();
        assert_eq!(r.lufs, FLOOR_DB); // no complete gating block
        assert!(r.true_peak_db_tp > -1.0, "tp {}", r.true_peak_db_tp);
    }

    #[test]
    fn result_is_repeatable() {
        let pcm = sine(44100, 2, 44100, 1000.0, 0.5);
        let mut m = LoudnessMeter::new(2, 44100).unwrap();
        m.process(&pcm).unwrap();
        assert_eq!(m.result(), m.result());
    }

    #[test]
    fn chunking_is_bit_exact() {
        let pcm = sine(60000, 2, 44100, 997.0, 0.8);
        let reference = {
            let mut m = LoudnessMeter::new(2, 44100).unwrap();
            m.process(&pcm).unwrap();
            m.result()
        };
        for chunk in [4096usize, 1024, 137, 1] {
            let mut m = LoudnessMeter::new(2, 44100).unwrap();
            for part in pcm.chunks(chunk * 2) {
                m.process(part).unwrap();
            }
            let r = m.result();
            assert_eq!(r.lufs.to_bits(), reference.lufs.to_bits(), "chunk {chunk}");
            assert_eq!(
                r.true_peak_db_tp.to_bits(),
                reference.true_peak_db_tp.to_bits(),
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn album_sequential_equals_concatenated() {
        let a = sine(44100 * 3, 2, 44100, 1000.0, 1.0);
        let b = sine(44100 * 3, 2, 44100, 1000.0, 0.25);

        let mut album = LoudnessMeter::new(2, 44100).unwrap();
        album.process(&a).unwrap();
        album.process(&b).unwrap();
        let album_result = album.result();

        let mut concat = LoudnessMeter::new(2, 44100).unwrap();
        let mut joined = a.clone();
        joined.extend_from_slice(&b);
        concat.process(&joined).unwrap();
        assert!((album_result.lufs - concat.result().lufs).abs() < 0.01);

        let mut ra = LoudnessMeter::new(2, 44100).unwrap();
        ra.process(&a).unwrap();
        let mut rb = LoudnessMeter::new(2, 44100).unwrap();
        rb.process(&b).unwrap();
        let mean = (ra.result().lufs + rb.result().lufs) / 2.0;
        assert!(
            (album_result.lufs - mean).abs() > 0.5,
            "must not be the mean"
        );
    }

    #[test]
    fn hostile_floats_do_not_panic() {
        let mut m = LoudnessMeter::new(2, 44100).unwrap();
        let mut pcm = vec![0.0f32; 44100 * 2];
        for (i, sample) in pcm.iter_mut().enumerate() {
            *sample = match i % 4 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                _ => 1.5,
            };
        }
        m.process(&pcm).unwrap();
        let r = m.result();
        assert!(r.lufs.is_finite());
        assert!(r.true_peak_db_tp.is_finite());
    }

    #[test]
    fn gain_is_derived() {
        assert_eq!(gain_db(-10.0, -16.0), -6.0);
    }
}
