use std::f64::consts::PI;

use microfft::real::rfft_512;

use crate::Result;

pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_SIZE: usize = 512;
pub const FRAME_HOP: usize = 256;
pub const MEL_BANDS: usize = 96;
pub const PATCH_SIZE: usize = 128;
pub const DEFAULT_PATCH_HOP: usize = 61;

#[derive(Debug, Clone)]
pub struct FrontendOutput {
    pub patches: Vec<Vec<f32>>,
    pub frame_count: usize,
}

#[derive(Debug, Clone)]
pub struct MelFrontend {
    filters: Vec<Vec<f32>>,
    patch_hop: usize,
}

impl MelFrontend {
    pub fn new(patch_hop: usize) -> Result<Self> {
        if patch_hop == 0 {
            return Err("patch hop must be positive".into());
        }
        let filters = build_filters();
        Ok(Self { filters, patch_hop })
    }

    pub fn patch_hop(&self) -> usize {
        self.patch_hop
    }

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
            start += self.patch_hop;
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

        // Essentia's Windowing is configured with normalized=false and its
        // default zeroPhase=true. For an even frame this rotates the two
        // windowed halves before the FFT.
        let mut windowed = [0.0f32; FRAME_SIZE];
        for index in 0..FRAME_SIZE / 2 {
            let first = FRAME_SIZE / 2 + index;
            windowed[index] = frame[first] * hann(first);
            windowed[FRAME_SIZE / 2 + index] = frame[index] * hann(index);
        }

        let spectrum = rfft_512(&mut windowed);
        let mut power = [0.0f32; FRAME_SIZE / 2 + 1];
        power[0] = spectrum[0].re * spectrum[0].re;
        for index in 1..FRAME_SIZE / 2 {
            power[index] =
                spectrum[index].re * spectrum[index].re + spectrum[index].im * spectrum[index].im;
        }
        // microfft packs the Nyquist coefficient into the imaginary part of
        // the DC bin, as documented by microfft's real FFT API.
        power[FRAME_SIZE / 2] = spectrum[0].im * spectrum[0].im;

        let mut bands = vec![0.0f32; MEL_BANDS];
        for (band, filter) in self.filters.iter().enumerate() {
            let mut value = 0.0f64;
            for (bin, coefficient) in filter.iter().enumerate() {
                value += f64::from(power[bin]) * f64::from(*coefficient);
            }
            // TensorflowInputMusiCNN uses scale=10000, shift=1, then log10.
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
    let high_hz = f64::from(SAMPLE_RATE) / 2.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filterbank_has_expected_shape() {
        let filters = build_filters();
        assert_eq!(filters.len(), MEL_BANDS);
        assert!(
            filters
                .iter()
                .all(|filter| filter.len() == FRAME_SIZE / 2 + 1)
        );
        assert!(
            filters
                .iter()
                .all(|filter| filter.iter().all(|value| value.is_finite()))
        );
    }

    #[test]
    fn short_audio_has_no_model_patch() {
        let frontend = MelFrontend::new(DEFAULT_PATCH_HOP).unwrap();
        let output = frontend.process(&vec![0.0; 16_000]);
        assert!(output.patches.is_empty());
        assert!(output.frame_count < PATCH_SIZE);
    }

    #[test]
    fn ten_seconds_produces_historical_patch_count() {
        let frontend = MelFrontend::new(DEFAULT_PATCH_HOP).unwrap();
        let output = frontend.process(&vec![0.0; 160_000]);
        assert_eq!(output.frame_count, 625);
        assert_eq!(output.patches.len(), 9);
        assert!(
            output
                .patches
                .iter()
                .all(|patch| patch.len() == PATCH_SIZE * MEL_BANDS)
        );
    }
}
