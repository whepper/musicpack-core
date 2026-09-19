//! Audio domain: PCM representations, the decode abstraction, the streaming
//! waveform accumulator, and BS.1770-5 loudness/true-peak metering.
//!
//! This module is a compatibility port of the reference's
//! `musicpack_audio_*` seam (`core/libmusicpack/include/musicpack/audio.h`,
//! `src/audio.c`), not a general media framework. The parts implemented in
//! phase 8 are the PCM contract, the metadata model, the decoder trait, a
//! native RIFF/WAVE reader, and a FLAC decoder adapter.
//!
//! # PCM contract
//!
//! Decoders write into **caller-owned** slices; the core allocates nothing
//! from an untrusted audio length.
//!
//! - [`AudioDecoder::read_f32`] produces interleaved IEEE-754 `f32` in the
//!   reference's `~[-1, 1]` range.
//! - [`AudioDecoder::read_s32`] produces interleaved **left-aligned** 32-bit
//!   integer PCM (16-bit content in bits 31..16, 24-bit in bits 31..8), the
//!   exact representation the encoder path consumes.
//! - Channels are 1..=8; the destination length must be a multiple of the
//!   channel count.
//! - Both methods return the number of **frames** (not samples) written.
//!   `Ok(0)` means clean end of stream; a short read is a success (this is
//!   what makes the reference's truncated-WAV semantics work), and only a
//!   source or malformed-stream failure is an [`Error`].
//!
//! ## Exact conversion
//!
//! Integer PCM is converted with the reference arithmetic, in `f32`, with
//! no clipping and no symmetric remapping:
//!
//! ```text
//! f32 = (sample as f32) * 2^-(bits-1)
//! ```
//!
//! so the integer minimum maps exactly to `-1.0` while the positive maximum
//! maps to `1 - 2^-(bits-1)` and `+1.0` is unreachable from integer PCM.
//! IEEE-float WAV is a raw bit copy ([`f32::from_le_bytes`]): NaN
//! payloads, infinities, denormals and out-of-range values pass through
//! untouched. `s32` reads from an IEEE-float source are structurally
//! unsupported ([`Error::Unsupported`]) and never silently convert.
//!
//! # Decoder seam
//!
//! [`open`] sniffs the magic bytes (`RIFF` → WAVE, `fLaC` → FLAC; the
//! reference selects by file extension, which is application policy and
//! stays out of the core) and returns a boxed [`AudioDecoder`]. The decoder
//! owns a `Box<dyn std::io::Read>` — exactly the shape
//! [`crate::storage::OpenedAsset::reader`] already yields for MPAK members —
//! so no new byte-stream trait is introduced and
//! [`crate::format::mpak::ByteSource`] remains the random-access container
//! seam. There are no `Send`/`Sync`/async/platform requirements.
//!
//! # Loudness (BS.1770-5)
//!
//! Implemented by [`LoudnessMeter`] / [`Loudness`] (phase 9), a hand-port of
//! the vendored `libebur128` 1.2.6 subset the reference uses. `specs/musicpack-v1.md`
//! §5 is normative:
//!
//! - Integrated loudness: K-weighted, 400 ms blocks, 100 ms hop, absolute
//!   gate −70 LUFS and relative gate −10 LU (identical to BS.1770-4 for
//!   mono/stereo). Only channels 1–2; no LRA or multichannel additions.
//! - True peak: 49-tap polyphase interpolation at 4× (< 96 kHz), 2×
//!   (96…<192 kHz) or none (≥ 192 kHz, where it degrades to the sample
//!   peak), floored at −70 dBTP.
//! - **Album loudness is measured as one concatenated program** — all
//!   tracks fed in manifest order through a single meter, gating blocks
//!   running continuously across boundaries; never an average of track
//!   values. Album true peak is the max across tracks. This is caller
//!   policy: no album type exists.
//! - Gain is derived, never stored: [`gain_db`] = `target − measured`.
//!
//! Loudness is compared to the reference within ±0.05 LU / ±0.05 dBTP (see
//! [`loudness`] and `docs/architecture.md` for the platform-variance
//! rationale).
//!
//! # Waveform accumulation
//!
//! Implemented by [`WaveformAccumulator`] (phase 9). `specs/musicpack-waveform-v1.md`
//! §2/§5/§6: streaming, constant-memory, one 100 ms bucket at a time; bucket
//! index derived from **cumulative** frames
//! (`floor(frames * 1000 / (rate * 100))`) to avoid drift; peak = max
//! |sample| across channels, RMS = single denominator over all channel
//! samples. Quantization reuses
//! [`crate::format::waveform::quantize_amplitude`] unchanged. Results are
//! **byte-exact** against the reference corpus and independent of decoder
//! chunk size.
//!
//! # Hostile-input normalization
//!
//! `sanitize_sample` maps NaN→0, ±Inf→±1 (finite out-of-range values are
//! left alone) at both analysis feed boundaries. This is a documented
//! robustness deviation from the reference, whose C code is undefined for
//! such values; the phase 8 decoder contract is unchanged.

