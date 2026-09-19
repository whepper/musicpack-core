//! Streaming `.mpack` v1 waveform accumulator.
//!
//! Port of the reference streaming accumulator in
//! `core/libmusicpack/src/waveform.c` (BSD-3-Clause). It consumes
//! interleaved `f32` PCM from [`crate::audio::AudioDecoder::read_f32`] and
//! emits the `peak-rms-u8` payload described by
//! `specs/musicpack-waveform-v1.md` §5/§6:
//!
//! - 100 ms buckets on **cumulative sample-time** boundaries
//!   (`floor(frames · 1000 / (rate · 100))`) — never from floating-point
//!   elapsed seconds, so boundaries do not drift over long programs;
//! - per bucket, `peak = max |sample|` over all channels and frames, and
//!   `rms = sqrt(Σs² / N)` with a single denominator `N = frames · channels`;
//! - silent buckets (`peak == 0 && Σs² == 0`) emit `(0, 0)` without running
//!   the logarithmic quantizer;
//! - quantization reuses [`crate::format::waveform::quantize_amplitude`]
//!   unchanged.
//!
//! The accumulator is streaming and constant-memory apart from its bounded
//! output: at most [`MAX_POINTS`] buckets (1 728 000 payload bytes).
//!
//! # Numerical deviation (documented)
//!
//! The reference accumulates `Σs²` in C `long double`, whose width is
//! platform-dependent (80-bit on x86, 64-bit elsewhere). Rust has no
//! portable 80-bit float, so this port uses `f64`. This is a deliberate
//! numerical-portability deviation; the payloads are still required to be
//! **byte-identical** to the reference corpus (see `docs/architecture.md`).
//! If a platform-specific divergence ever appears it must be investigated,
//! not papered over with a tolerance.
//!
//! # Hostile input
//!
//! Samples are passed through `sanitize_sample` (NaN→0,
//! ±Inf→±1, finite out-of-range unchanged) before accumulation. The output
//! is bounded by [`MAX_POINTS`]; no allocation is influenced by a declared
//! WAV length or an untrusted frame count.

use crate::format::waveform::{MAX_POINTS, quantize_amplitude};

use super::{invalid, sanitize_sample};
use crate::Result;

/// A streaming waveform-envelope accumulator.
///
/// See the [module documentation](self) for the exact contract.
pub struct WaveformAccumulator {
    sample_rate: u32,
    channels: u8,
    /// Cumulative frames fed so far (the sample clock).
    total_frames: u64,
    /// Cumulative bucket index already emitted.
    bucket_index: u64,
    /// Current bucket peak (max |sample| across channels and frames).
    peak_abs: f64,
    /// Current bucket sum of squares (channel samples).
    sum_sq: f64,
    /// Current bucket channel-sample count (`frames × channels`).
    bucket_samples: u64,
    /// Emitted `peak, rms, peak, rms, …` payload.
    buckets: Vec<u8>,
}

impl WaveformAccumulator {
    /// Creates an accumulator for a stream.
    ///
    /// Rejects `sample_rate == 0` and `channels` outside `1..=8` with
    /// [`crate::Error::Invalid`] (matching `musicpack_waveform_acc_new`). No upper
    /// bound is placed on the sample rate, exactly like the reference.
    pub fn new(sample_rate: u32, channels: u8) -> Result<Self> {
        if sample_rate == 0 || channels == 0 || channels > super::MAX_CHANNELS {
            return Err(invalid(
                "waveform accumulator requires a non-zero sample rate and 1..=8 channels",
            ));
        }
        Ok(Self {
            sample_rate,
            channels,
            total_frames: 0,
            bucket_index: 0,
            peak_abs: 0.0,
            sum_sq: 0.0,
            bucket_samples: 0,
            buckets: Vec::new(),
        })
    }

    /// Frames emitted so far (payload length ÷ 2).
    #[inline]
    fn bucket_count(&self) -> usize {
        self.buckets.len() / 2
    }

