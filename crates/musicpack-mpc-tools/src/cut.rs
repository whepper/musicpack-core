//! SV8 stream cutting: the Rust equivalent of legacy `mpccut`.
//!
//! Source-derived port of `codec/mpccut/mpccut.c` (LGPL-2.1-or-later). The C
//! tool links `mpcenc_static` only for container writing (`writeMagic`,
//! `writeStreamInfo`, `writeBlock`, `writeBits`, `writeSeekTable`) — it never
//! calls the filterbank, psychoacoustics, allocation, quantisation or Huffman
//! coder. This module replaces exactly those container operations with
//! [`musicpack_musepack_encoder::blocks::Sv8StreamWriter`] and copies `AP`
//! payloads verbatim, so the legacy encoder library is no longer needed.
//!
//! ## Semantics (mirroring the reference)
//!
//! * `start_sample`/`end_sample` are stream samples; `end_sample == 0` means
//!   end-of-stream. Both are shifted by the input `beg_silence`, then
//!   validated (`0 <= start < end <= samples`).
//! * The output `SH` carries the cut-relative sample count and the
//!   block-aligned leading silence (`beg_silence`); `RG` is zeroed; the first
//!   input `EI` block is copied verbatim.
//! * `AP` blocks are copied byte-for-byte (never re-encoded); the seek table
//!   is rebuilt with `seek_pwr = 1`, then `ST`/`SE` are appended.
//! * Trailing input blocks (old `ST`/`SE`, tags) are dropped, as in the
//!   reference.

use musicpack_core::audio::musepack::sv8::{self, MusepackInfo};
use musicpack_musepack_encoder::blocks::{GainInfo, StreamInfo, Sv8StreamWriter};
use musicpack_musepack_encoder::error::EncoderError;

/// Samples per Musepack frame (`MPC_FRAME_LENGTH`).
const FRAME_LENGTH: u64 = 1152;

/// Error cutting an SV8 stream.
#[derive(Debug, Clone, PartialEq)]
pub enum CutError {
    /// Input is not a decodable SV8 stream (truncated, bad key/size, SV7…).
    InvalidInput(String),
    /// Requested sample range is out of stream bounds.
    OutOfRange {
        /// Requested start (after `beg_silence` shift).
        start: u64,
        /// Requested end (after `beg_silence` shift, `0` = end-of-stream).
        end: u64,
        /// Total stream samples.
        samples: u64,
    },
    /// A container primitive rejected constructed values.
    Container(EncoderError),
}

impl std::fmt::Display for CutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid SV8 input: {msg}"),
            Self::OutOfRange {
                start,
                end,
                samples,
            } => write!(
                f,
                "sample range [{start}, {end}) out of stream bounds ({samples} samples)"
            ),
            Self::Container(e) => write!(f, "container error: {e}"),
        }
    }
}

impl std::error::Error for CutError {}

impl From<EncoderError> for CutError {
    fn from(e: EncoderError) -> Self {
        Self::Container(e)
    }
}

/// One raw top-level block: byte range covering key + size + payload.
struct RawBlock {
    key: [u8; 2],
    start: usize,
    end: usize,
}

/// Splits `bytes` (after the 4-byte magic) into raw blocks.
///
/// Mirrors `mpc_bits_get_block` + `mpc_check_key`: ASCII-uppercase keys only,
/// big-endian base-128 sizes that must at least cover their own header.
fn split_blocks(bytes: &[u8]) -> Result<Vec<RawBlock>, CutError> {
    if bytes.len() < 4 || &bytes[0..4] != b"MPCK" {
        return Err(CutError::InvalidInput("missing MPCK magic".into()));
    }
    let mut out = Vec::new();
    let mut i = 4usize;
    while i + 2 <= bytes.len() {
        let key = [bytes[i], bytes[i + 1]];
        if !key[0].is_ascii_uppercase() || !key[1].is_ascii_uppercase() {
            return Err(CutError::InvalidInput(format!(
                "invalid block key {key:?} at offset {i}"
            )));
        }
        // Big-endian base-128 varint starting at i+2.
        let mut declared: u64 = 0;
        let mut header_len = 2usize;
        let mut pos = i + 2;
        loop {
            if pos >= bytes.len() {
                return Err(CutError::InvalidInput("truncated block size".into()));
            }
            let byte = bytes[pos];
            pos += 1;
            header_len += 1;
            declared = (declared << 7) | u64::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                break;
            }
            if header_len > 12 {
                return Err(CutError::InvalidInput("oversize block size".into()));
            }
        }
        let total = usize::try_from(declared)
            .map_err(|_| CutError::InvalidInput("block size overflows usize".into()))?;
        if total < header_len {
            return Err(CutError::InvalidInput(format!(
                "block smaller than its header at offset {i}"
            )));
        }
        let end = i
            .checked_add(total)
            .ok_or_else(|| CutError::InvalidInput("block end overflows usize".into()))?;
        if end > bytes.len() {
            return Err(CutError::InvalidInput("truncated block payload".into()));
        }
        let start = i;
        out.push(RawBlock { key, start, end });
        if &key == b"SE" {
            break;
        }
        i = end;
    }
    Ok(out)
}

