//! Native RIFF/WAVE reader — a compatibility port of the reference's reader
//! (`core/libmusicpack/src/audio.c`, `audio_open_wav` / `wav_read_frames`).
//!
//! This is deliberately **not** a "better" WAV parser. It reproduces the
//! reference's acceptance matrix, including its quirks:
//!
//! - `RIFF`/`WAVE` required; the RIFF size field is **never** validated.
//! - Chunk scan stops as soon as both `fmt ` and `data` have been seen, so
//!   chunks after the first `data` are never examined; `data` before `fmt `
//!   is rejected; if several `fmt ` chunks precede `data`, the last wins.
//! - `fmt ` must be ≥ 16 bytes and at most 40 are parsed; a larger chunk is
//!   skipped to its end. Unknown chunks are skipped by `size + (size & 1)`
//!   (odd-size padding); the reference skips **no** pad byte after `fmt `.
//! - Format tag 1 accepts 8/16/24/32-bit PCM; tag 3 accepts 32-bit IEEE
//!   float only. Every other tag (ADPCM, A-law, …) is [`Error::Invalid`].
//! - `WAVE_FORMAT_EXTENSIBLE` (tag `0xFFFE`) resolves the sub-format with
//!   the reference's non-canonical GUID check — see discrepancy **D11** in
//!   `docs/architecture.md`. `wValidBitsPerSample` and `dwChannelMask` are
//!   ignored.
//! - Channels must be 1..=8; the sample rate must be non-zero (there is no
//!   upper bound); `block_align` must equal `channels × bytes_per_sample`
//!   exactly (which rejects 24-bit-in-32-bit-container layouts); `byte_rate`
//!   is never validated.
//!
//! ## Truncated-data semantics
//!
//! `total_frames` is the **declared** `data_len / block_align` and is never
//! clamped to the bytes physically present. A read returns every complete
//! frame the source actually yields, discards a trailing partial frame, and
//! reports `Ok(0)` at EOF — physical truncation is not an error. See
//! `docs/architecture.md` (audio section).

use std::io::Read;

use super::{AudioDecoder, AudioInfo, Codec, MAX_CHANNELS, int_pcm_to_f32, invalid, read_full};
use crate::{Error, Result};

/// The reference-specific `WAVE_FORMAT_EXTENSIBLE` sub-format GUID tail.
///
/// This is the 12 bytes at `fmt + 28` that the reference compares against
/// (its `extensible_subformat` `tail[]`). It is **not** the canonical
/// Windows `KSDATAFORMAT_SUBTYPE_PCM`/`IEEE_FLOAT` tail: the two bytes at
/// `fmt + 26..28` are deliberately unchecked and the reference's fixture
/// `wav24-ext.wav` carries exactly this form. Recorded as discrepancy D11.
const EXTENSIBLE_TAIL: [u8; 12] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
];

/// A native RIFF/WAVE decoding session.
pub struct WavDecoder {
    source: Box<dyn Read>,
    info: AudioInfo,
    /// Bytes per sample (1/2/3/4).
    bytes_per_sample: u8,
    /// Frames still claimable from the declared `data` chunk length.
    data_left: u64,
    /// Reusable raw-byte staging buffer, sized from the caller's request.
    scratch: Vec<u8>,
}