    /// Feeds interleaved `f32` PCM.
    ///
    /// `interleaved.len()` must be a multiple of the channel count, else
    /// [`crate::Error::Invalid`] is returned and no state changes. Empty input is
    /// valid and a no-op. The frame count is derived from the slice length.
    ///
    /// Once [`MAX_POINTS`] buckets exist, further frames are rejected
    /// (matching the reference) and the accumulated state is left intact.
    pub fn feed(&mut self, interleaved: &[f32]) -> Result<()> {
        let channels = self.channels as usize;
        if interleaved.len() % channels != 0 {
            return Err(invalid(
                "waveform input length is not a multiple of the channel count",
            ));
        }
        if self.bucket_count() >= MAX_POINTS as usize {
            return Err(invalid(
                "waveform bucket count exceeds the 864000-point limit",
            ));
        }
        let frames = interleaved.len() / channels;
        let rate = self.sample_rate as u128 * 100; // rate·100 ms
        for frame in 0..frames {
            let base = frame * channels;
            for channel in 0..channels {
                let sample = sanitize_sample(interleaved[base + channel]) as f64;
                let abs = if sample < 0.0 { -sample } else { sample };
                if abs > self.peak_abs {
                    self.peak_abs = abs;
                }
                self.sum_sq += sample * sample;
                self.bucket_samples += 1;
            }
            self.total_frames += 1;
            // Overflow-safe cumulative bucket index (the C reference wraps
            // `uint64 * 1000`; this port never does).
            let index = (self.total_frames as u128 * 1000 / rate) as u64;
            if index > self.bucket_index {
                self.flush_bucket()?;
                self.bucket_index = index;
            }
        }
        Ok(())
    }

    /// Finalizes accumulation, flushing the final partial bucket.
    ///
    /// Consumes the accumulator (the reference resets its bucket storage on
    /// finish; ownership makes double-finish unrepresentable). An empty
    /// stream yields an empty payload.
    pub fn finish(mut self) -> Vec<u8> {
        // A partial bucket can only remain below the cap: `feed` refuses
        // further frames once `MAX_POINTS` buckets exist, so `flush_bucket`
        // cannot fail here.
        if self.bucket_samples > 0 && self.bucket_count() < MAX_POINTS as usize {
            let _ = self.flush_bucket();
        }
        self.buckets
    }

    /// Emits the current bucket and resets the per-bucket state.
    fn flush_bucket(&mut self) -> Result<()> {
        if self.bucket_samples == 0 {
            return Ok(());
        }
        if self.bucket_count() >= MAX_POINTS as usize {
            return Err(invalid(
                "waveform bucket count exceeds the 864000-point limit",
            ));
        }
        let (peak, rms) = if self.sum_sq == 0.0 && self.peak_abs == 0.0 {
            // Silent fast path: never touch the logarithmic quantizer.
            (0, 0)
        } else {
            let mean = self.sum_sq / self.bucket_samples as f64;
            let rms_linear = if mean > 0.0 { mean.sqrt() } else { 0.0 };
            (
                quantize_amplitude(self.peak_abs),
                quantize_amplitude(rms_linear),
            )
        };
        self.buckets.push(peak);
        self.buckets.push(rms);
        self.sum_sq = 0.0;
        self.peak_abs = 0.0;
        self.bucket_samples = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(frames: usize, channels: usize, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            for c in 0..channels {
                out.push(f(i, c));
            }
        }
        out
    }

    #[test]
    fn rejects_bad_configuration_and_lengths() {
        assert!(WaveformAccumulator::new(0, 2).is_err());
        assert!(WaveformAccumulator::new(44100, 0).is_err());
        assert!(WaveformAccumulator::new(44100, 9).is_err());
        let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
        assert!(acc.feed(&[0.0, 0.0, 0.0]).is_err()); // 3 % 2 != 0
        assert!(acc.feed(&[]).is_ok());
    }