/// Cuts `[start_sample, end_sample)` from an SV8 stream.
///
/// `end_sample == 0` selects end-of-stream, matching `mpccut -e` omission.
/// Returns the new SV8 stream bytes.
pub fn cut(input: &[u8], start_sample: u64, end_sample: u64) -> Result<Vec<u8>, CutError> {
    let info: MusepackInfo = sv8::parse(input).map_err(|e| CutError::InvalidInput(e.0))?;
    cut_with_info(input, &info, start_sample, end_sample)
}

fn cut_with_info(
    input: &[u8],
    info: &MusepackInfo,
    start_sample: u64,
    end_sample: u64,
) -> Result<Vec<u8>, CutError> {
    let end_sample = if end_sample == 0 {
        info.samples
    } else {
        end_sample
    };
    let start_sample = start_sample.saturating_add(info.beg_silence);
    let end_sample = end_sample.saturating_add(info.beg_silence);
    if start_sample >= end_sample || end_sample > info.samples {
        return Err(CutError::OutOfRange {
            start: start_sample,
            end: end_sample,
            samples: info.samples,
        });
    }

    let block_frames: u64 = FRAME_LENGTH << info.block_pwr;
    let beg_silence = start_sample % block_frames;
    let start_block = start_sample / block_frames;
    let block_num = end_sample
        .div_ceil(block_frames)
        .saturating_sub(start_block);
    let out_samples = end_sample - start_block * block_frames;

    let blocks = split_blocks(input)?;
    // First EI block before the first AP, copied verbatim (may be absent).
    let mut ei: Option<(usize, usize)> = None;
    for b in &blocks {
        if &b.key == b"AP" {
            break;
        }
        if &b.key == b"EI" && ei.is_none() {
            ei = Some((b.start, b.end));
        }
    }
    let ap_blocks: Vec<(usize, usize)> = blocks
        .iter()
        .filter(|b| &b.key == b"AP")
        .map(|b| (b.start, b.end))
        .collect();
    let start_block = usize::try_from(start_block)
        .map_err(|_| CutError::InvalidInput("start block overflows usize".into()))?;
    let block_num = usize::try_from(block_num)
        .map_err(|_| CutError::InvalidInput("block count overflows usize".into()))?;
    if start_block + block_num > ap_blocks.len() {
        return Err(CutError::InvalidInput(format!(
            "stream has {} AP blocks, need {}",
            ap_blocks.len(),
            start_block + block_num
        )));
    }

    let mut w = Sv8StreamWriter::new();
    w.write_magic();
    w.write_stream_info(&StreamInfo {
        samples: out_samples,
        beg_silence,
        sample_rate: info.sample_rate,
        max_band: u32::from(info.max_band),
        channels: u32::from(info.channels),
        ms: info.ms,
        frames_per_block_pwr: u32::from(info.block_pwr),
    })?;
    w.write_gain_info(&GainInfo::default())?;
    if let Some((s, e)) = ei {
        w.append(&input[s..e]);
    }
    w.write_seek_offset()?;

    let mut block_cnt = 0u32;
    let mut seek_pos = 0u32;
    let mut entries = Vec::new();
    // seek_pwr = 1, mirroring `mpc_encoder_init(&e, end_sample, si.block_pwr, 1)`.
    // (Loop kept in reference form — counter mirrors `e.block_cnt` — with an
    // allow for the explicit-counter lint.)
    #[allow(clippy::explicit_counter_loop)]
    for (s, e) in ap_blocks.iter().skip(start_block).take(block_num) {
        if block_cnt & ((1 << 1) - 1) == 0 {
            entries.push(w.position());
            seek_pos += 1;
        }
        block_cnt += 1;
        w.append(&input[*s..*e]);
    }
    w.write_seek_table(seek_pos, 1, &entries)?;
    w.write_end()?;
    Ok(w.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_sv8() {
        assert!(matches!(
            cut(b"RIFF....", 0, 0),
            Err(CutError::InvalidInput(_))
        ));
        assert!(matches!(cut(b"MPCK", 0, 0), Err(CutError::InvalidInput(_))));
    }

    #[test]
    fn rejects_bad_range() {
        // Well-formed SH: version 8, samples 2311, beg_silence 0, 44.1k,
        // max_band 28, stereo, MS, fpb>>1 = 3. Built via the writer itself.
        let mut w = Sv8StreamWriter::new();
        w.write_magic();
        w.write_stream_info(&StreamInfo {
            samples: 2311,
            beg_silence: 0,
            sample_rate: 44100,
            max_band: 28,
            channels: 2,
            ms: true,
            frames_per_block_pwr: 6,
        })
        .unwrap();
        w.write_end().unwrap();
        let bytes = w.into_bytes();
        assert!(matches!(
            cut(&bytes, 2311, 2311),
            Err(CutError::OutOfRange { .. })
        ));
        assert!(matches!(
            cut(&bytes, 0, 2312),
            Err(CutError::OutOfRange { .. })
        ));
    }
}
