use std::fs::File;
use std::io::Read;
use std::path::Path;

use musicpack_core::audio;
use rubato::{FftFixedIn, Resampler};
use sha2::{Digest, Sha256};

use crate::Result;

pub const ANALYSIS_SAMPLE_RATE: u32 = 16_000;
const DECODE_CHUNK_FRAMES: usize = 4096;
const MAX_SAMPLES: usize = 250_000_000;

#[derive(Debug, Clone)]
pub struct DecodedTrack {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub source_rate: u32,
    pub source_channels: u8,
    pub source_frames: u64,
    pub duration_seconds: f64,
    pub source_sha256: String,
}

pub fn decode_and_prepare(path: &Path) -> Result<DecodedTrack> {
    let source_sha256 = sha256_file(path)?;
    let file = File::open(path)?;
    let mut decoder = audio::open(Box::new(file))?;
    let info = decoder.info().clone();
    let channels = usize::from(info.channels);
    if channels == 0 {
        return Err("decoder returned zero channels".into());
    }

    let mut interleaved = Vec::new();
    let mut buffer = vec![0.0f32; DECODE_CHUNK_FRAMES * channels];
    loop {
        let frames = decoder.read_f32(&mut buffer)?;
        if frames == 0 {
            break;
        }
        let new_len = interleaved
            .len()
            .checked_add(frames * channels)
            .ok_or("decoded sample length overflow")?;
        if new_len > MAX_SAMPLES {
            return Err("decoded sample limit exceeded".into());
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

    Ok(DecodedTrack {
        samples,
        sample_rate: ANALYSIS_SAMPLE_RATE,
        source_rate: info.sample_rate,
        source_channels: info.channels,
        source_frames,
        duration_seconds,
        source_sha256,
    })
}

pub fn sha256_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sha256_f32(values: &[f32]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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

fn resample(input: &[f32], source_rate: u32, target_rate: u32) -> Result<Vec<f32>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if source_rate == target_rate {
        return Ok(input.to_vec());
    }

    // The experiment uses a fixed-ratio FFT resampler. It is intentionally
    // local to this package; the playback resampler has different semantics.
    let mut resampler =
        FftFixedIn::<f32>::new(source_rate as usize, target_rate as usize, 1024, 2, 1)?;
    let delay = resampler.output_delay();
    let mut output = vec![vec![0.0f32; resampler.output_frames_max()]];
    let mut result = Vec::with_capacity(
        (input.len() as f64 * f64::from(target_rate) / f64::from(source_rate)).ceil() as usize
            + delay,
    );
    let mut offset = 0usize;

    while input.len() - offset >= resampler.input_frames_next() {
        let end = offset + resampler.input_frames_next();
        let (_consumed, produced) =
            resampler.process_into_buffer(&[&input[offset..end]], &mut output, None)?;
        result.extend_from_slice(&output[0][..produced]);
        offset = end;
    }

    if offset < input.len() {
        let (_consumed, produced) =
            resampler.process_partial_into_buffer(Some(&[&input[offset..]]), &mut output, None)?;
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