use std::io::{Cursor, Read};

use crate::{Error, Result};

pub mod flac;
pub mod loudness;
pub mod musepack;
pub mod wav;
pub mod waveform_acc;

pub use loudness::{Loudness, LoudnessMeter, gain_db};
pub use waveform_acc::WaveformAccumulator;

/// Normalizes hostile IEEE-754 values at the analysis feed boundary.
///
/// The Phase 8 decoder deliberately preserves arbitrary float-WAV values
/// (NaN payloads, infinities, out-of-range finite samples). Analysis is a
/// different contract: it must be total and deterministic, so Phase 9 maps
/// the non-finite hostile values to well-defined saturations before they
/// reach an algorithm:
///
/// ```text
/// NaN  -> 0.0
/// +Inf -> +1.0
/// -Inf -> -1.0
/// ```
///
/// Finite values outside `[-1, 1]` are left untouched (the waveform
/// quantizer clamps them; the loudness meter carries them through exactly
/// as the reference does). This is a documented robustness deviation from
/// the reference, which has no defined behavior for NaN/Inf here (the C
/// waveform quantizer's float→`uint8_t` cast is undefined for NaN). The
/// Phase 8 decoder contract is not modified — sanitization happens only in
/// the analysis feed paths.
#[inline]
pub(crate) fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_nan() {
        0.0
    } else if sample == f32::INFINITY {
        1.0
    } else if sample == f32::NEG_INFINITY {
        -1.0
    } else {
        sample
    }
}

/// Maximum channel count the decoder seam accepts, matching the reference
/// (`MUSICPACK_AUDIO_MAX_CHANNELS`) and [`crate::format::waveform::MAX_CHANNELS`].
pub const MAX_CHANNELS: u8 = 8;

/// The container/codec a decoder reads.
///
/// The enum is `#[non_exhaustive]` so adding codecs is not a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Codec {
    /// Native RIFF/WAVE (integer PCM 8/16/24/32-bit or 32-bit IEEE float).
    Wav,
    /// FLAC (decoded through the `claxon` adapter).
    Flac,
    /// Musepack SV8 (`bits_per_sample == 0`, codec-native float).
    ///
    /// The container/metadata layer is implemented in [`musepack`]; audio
    /// synthesis is the deferred next step (see the module docs).
    Musepack,
}

/// Stream properties — the port of `musicpack_audio_format`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    /// Sample rate in Hz (never zero for an accepted stream).
    pub sample_rate: u32,
    /// Channel count, 1..=8.
    pub channels: u8,
    /// Bits per sample: 8/16/24/32 for WAV and FLAC. Zero is reserved for
    /// codec-native float formats (Musepack, a later phase).
    pub bits_per_sample: u8,
    /// Declared total sample-frames, or `None` when unknown.
    ///
    /// For WAV this is always `Some(data_len / block_align)` — the
    /// **declared** length from the `data` chunk header, deliberately not
    /// clamped to the physically available bytes (the reference trusts it).
    /// For FLAC it is the STREAMINFO sample count when present.
    pub total_frames: Option<u64>,
    /// The codec this stream was opened as.
    pub codec: Codec,
    /// `true` for IEEE-float WAV (never encodable, `s32` reads unsupported).
    pub is_float: bool,
}

/// A decoding session over an owned byte stream.
///
/// See the [module documentation](self) for the PCM contract. Implementors
/// own their source reader; callers drive decoding by repeatedly handing in
/// a buffer until `Ok(0)`.
pub trait AudioDecoder {
    /// Returns the stream's format facts.
    fn info(&self) -> &AudioInfo;

    /// Decodes up to `interleaved.len() / channels` frames of interleaved
    /// `f32` PCM.
    ///
    /// `interleaved.len()` must be a multiple of the channel count. Returns
    /// the number of frames written; `Ok(0)` is clean end of stream.
    fn read_f32(&mut self, interleaved: &mut [f32]) -> Result<usize>;

    /// Decodes up to `interleaved.len() / channels` frames of interleaved
    /// **left-aligned** 32-bit integer PCM.
    ///
    /// Returns [`Error::Unsupported`] (with no frames produced) for sources
    /// that have no integer representation — currently IEEE-float WAV.
    fn read_s32(&mut self, interleaved: &mut [i32]) -> Result<usize>;
}

