//! Encoding stage: supported source audio → Musepack SV8.
//!
//! Sources are decoded with the core's native decoders (`musicpack-core`
//! `audio`), converted to interleaved 16-bit PCM and encoded by the isolated
//! [`musicpack_musepack_encoder`] crate, which reproduces the reference
//! `mpcenc` stream **byte-for-byte** for its frozen compatibility corpus.
//! No external process, no FFmpeg, no second codec.
//!
//! # Settings contract
//!
//! The reference `encode-draft` invokes `mpcenc --quality <q> --overwrite
//! --silent <wav> <mpc>`; there are no other settings. The Author's default
//! quality is `6.0` and the UI offers `5.0/6.0/7.0/8.0`. Rust preserves the
//! semantic contract: quality is a numeric value, sample rate and channels
//! come from the decoded source, output is `.mpc`.
//!
//! # Known compatibility gap (documented, not silent)
//!
//! The Rust encoder's frozen psychoacoustic tables cover the complete
//! **integer** quality matrix of the reference `mpcenc`: qualities `0..=10`
//! at `44100`, `48000`, `37800` and `32000` Hz (44 configurations; see
//! `musicpack-musepack-encoder/tests/data/encoder/matrix_manifest.txt`).
//! Fractional qualities (e.g. `5.5`, which the C encoder interpolates) and
//! out-of-range qualities (which the C encoder clips) are deliberately
//! deferred to a separate parity slice and are rejected with a typed
//! [`AuthorError::Unsupported`] rather than being silently mapped to a
//! different profile.
//!
//! Sources deeper than 16 bits are reduced to 16 bits for the encoder
//! (the top 16 bits of the sample, which is exact for 16-bit sources). The
//! reference passes the source bit depth to `mpcenc`; this is a documented
//! fidelity gap of the current Rust encoder API, not a format change.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use musicpack_core::audio;
use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};

use crate::error::{AuthorError, Result};

/// Sample rates Musepack accepts (the reference's `encode_supported_rate`).
pub const SUPPORTED_SAMPLE_RATES: [u32; 4] = [32000, 37800, 44100, 48000];

/// The Author's default Musepack quality (`EncodePanel.svelte`).
pub const DEFAULT_QUALITY: f32 = 6.0;

/// Facts about an encoded track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeInfo {
    /// Sample rate of the source (and the stream).
    pub sample_rate: u32,
    /// Channel count of the source (and the stream).
    pub channels: u8,
    /// Total sample frames encoded.
    pub frames: u64,
}

/// `true` when the source extension can be encoded by this stage.
pub fn is_encodable_extension(path: &str) -> bool {
    matches!(extension(path).as_str(), "flac" | "wav")
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Decodes `source` and writes a Musepack SV8 stream to `dest`.
pub fn encode_to(source: &Path, dest: &Path, quality: f32) -> Result<EncodeInfo> {
    let file = File::open(source).map_err(|e| AuthorError::Encode {
        disc: 0,
        track: 0,
        detail: format!("cannot read '{}': {e}", source.display()),
    })?;
    let mut decoder = audio::open(Box::new(file)).map_err(|e| AuthorError::Encode {
        disc: 0,
        track: 0,
        detail: format!("cannot decode '{}': {e}", source.display()),
    })?;
    let info = decoder.info().clone();

    if info.is_float {
        return Err(AuthorError::Unsupported {
            detail: "floating-point WAV sources are not supported".into(),
        });
    }
    if info.channels == 0 || info.channels > 2 {
        return Err(AuthorError::Unsupported {
            detail: format!(
                "{} channels are not supported by Musepack (1 or 2 required)",
                info.channels
            ),
        });
    }
    if !SUPPORTED_SAMPLE_RATES.contains(&info.sample_rate) {
        return Err(AuthorError::Unsupported {
            detail: format!(
                "sample rate {} Hz is not supported by Musepack (supported: 32/37.8/44.1/48 kHz)",
                info.sample_rate
            ),
        });
    }

    let mut encoder = MusepackEncoder::new(EncoderConfig::new(
        quality,
        info.sample_rate,
        info.channels as u32,
    ))
    .map_err(|e| match e {
        musicpack_musepack_encoder::error::EncoderError::UnsupportedPsyConfig {
            qual,
            sample_rate,
        } => AuthorError::Unsupported {
            detail: format!(
                "the Rust encoder has no frozen profile for quality {qual} at {sample_rate} Hz \
                 (supported: integer qualities 0..=10 at 32000/37800/44100/48000 Hz; \
                 fractional qualities are deferred)"
            ),
        },
        other => AuthorError::Encode {
            disc: 0,
            track: 0,
            detail: format!("cannot initialise the encoder: {other}"),
        },
    })?;

    let channels = info.channels as usize;
    let mut pcm: Vec<i16> = Vec::new();
    let mut block = vec![0i32; 8192 * channels];
    let mut frames: u64 = 0;
    loop {
        let read = decoder
            .read_s32(&mut block)
            .map_err(|e| AuthorError::Encode {
                disc: 0,
                track: 0,
                detail: format!("cannot decode '{}': {e}", source.display()),
            })?;
        if read == 0 {
            break;
        }
        let samples = read * channels;
        pcm.extend(block[..samples].iter().map(|s| (s >> 16) as i16));
        frames += read as u64;
    }

    let bytes = encoder.encode(&pcm).map_err(|e| AuthorError::Encode {
        disc: 0,
        track: 0,
        detail: format!("cannot encode '{}': {e}", source.display()),
    })?;

    let mut out = File::create(dest).map_err(|e| AuthorError::Encode {
        disc: 0,
        track: 0,
        detail: format!("cannot write '{}': {e}", dest.display()),
    })?;
    out.write_all(&bytes).map_err(|e| AuthorError::Encode {
        disc: 0,
        track: 0,
        detail: format!("cannot write '{}': {e}", dest.display()),
    })?;

    Ok(EncodeInfo {
        sample_rate: info.sample_rate,
        channels: info.channels,
        frames,
    })
}
