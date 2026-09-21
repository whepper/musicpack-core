//! Waveform stage: packaged audio → canonical v1 envelope payload.
//!
//! Decoding uses the core's native decoders; accumulation uses the core's
//! [`WaveformAccumulator`] (the same kernel the package contract is defined
//! by, `specs/musicpack-waveform-v1.md`). There is no second waveform
//! representation: the payload written here is exactly what
//! `build_directory` materializes and the verifier checks
//! (`analysis/waveform/<DD>-<TT>.wfm`, `peak-rms-u8`, 100 ms, −60 dB).
//!
//! The stage is deterministic: bucket boundaries derive from the cumulative
//! frame counter and results are independent of decoder chunk size.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use musicpack_core::audio::{self, WaveformAccumulator};

use crate::error::{AuthorError, Result};

/// Decodes `source` and writes the canonical waveform payload to `dest`.
///
/// Returns the payload's point (bucket) count — `payload_len / 2`.
pub fn generate_to(source: &Path, dest: &Path) -> Result<u64> {
    let file = File::open(source).map_err(|e| AuthorError::Io {
        detail: format!("cannot read '{}': {e}", source.display()),
    })?;
    let mut decoder = audio::open(Box::new(file)).map_err(|e| AuthorError::Io {
        detail: format!("cannot decode '{}': {e}", source.display()),
    })?;
    let info = decoder.info().clone();

    let mut accumulator =
        WaveformAccumulator::new(info.sample_rate, info.channels).map_err(|e| {
            AuthorError::Unsupported {
                detail: format!(
                    "cannot accumulate a waveform for '{}': {e}",
                    source.display()
                ),
            }
        })?;

    let channels = info.channels as usize;
    let mut block = vec![0f32; 8192 * channels];
    loop {
        let read = decoder.read_f32(&mut block).map_err(|e| AuthorError::Io {
            detail: format!("cannot decode '{}': {e}", source.display()),
        })?;
        if read == 0 {
            break;
        }
        let samples = read * channels;
        accumulator
            .feed(&block[..samples])
            .map_err(|e| AuthorError::Io {
                detail: format!(
                    "waveform accumulation failed for '{}': {e}",
                    source.display()
                ),
            })?;
    }

    let payload = accumulator.finish();
    let mut out = File::create(dest).map_err(|e| AuthorError::Io {
        detail: format!("cannot write '{}': {e}", dest.display()),
    })?;
    out.write_all(&payload).map_err(|e| AuthorError::Io {
        detail: format!("cannot write '{}': {e}", dest.display()),
    })?;
    Ok((payload.len() / 2) as u64)
}

/// Generates the standard per-track waveform payload path
/// (`analysis/waveform/<DD>-<TT>.wfm` package path and its work-relative
/// counterpart).
pub fn payload_names(disc: i32, track: i32) -> (String, String) {
    (
        format!("analysis/waveform/{disc:02}-{track:02}.wfm"),
        format!("waveform/{disc:02}-{track:02}.wfm"),
    )
}