impl WavDecoder {
    /// Opens a RIFF/WAVE stream, reproducing the reference acceptance rules.
    ///
    /// The source must be positioned at the `RIFF` magic. Malformed or
    /// unsupported streams are [`Error::Invalid`]; read failures are
    /// [`Error::Io`].
    pub fn new(mut source: Box<dyn Read>) -> Result<Self> {
        let mut hdr = [0u8; 12];
        if read_full(&mut *source, &mut hdr)? != hdr.len()
            || &hdr[0..4] != b"RIFF"
            || &hdr[8..12] != b"WAVE"
        {
            return Err(invalid("not a RIFF/WAVE stream"));
        }

        let mut have_fmt = false;
        let mut have_data = false;
        let mut fmt_tag: u16 = 0;
        let mut channels: u16 = 0;
        let mut sample_rate: u32 = 0;
        let mut block_align: u16 = 0;
        let mut bits: u16 = 0;
        let mut data_len: u64 = 0;

        while !have_fmt || !have_data {
            let mut chdr = [0u8; 8];
            if read_full(&mut *source, &mut chdr)? != chdr.len() {
                return Err(invalid("WAVE chunk header truncated"));
            }
            let csz = u32::from_le_bytes([chdr[4], chdr[5], chdr[6], chdr[7]]);
            match &chdr[0..4] {
                b"fmt " => {
                    // The reference parses at most 40 bytes and requires 16.
                    let rd = (csz as usize).min(40);
                    if rd < 16 {
                        return Err(invalid("WAVE fmt chunk shorter than 16 bytes"));
                    }
                    let mut fmt = [0u8; 40];
                    if read_full(&mut *source, &mut fmt[..rd])? != rd {
                        return Err(invalid("WAVE fmt chunk truncated"));
                    }
                    // Any bytes beyond the 40 parsed are skipped. Note the
                    // reference does *not* skip a pad byte after `fmt `.
                    if (csz as usize) > rd {
                        skip_bytes(&mut *source, (csz as usize - rd) as u64)?;
                    }
                    fmt_tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                    channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                    sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                    block_align = u16::from_le_bytes([fmt[12], fmt[13]]);
                    bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                    if fmt_tag == 0xFFFE {
                        // WAVE_FORMAT_EXTENSIBLE: resolve the sub-format GUID
                        // at fmt + 24 with the reference's exact check.
                        if rd < 40 {
                            return Err(invalid("WAVE extensible fmt chunk shorter than 40 bytes"));
                        }
                        fmt_tag = extensible_subformat(&fmt[24..40]).ok_or_else(|| {
                            invalid("WAVE extensible sub-format is not PCM/float")
                        })?;
                    }
                    have_fmt = true;
                }
                b"data" => {
                    data_len = csz as u64;
                    have_data = true;
                    if !have_fmt {
                        return Err(invalid("WAVE data chunk precedes fmt chunk"));
                    }
                }
                _ => {
                    // Unknown chunk: skip size plus odd-size padding.
                    skip_bytes(&mut *source, csz as u64 + (csz as u64 & 1))?;
                }
            }
        }

        if channels == 0 || channels > MAX_CHANNELS as u16 || sample_rate == 0 {
            return Err(invalid("WAVE channel count or sample rate out of range"));
        }
        let is_float = match fmt_tag {
            1 => {
                if !matches!(bits, 8 | 16 | 24 | 32) {
                    return Err(invalid("unsupported WAVE PCM bit depth"));
                }
                false
            }
            3 => {
                if bits != 32 {
                    return Err(invalid("IEEE-float WAVE must be 32-bit"));
                }
                true
            }
            // ADPCM (tag 2), A-law, compressed containers, … are rejected
            // explicitly with the reference's INVALID status.
            _ => return Err(invalid("unsupported WAVE format tag")),
        };
        let bytes_per_sample = (bits / 8) as u8;
        if bytes_per_sample == 0 || block_align as u32 != channels as u32 * bytes_per_sample as u32
        {
            return Err(invalid("WAVE block alignment does not match the format"));
        }
        let frame_bytes = channels as u64 * bytes_per_sample as u64;
        let total = data_len / frame_bytes;

        Ok(Self {
            source,
            info: AudioInfo {
                sample_rate,
                channels: channels as u8,
                bits_per_sample: bits as u8,
                total_frames: Some(total),
                codec: Codec::Wav,
                is_float,
            },
            bytes_per_sample,
            data_left: total,
            scratch: Vec::new(),
        })
    }

    /// Reads one bounded block of raw bytes, mirroring the reference's
    /// `malloc(want * ch * bytes)` + `fread` sequence.
    ///
    /// The allocation is sized from the caller's requested frame count
    /// (never from the untrusted declared data length), and a trailing
    /// partial frame is discarded exactly as the reference does.
    fn fill_raw(&mut self, want_frames: usize) -> Result<usize> {
        let frame_bytes = self.info.channels as usize * self.bytes_per_sample as usize;
        let want_bytes = want_frames
            .checked_mul(frame_bytes)
            .ok_or_else(|| invalid("WAVE read size overflows the address space"))?;
        self.scratch.clear();
        self.scratch.resize(want_bytes, 0);
        let got_bytes = read_full(&mut *self.source, &mut self.scratch)?;
        let got_frames = got_bytes / frame_bytes;
        self.data_left -= got_frames as u64;
        Ok(got_frames)
    }

    /// Reads a 24-bit little-endian sample with sign extension.
    #[inline]
    fn read_i24(raw: &[u8]) -> i32 {
        let u = (raw[0] as u32) | ((raw[1] as u32) << 8) | ((raw[2] as u32) << 16);
        if u & 0x0080_0000 != 0 {
            (u | 0xFF00_0000) as i32
        } else {
            u as i32
        }
    }