    #[test]
    fn silence_emits_zero_buckets() {
        let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
        acc.feed(&vec![0.0f32; 88200 * 2]).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len(), 20 * 2);
        assert!(payload.iter().all(|&b| b == 0));
    }

    #[test]
    fn empty_stream_has_no_buckets() {
        let acc = WaveformAccumulator::new(44100, 2).unwrap();
        assert!(acc.finish().is_empty());
    }

    #[test]
    fn full_scale_sine_matches_reference_ranges() {
        let frames = 44100;
        let rate = 44100.0f64;
        let pcm = synth(frames, 2, |i, _| {
            (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin() as f32
        });
        let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
        acc.feed(&pcm).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 10);
        for pair in payload.chunks_exact(2) {
            assert_eq!(pair[0], 255);
            assert!((240..=244).contains(&pair[1]), "rms {}", pair[1]);
        }
    }

    #[test]
    fn one_channel_of_stereo_still_collects() {
        let pcm = synth(44100, 2, |_, c| if c == 0 { 1.0 } else { 0.0 });
        let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
        acc.feed(&pcm).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 10);
        for pair in payload.chunks_exact(2) {
            assert_eq!(pair[0], 255);
            assert!((240..=244).contains(&pair[1]));
        }
    }

    #[test]
    fn cumulative_boundary_at_11025() {
        // 100 ms is 1102.5 frames, so the first boundary is frame 1103.
        let mut pcm = vec![0.0f32; 1103];
        pcm[1102] = 1.0;
        let mut acc = WaveformAccumulator::new(11025, 1).unwrap();
        acc.feed(&pcm).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 1);
        assert_eq!(payload[0], 255);
    }

    #[test]
    fn partial_final_bucket_is_flushed() {
        let frames = (4.05 * 44100.0) as usize; // 178605
        let pcm = vec![0.3f32; frames * 2];
        let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
        acc.feed(&pcm).unwrap();
        assert_eq!(acc.finish().len() / 2, 41);
    }

    #[test]
    fn chunking_is_bit_exact() {
        let frames = 20_000usize;
        let pcm = synth(frames, 2, |i, c| {
            ((i as f32) * 0.013 + c as f32).sin() * 0.7
        });
        let reference = {
            let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
            acc.feed(&pcm).unwrap();
            acc.finish()
        };
        for chunk in [4096usize, 1024, 137, 1] {
            let mut acc = WaveformAccumulator::new(44100, 2).unwrap();
            for part in pcm.chunks(chunk * 2) {
                acc.feed(part).unwrap();
            }
            assert_eq!(acc.finish(), reference, "chunk size {chunk}");
        }
    }

    #[test]
    fn bucket_cap_is_enforced_with_intact_state() {
        // 10 Hz mono: one frame advances the cumulative bucket index by one.
        let n = MAX_POINTS as usize;
        let pcm = vec![0.5f32; n];
        let mut acc = WaveformAccumulator::new(10, 1).unwrap();
        acc.feed(&pcm).unwrap();
        assert_eq!(acc.finish().len() / 2, n);

        let mut acc = WaveformAccumulator::new(10, 1).unwrap();
        acc.feed(&pcm).unwrap();
        assert!(acc.feed(&[0.5]).is_err());
        assert_eq!(acc.finish().len() / 2, n);
    }

    #[test]
    fn dc_buckets_match_the_direct_quantizer() {
        // A constant 0.5 mono signal: every bucket's peak and RMS equal the
        // quantized 0.5 amplitude (the reference's `mono` test).
        let pcm = vec![0.5f32; 24000];
        let mut acc = WaveformAccumulator::new(48000, 1).unwrap();
        acc.feed(&pcm).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 5);
        let expected = quantize_amplitude(0.5);
        for pair in payload.chunks_exact(2) {
            assert_eq!(pair, [expected, expected]);
        }
    }

    #[test]
    fn eight_channels_accumulate() {
        let mut acc = WaveformAccumulator::new(48000, 8).unwrap();
        let pcm = synth(48000, 8, |_, c| if c == 3 { 1.0 } else { 0.0 });
        acc.feed(&pcm).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 10);
        for pair in payload.chunks_exact(2) {
            assert_eq!(pair[0], 255);
        }
    }

    #[test]
    fn large_finite_values_clamp_at_quantization() {
        let mut acc = WaveformAccumulator::new(10, 1).unwrap();
        acc.feed(&[100.0, -100.0, f32::MAX, f32::MIN]).unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 4);
        for pair in payload.chunks_exact(2) {
            assert_eq!(pair[0], 255);
        }
    }

    #[test]
    fn bucket_count_is_sample_rate_invariant() {
        // The same duration at 44.1 and 48 kHz yields ~10 buckets per second.
        let a44 = waveform_payload(&vec![0.5f32; 44100], 44100, 1);
        let a48 = waveform_payload(&vec![0.5f32; 44100], 48000, 1);
        let c44 = a44.len() / 2;
        let c48 = a48.len() / 2;
        assert!((9..=10).contains(&c44));
        assert!((9..=11).contains(&c48));
    }

    fn waveform_payload(pcm: &[f32], rate: u32, channels: u8) -> Vec<u8> {
        let mut acc = WaveformAccumulator::new(rate, channels).unwrap();
        acc.feed(pcm).unwrap();
        acc.finish()
    }

    #[test]
    fn hostile_floats_are_sanitized() {
        let mut acc = WaveformAccumulator::new(10, 1).unwrap();
        acc.feed(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0])
            .unwrap();
        let payload = acc.finish();
        assert_eq!(payload.len() / 2, 4);
        // Frame 0 (NaN -> 0) is silent; frames 1..4 carry ±1.
        assert_eq!(&payload[0..2], &[0, 0]);
        assert_eq!(payload[2], 255);
    }
}