/// Opens an audio source by sniffing its magic bytes.
///
/// `RIFF` selects the native WAVE reader, `fLaC` selects the FLAC adapter and
/// `MPCK` selects the Musepack SV8 reader. Any other content is
/// [`Error::Invalid`]. Extension-based dispatch (`.wav`/`.flac`/`.mpc`) is
/// the reference CLI's application policy and is deliberately not part of the
/// core.
///
/// Musepack currently exposes container metadata only: [`musepack`]'s decoder
/// returns [`Error::Unsupported`] from the PCM read methods until the
/// synthesis path lands.
pub fn open(mut source: Box<dyn Read>) -> Result<Box<dyn AudioDecoder>> {
    let mut magic = [0u8; 4];
    if read_full(&mut *source, &mut magic)? != 4 {
        return Err(invalid(
            "audio stream shorter than a 4-byte container magic",
        ));
    }
    // Replay the sniffed bytes to the decoder: `Cursor::chain` keeps the
    // stream positioned at byte zero without seeking or buffering it whole.
    let prefixed: Box<dyn Read> = Box::new(Cursor::new(magic).chain(source));
    match &magic {
        b"RIFF" => Ok(Box::new(wav::WavDecoder::new(prefixed)?)),
        b"fLaC" => Ok(Box::new(flac::FlacDecoder::new(prefixed)?)),
        b"MPCK" => Ok(Box::new(musepack::MusepackDecoder::new(prefixed)?)),
        _ => Err(invalid(
            "unrecognized audio container (expected RIFF/WAVE, FLAC or Musepack)",
        )),
    }
}

/// Reads until `buf` is full or the source reaches EOF.
///
/// Mirrors C `fread`'s "fill the buffer unless EOF/error" behaviour, which
/// the reference's chunk scanner relies on. Returns the number of bytes
/// read; a value below `buf.len()` means EOF. An I/O error is reported as
/// [`Error::Io`] (the reference folds it into its INVALID path; the core
/// keeps the more precise category).
pub(crate) fn read_full(reader: &mut dyn Read, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Error::Io {
                    detail: e.to_string(),
                });
            }
        }
    }
    Ok(total)
}

/// Converts one integer PCM sample to `f32` with the reference arithmetic.
///
/// `f32 = (sample as f32) * 2^-(bits-1)`, evaluated in `f32` exactly as the
/// C does (cast first, then multiply). `bits` must be in `1..=32`.
#[inline]
pub(crate) fn int_pcm_to_f32(sample: i32, bits: u8) -> f32 {
    debug_assert!((1..=32).contains(&bits));
    let scale = 1.0f32 / ((1u32 << (bits - 1)) as f32);
    (sample as f32) * scale
}

/// Builds an [`Error::Invalid`] with a fixed message.
#[inline]
pub(crate) fn invalid(detail: &str) -> Error {
    Error::Invalid {
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scale_is_asymmetric() {
        // Minimum maps exactly to -1.0; positive maximum never reaches +1.0.
        assert_eq!(int_pcm_to_f32(i16::MIN as i32, 16), -1.0);
        assert_eq!(int_pcm_to_f32(i16::MAX as i32, 16), 32767.0 / 32768.0);
        assert_eq!(int_pcm_to_f32(-(1 << 23), 24), -1.0);
        assert_eq!(
            int_pcm_to_f32((1 << 23) - 1, 24),
            (8388607.0f32) / 8388608.0
        );
        assert_eq!(int_pcm_to_f32(i32::MIN, 32), -1.0);
        assert_eq!(int_pcm_to_f32(-128, 8), -1.0);
        assert_eq!(int_pcm_to_f32(127, 8), 127.0 / 128.0);
    }

    #[test]
    fn i32_maximum_rounds_up_to_full_scale() {
        // Unlike the narrower depths, 32-bit integer PCM *can* reach +1.0:
        // the reference casts to f32 before multiplying, and `i32::MAX`
        // (2^31 - 1) rounds up to 2^31 in f32. This is the reference
        // behaviour, not a bug to "fix".
        assert_eq!(int_pcm_to_f32(i32::MAX, 32), 1.0);
        assert_eq!(int_pcm_to_f32(1, 32), 1.0 / 2147483648.0);
    }

    #[test]
    fn open_rejects_short_and_unknown_magic() {
        let short = open(Box::new(Cursor::new(b"RIF".to_vec())));
        assert!(matches!(short, Err(Error::Invalid { .. })));
        let unknown = open(Box::new(Cursor::new(b"\x00\x01\x02\x03rest".to_vec())));
        assert!(matches!(unknown, Err(Error::Invalid { .. })));
    }
}
