//! Fuzz target: the Phase 9 analysis feed paths (`WaveformAccumulator` and
//! `LoudnessMeter`).
//!
//! Properties under test:
//!
//! - no panic on arbitrary `f32` bit patterns (NaN, ±Inf, huge finite);
//! - termination on arbitrary chunk boundaries;
//! - bounded memory: the waveform payload is capped at `MAX_POINTS`; the
//!   loudness block history grows only with the (fuzzer-bounded) input;
//! - no arithmetic overflow feeding indexing or allocation.
#![no_main]

use libfuzzer_sys::fuzz_target;
use musicpack_core::audio::{LoudnessMeter, WaveformAccumulator};

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let channels = (data[0] % 8) + 1; // 1..=8 (waveform)
    let rate = match data[1] % 4 {
        0 => 44100,
        1 => 48000,
        2 => 96000,
        _ => 192000,
    };
    let chunk_frames = (data[2] as usize % 7) + 1;

    let payload = &data[3..];
    let mut samples = Vec::with_capacity(payload.len() / 4);
    for bytes in payload.chunks_exact(4) {
        samples.push(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    }

    // Waveform: real channel count.
    if let Ok(mut acc) = WaveformAccumulator::new(rate, channels) {
        let stride = channels as usize;
        let mut i = 0;
        while i + stride <= samples.len() {
            let frames = ((samples.len() - i) / stride).min(chunk_frames);
            if frames == 0 {
                break;
            }
            let _ = acc.feed(&samples[i..i + frames * stride]);
            i += frames * stride;
        }
        let _ = acc.finish();
    }

    // Loudness: clamped to the supported 1..=2 channels.
    let loudness_channels = if channels > 2 { 2 } else { channels };
    if let Ok(mut meter) = LoudnessMeter::new(loudness_channels, rate) {
        let stride = loudness_channels as usize;
        let mut i = 0;
        while i + stride <= samples.len() {
            let frames = ((samples.len() - i) / stride).min(257);
            if frames == 0 {
                break;
            }
            let _ = meter.process(&samples[i..i + frames * stride]);
            i += frames * stride;
        }
        let _ = meter.result();
    }
});