    /// Converts one raw sample to `f32` (reference `as_f32` branch).
    #[inline]
    fn raw_to_f32(raw: &[u8], bytes: usize, is_float: bool) -> f32 {
        if is_float {
            f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        } else {
            match bytes {
                1 => int_pcm_to_f32(raw[0] as i32 - 128, 8),
                2 => int_pcm_to_f32(i16::from_le_bytes([raw[0], raw[1]]) as i32, 16),
                3 => int_pcm_to_f32(Self::read_i24(raw), 24),
                _ => int_pcm_to_f32(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]), 32),
            }
        }
    }

    /// Converts one raw sample to left-aligned `s32` (reference `s32` branch).
    #[inline]
    fn raw_to_s32(raw: &[u8], bytes: usize) -> i32 {
        match bytes {
            1 => (raw[0] as i32 - 128) << 24,
            2 => (i16::from_le_bytes([raw[0], raw[1]]) as i32) << 16,
            3 => Self::read_i24(raw) << 8,
            _ => i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
        }
    }
}

impl AudioDecoder for WavDecoder {
    fn info(&self) -> &AudioInfo {
        &self.info
    }

    fn read_f32(&mut self, interleaved: &mut [f32]) -> Result<usize> {
        let channels = self.info.channels as usize;
        if interleaved.len() % channels != 0 {
            return Err(invalid("destination length is not a multiple of channels"));
        }
        let requested = interleaved.len() / channels;
        let want = requested.min(self.data_left.min(usize::MAX as u64) as usize);
        if want == 0 {
            return Ok(0);
        }
        let got = self.fill_raw(want)?;
        let samples = got * channels;
        let bytes = self.bytes_per_sample as usize;
        let is_float = self.info.is_float;
        for (i, out) in interleaved[..samples].iter_mut().enumerate() {
            *out = Self::raw_to_f32(&self.scratch[i * bytes..], bytes, is_float);
        }
        Ok(got)
    }

    fn read_s32(&mut self, interleaved: &mut [i32]) -> Result<usize> {
        let channels = self.info.channels as usize;
        if interleaved.len() % channels != 0 {
            return Err(invalid("destination length is not a multiple of channels"));
        }
        if self.info.is_float {
            // IEEE-float WAV has no integer representation; never convert.
            return Err(Error::Unsupported {
                what: "s32 PCM for IEEE-float WAV".to_string(),
            });
        }
        let requested = interleaved.len() / channels;
        let want = requested.min(self.data_left.min(usize::MAX as u64) as usize);
        if want == 0 {
            return Ok(0);
        }
        let got = self.fill_raw(want)?;
        let samples = got * channels;
        let bytes = self.bytes_per_sample as usize;
        for (i, out) in interleaved[..samples].iter_mut().enumerate() {
            *out = Self::raw_to_s32(&self.scratch[i * bytes..], bytes);
        }
        Ok(got)
    }
}

/// Skips `skip` bytes with bounded reads; short input is a malformed chunk.
fn skip_bytes(reader: &mut dyn Read, mut skip: u64) -> Result<()> {
    let mut junk = [0u8; 256];
    while skip > 0 {
        let n = skip.min(junk.len() as u64) as usize;
        if read_full(reader, &mut junk[..n])? != n {
            return Err(invalid("WAVE chunk payload extends past end of stream"));
        }
        skip -= n as u64;
    }
    Ok(())
}

/// The reference's extensible sub-format check.
///
/// Returns the resolved format tag (1 = PCM, 3 = IEEE float) when the
/// reference would accept the GUID. Bytes `guid[2..4]` (fmt + 26..28) are
/// deliberately ignored; see [`EXTENSIBLE_TAIL`] and discrepancy D11.
fn extensible_subformat(guid: &[u8]) -> Option<u16> {
    debug_assert_eq!(guid.len(), 16);
    let tag = u16::from_le_bytes([guid[0], guid[1]]);
    if tag != 1 && tag != 3 {
        return None;
    }
    if guid[4..16] != EXTENSIBLE_TAIL {
        return None;
    }
    Some(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal canonical WAVE byte stream with the given fmt fields
    /// and `data` payload, including an optional unknown odd-sized chunk.
    fn build_wav(
        fmt_tag: u16,
        channels: u16,
        sample_rate: u32,
        block_align: u16,
        bits: u16,
        fmt_extra: &[u8],
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes()); // size unchecked
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        let fmt_len = 16 + fmt_extra.len() as u32;
        out.extend_from_slice(&fmt_len.to_le_bytes());
        out.extend_from_slice(&fmt_tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // byte_rate unchecked
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(fmt_extra);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn open_wav(bytes: Vec<u8>) -> Result<WavDecoder> {
        WavDecoder::new(Box::new(std::io::Cursor::new(bytes)))
    }

    #[test]
    fn canonical_pcm16_round_trips_to_f32_and_s32() {
        // Two frames, stereo: (-32768, 32767), (0, 16384).
        let mut data = Vec::new();
        for v in [-32768i16, 32767, 0, 16384] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut dec = open_wav(build_wav(1, 2, 44100, 4, 16, &[], &data)).unwrap();
        let mut f32buf = [0.0f32; 4];
        assert_eq!(dec.read_f32(&mut f32buf).unwrap(), 2);
        assert_eq!(f32buf, [-1.0, 32767.0 / 32768.0, 0.0, 16384.0 / 32768.0]);

        let mut dec = open_wav(build_wav(1, 2, 44100, 4, 16, &[], &data)).unwrap();
        let mut s32buf = [0i32; 4];
        assert_eq!(dec.read_s32(&mut s32buf).unwrap(), 2);
        assert_eq!(s32buf, [-32768 << 16, 32767 << 16, 0, 16384 << 16]);
        assert_eq!(
            open_wav(build_wav(1, 2, 44100, 4, 16, &[], &data))
                .unwrap()
                .info()
                .total_frames,
            Some(2)
        );
    }

    #[test]
    fn u8_and_i24_conversions_are_exact() {
        // 8-bit mono: 0, 128, 255.
        let mut dec = open_wav(build_wav(1, 1, 8000, 1, 8, &[], &[0, 128, 255])).unwrap();
        let mut buf = [0.0f32; 3];
        assert_eq!(dec.read_f32(&mut buf).unwrap(), 3);
        assert_eq!(buf, [-1.0, 0.0, 127.0 / 128.0]);

        let mut dec = open_wav(build_wav(1, 1, 8000, 1, 8, &[], &[0, 128, 255])).unwrap();
        let mut sbuf = [0i32; 3];
        assert_eq!(dec.read_s32(&mut sbuf).unwrap(), 3);
        assert_eq!(sbuf, [-128 << 24, 0, 127 << 24]);

        // 24-bit mono: 0x800000 (min), 0x000001, 0x7FFFFF (max).
        let data = [0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x7F];
        let mut dec = open_wav(build_wav(1, 1, 8000, 3, 24, &[], &data)).unwrap();
        let mut sbuf = [0i32; 3];
        assert_eq!(dec.read_s32(&mut sbuf).unwrap(), 3);
        assert_eq!(sbuf, [i32::MIN, 1 << 8, 0x7FFF_FF00]);
        let mut dec = open_wav(build_wav(1, 1, 8000, 3, 24, &[], &data)).unwrap();
        let mut buf = [0.0f32; 3];
        assert_eq!(dec.read_f32(&mut buf).unwrap(), 3);
        assert_eq!(buf, [-1.0, 1.0 / 8388608.0, 8388607.0 / 8388608.0]);
    }

    #[test]
    fn i32_wav_conversion_observes_f32_rounding() {
        // i32::MAX rounds up to 2^31 in f32 (so it maps to +1.0); i32::MIN is
        // -1.0; 1 is 2^-31. The s32 path is a verbatim copy.
        let data = [
            0xFF, 0xFF, 0xFF, 0x7F, // i32::MAX
            0x00, 0x00, 0x00, 0x80, // i32::MIN
            0x01, 0x00, 0x00, 0x00, // 1
        ];
        let mut dec = open_wav(build_wav(1, 1, 44100, 4, 32, &[], &data)).unwrap();
        let mut buf = [0.0f32; 3];
        assert_eq!(dec.read_f32(&mut buf).unwrap(), 3);
        assert_eq!(buf, [1.0, -1.0, 1.0 / 2147483648.0]);

        let mut dec = open_wav(build_wav(1, 1, 44100, 4, 32, &[], &data)).unwrap();
        let mut sbuf = [0i32; 3];
        assert_eq!(dec.read_s32(&mut sbuf).unwrap(), 3);
        assert_eq!(sbuf, [i32::MAX, i32::MIN, 1]);
    }

    #[test]
    fn float32_wav_preserves_raw_bits() {
        let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.5, -3.5];
        let mut data = Vec::new();
        for v in values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut dec = open_wav(build_wav(3, 1, 48000, 4, 32, &[], &data)).unwrap();
        assert!(dec.info().is_float);
        let mut buf = [0.0f32; 5];
        assert_eq!(dec.read_f32(&mut buf).unwrap(), 5);
        for (got, want) in buf.iter().zip(values.iter()) {
            assert_eq!(got.to_bits(), want.to_bits(), "raw bits preserved");
        }
        // s32 is structurally unsupported for float WAV.
        let mut dec = open_wav(build_wav(3, 1, 48000, 4, 32, &[], &data)).unwrap();
        assert!(matches!(
            dec.read_s32(&mut [0i32; 1]),
            Err(Error::Unsupported { .. })
        ));

        // Explicit little-endian interpretation: 0x3F800000 is 1.0f32.
        let le_one = [0x00u8, 0x00, 0x80, 0x3F];
        let mut dec = open_wav(build_wav(3, 1, 48000, 4, 32, &[], &le_one)).unwrap();
        let mut one = [0.0f32; 1];
        assert_eq!(dec.read_f32(&mut one).unwrap(), 1);
        assert_eq!(one[0], 1.0);
    }

    #[test]
    fn truncated_data_reports_short_then_eof() {
        // Declared 4 frames (8 bytes), only 2 frames physically present.
        let mut bytes = build_wav(1, 1, 44100, 2, 16, &[], &[0u8, 0, 1, 0, 2, 0, 3, 0]);
        bytes.truncate(bytes.len() - 4);
        let mut dec = open_wav(bytes).unwrap();
        assert_eq!(dec.info().total_frames, Some(4));
        let mut buf = [0i32; 8];
        assert_eq!(dec.read_s32(&mut buf).unwrap(), 2);
        assert_eq!(dec.read_s32(&mut buf).unwrap(), 0);
    }

    #[test]
    fn rejects_adpcm_and_bad_alignment() {
        assert!(open_wav(build_wav(2, 2, 44100, 4, 4, &[], &[0; 8])).is_err());
        // 24-bit stereo must use block_align 6, not 8 (24-in-32 container).
        assert!(open_wav(build_wav(1, 2, 44100, 8, 24, &[], &[0; 12])).is_err());
        // Zero channels and zero rate are rejected.
        assert!(open_wav(build_wav(1, 0, 44100, 0, 16, &[], &[])).is_err());
        assert!(open_wav(build_wav(1, 1, 0, 2, 16, &[], &[])).is_err());
    }

    #[test]
    fn extensible_reference_tail_matrix() {
        // fmt_extra: cbSize(2) + validBits(2) + channelMask(4) + GUID(16).
        let mut guid = vec![1u8, 0]; // sub-format tag 1 (PCM)
        guid.extend_from_slice(&[0xAB, 0xCD]); // fmt+26..28 unchecked
        guid.extend_from_slice(&EXTENSIBLE_TAIL);
        let mut extra = vec![22, 0, 24, 0];
        extra.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]); // channel mask
        extra.extend_from_slice(&guid);
        let mut data = Vec::new();
        for v in [-32768i16, 0, 32767, 1] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Reference tail GUID accepted despite non-canonical bytes.
        let bytes = build_wav(0xFFFE, 2, 44100, 4, 16, &extra, &data);
        let dec = open_wav(bytes).unwrap();
        assert_eq!(dec.info().bits_per_sample, 16);
        assert!(!dec.info().is_float);

        // Canonical Windows PCM GUID is rejected (D11).
        let mut canonical = vec![1u8, 0];
        canonical.extend_from_slice(&[0x00, 0x00]);
        canonical.extend_from_slice(&[
            0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
        let mut extra2 = vec![22, 0, 16, 0];
        extra2.extend_from_slice(&[0, 0, 0, 0]);
        extra2.extend_from_slice(&canonical);
        assert!(open_wav(build_wav(0xFFFE, 2, 44100, 4, 16, &extra2, &data)).is_err());

        // Non-PCM sub-format tag rejected.
        let mut nonpcm = vec![0x50u8, 0x00];
        nonpcm.extend_from_slice(&[0, 0]);
        nonpcm.extend_from_slice(&EXTENSIBLE_TAIL);
        let mut extra3 = vec![22, 0, 16, 0];
        extra3.extend_from_slice(&[0, 0, 0, 0]);
        extra3.extend_from_slice(&nonpcm);
        assert!(open_wav(build_wav(0xFFFE, 2, 44100, 4, 16, &extra3, &data)).is_err());
    }
}
