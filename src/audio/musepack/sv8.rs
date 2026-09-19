//! Musepack SV8 container and stream-info parsing.
//!
//! A faithful port of the header path in the project's vendored `libmpcdec`
//! (`mpc_demux.c` `mpc_demux_header`, `streaminfo.c`
//! `streaminfo_read_header_sv8` / `streaminfo_encoder_info` /
//! `streaminfo_gain`). It parses the `MPCK` block stream, validates the
//! `SH` header CRC-32 (IEEE), and exposes the stream metadata the reference
//! decoder exposes. All input is treated as hostile: sizes are bounded,
//! checked and never trusted before validation.
//!
//! **Scope:** this module implements the container/metadata layer plus the
//! packet framing the audio path will consume. Audio synthesis is not
//! implemented (see the module docs).

use std::fmt;

/// SV8 file magic.
pub const MAGIC: &[u8; 4] = b"MPCK";
/// Samples per Musepack frame (`36 * 32`).
pub const FRAME_LENGTH: u64 = 36 * 32;
/// Synthesizer delay prepended to every stream (`MPC_DECODER_SYNTH_DELAY`).
pub const DECODER_SYNTH_DELAY: u64 = 481;
/// Upper bound accepted for a variable-length size (2^48) — absurd values are
/// rejected rather than allocated against.
pub const MAX_SIZE: u64 = 1 << 48;

const SAMPLE_RATES: [u32; 4] = [44_100, 48_000, 37_800, 32_000];

/// A malformed or unsupported Musepack stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusepackError(pub String);

impl fmt::Display for MusepackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MusepackError {}

/// Parsed SV8 stream facts (the reference's `mpc_streaminfo` subset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusepackInfo {
    /// Stream version (always 8 for this parser).
    pub stream_version: u8,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count (1 or 2).
    pub channels: u8,
    /// Total decoded sample-frames including decoder delay.
    pub samples: u64,
    /// Leading sample-frames to discard (`beg_silence`).
    pub beg_silence: u64,
    /// Highest used subband + 1 (validated to 1..32).
    pub max_band: u8,
    /// Mid/side stereo enabled.
    pub ms: bool,
    /// Audio block size is `1 << block_pwr` frames.
    pub block_pwr: u8,
    /// Always true for SV8.
    pub is_true_gapless: bool,
    /// Replay-gain title value (0 when absent).
    pub gain_title: u16,
    /// Replay-gain album value (0 when absent).
    pub gain_album: u16,
    /// Replay-gain peak title value (0 when absent).
    pub peak_title: u16,
    /// Replay-gain peak album value (0 when absent).
    pub peak_album: u16,
    /// Encoder version word (0 when absent).
    pub encoder_version: u32,
    /// Encoder profile (`0..=15`; `0` when absent).
    pub profile: u8,
    /// PNS (noise substitution) flagged by the encoder info block.
    pub pns: bool,
    /// An `RG` block was present.
    pub has_replay_gain: bool,
    /// An `EI` block was present.
    pub has_encoder_info: bool,
    /// Byte offset of the first `AP` (audio packet) block header.
    pub audio_offset: Option<u64>,
}

impl Default for MusepackInfo {
    fn default() -> Self {
        Self {
            stream_version: 0,
            sample_rate: 0,
            channels: 0,
            samples: 0,
            beg_silence: 0,
            max_band: 0,
            ms: false,
            block_pwr: 0,
            is_true_gapless: true,
            gain_title: 0,
            gain_album: 0,
            peak_title: 0,
            peak_album: 0,
            encoder_version: 0,
            profile: 0,
            pns: false,
            has_replay_gain: false,
            has_encoder_info: false,
            audio_offset: None,
        }
    }
}

impl MusepackInfo {
    /// Playable sample-frames (`samples - beg_silence`), the reference's
    /// `mpc_streaminfo_get_length_samples`.
    pub fn length_samples(&self) -> u64 {
        self.samples.saturating_sub(self.beg_silence)
    }

    /// Playable duration in seconds.
    pub fn length_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.length_samples() as f64 / self.sample_rate as f64
        }
    }
}

/// An audio (`AP`) block located in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioBlock {
    /// Byte offset of the block's payload (after the key/size header).
    pub offset: u64,
    /// Declared audio payload size in bytes.
    pub size: u64,
}

/// Parses the SV8 container header from a bounded prefix of the file.
///
/// The prefix must include the leading metadata blocks; the first `AP` block
/// terminates parsing and is recorded in [`MusepackInfo::audio_offset`].
pub fn parse(bytes: &[u8]) -> Result<MusepackInfo, MusepackError> {
    if bytes.len() < 4 || &bytes[0..4] != MAGIC {
        return Err(MusepackError("not a Musepack SV8 stream (MPCK)".into()));
    }
    let mut info = MusepackInfo::default();
    let mut have_header = false;
    let mut i = 4usize;

    while i + 2 <= bytes.len() {
        let key = [bytes[i], bytes[i + 1]];
        i += 2;
        if !key[0].is_ascii_uppercase() || !key[1].is_ascii_uppercase() {
            return Err(MusepackError("invalid SV8 block key".into()));
        }
        let (declared, size_bytes) = read_varint(bytes, i)?;
        i += size_bytes;
        let header_len = 2u64 + size_bytes as u64;
        if declared < header_len {
            return Err(MusepackError("SV8 block smaller than its header".into()));
        }
        let payload_len = usize::try_from(declared - header_len)
            .map_err(|_| MusepackError("SV8 block length overflows usize".into()))?;
        if payload_len > bytes.len() {
            return Err(MusepackError("SV8 block length exceeds input".into()));
        }
        if i.checked_add(payload_len)
            .is_none_or(|end| end > bytes.len())
        {
            // The bounded prefix ends inside a block: metadata seen so far
            // stands, later blocks are simply not visible.
            break;
        }
        let payload = &bytes[i..i + payload_len];
        match &key {
            b"SH" => {
                parse_stream_header(&mut info, payload)?;
                have_header = true;
            }
            b"RG" => parse_replay_gain(&mut info, payload),
            b"EI" => parse_encoder_info(&mut info, payload),
            b"SE" => break,
            b"AP" => {
                info.audio_offset = Some((i - header_len as usize) as u64);
                break;
            }
            _ => {}
        }
        i += payload_len;
    }

    if !have_header || info.stream_version != 8 {
        return Err(MusepackError(
            "SV8 stream has no usable SH header block".into(),
        ));
    }
    if info.max_band == 0
        || info.max_band >= 32
        || info.channels == 0
        || info.channels > 2
        || info.sample_rate == 0
        || info.beg_silence > info.samples
    {
        return Err(MusepackError("invalid SV8 stream information".into()));
    }
    Ok(info)
}

/// Walks the top-level block stream, returning each audio (`AP`) block's
/// offset and declared payload size in order.
///
/// This is the container navigation the audio path will drive; it does not
/// decode.
pub fn audio_blocks(bytes: &[u8]) -> Result<Vec<AudioBlock>, MusepackError> {
    if bytes.len() < 4 || &bytes[0..4] != MAGIC {
        return Err(MusepackError("not a Musepack SV8 stream (MPCK)".into()));
    }
    let mut out = Vec::new();
    let mut i = 4usize;
    while i + 2 <= bytes.len() {
        let key = [bytes[i], bytes[i + 1]];
        i += 2;
        let (declared, size_bytes) = read_varint(bytes, i)?;
        i += size_bytes;
        let header_len = 2u64 + size_bytes as u64;
        if declared < header_len {
            return Err(MusepackError("SV8 block smaller than its header".into()));
        }
        let payload_len = usize::try_from(declared - header_len)
            .map_err(|_| MusepackError("SV8 block length overflows usize".into()))?;
        if i.checked_add(payload_len)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(MusepackError("truncated SV8 block stream".into()));
        }
        if &key == b"AP" {
            out.push(AudioBlock {
                offset: i as u64,
                size: declared - header_len,
            });
        } else if &key == b"SE" {
            break;
        }
        i += payload_len;
    }
    Ok(out)
}

/// Applies one header block (`SH`/`RG`/`EI`) to `info`; other keys are ignored.
///
/// Shared by the whole-buffer [`parse`] and the streaming demux in
/// [`super::decoder`].
pub(crate) fn handle_header_block(
    info: &mut MusepackInfo,
    key: &[u8; 2],
    payload: &[u8],
) -> Result<(), MusepackError> {
    match key {
        b"SH" => parse_stream_header(info, payload)?,
        b"RG" => parse_replay_gain(info, payload),
        b"EI" => parse_encoder_info(info, payload),
        _ => {}
    }
    Ok(())
}

/// Validates parsed stream facts (the reference's `check_streaminfo`).
pub(crate) fn validate(info: &MusepackInfo) -> Result<(), MusepackError> {
    if info.stream_version != 8 {
        return Err(MusepackError(
            "SV8 stream has no usable SH header block".into(),
        ));
    }
    if info.max_band == 0
        || info.max_band >= 32
        || info.channels == 0
        || info.channels > 2
        || info.sample_rate == 0
        || info.beg_silence > info.samples
    {
        return Err(MusepackError("invalid SV8 stream information".into()));
    }
    Ok(())
}

fn parse_stream_header(info: &mut MusepackInfo, payload: &[u8]) -> Result<(), MusepackError> {
    if payload.len() < 5 {
        return Err(MusepackError("SV8 SH block too short".into()));
    }
    let stored = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    if crc32(&payload[4..]) != stored {
        return Err(MusepackError("SV8 SH header CRC mismatch".into()));
    }
    let mut r = BitReader::new(&payload[4..]);
    let version = r.read(8)?;
    if version != 8 {
        return Err(MusepackError(format!(
            "unsupported Musepack stream version {version}"
        )));
    }
    info.stream_version = version as u8;
    info.samples = r.get_size()?;
    info.beg_silence = r.get_size()?;
    if info.beg_silence > info.samples {
        return Err(MusepackError(
            "SV8 beg_silence exceeds total samples".into(),
        ));
    }
    info.is_true_gapless = true;
    let freq = r.read(3)? as usize;
    info.sample_rate = *SAMPLE_RATES
        .get(freq)
        .ok_or_else(|| MusepackError("invalid SV8 sample-rate index".into()))?;
    info.max_band = (r.read(5)? + 1) as u8;
    info.channels = (r.read(4)? + 1) as u8;
    info.ms = r.read(1)? != 0;
    info.block_pwr = (r.read(3)? * 2) as u8;
    Ok(())
}

fn parse_replay_gain(info: &mut MusepackInfo, payload: &[u8]) {
    let mut r = BitReader::new(payload);
    let Ok(version) = r.read(8) else {
        return;
    };
    if version != 1 {
        return;
    }
    let (Ok(gain_title), Ok(peak_title), Ok(gain_album), Ok(peak_album)) =
        (r.read(16), r.read(16), r.read(16), r.read(16))
    else {
        return;
    };
    info.gain_title = gain_title as u16;
    info.peak_title = peak_title as u16;
    info.gain_album = gain_album as u16;
    info.peak_album = peak_album as u16;
    info.has_replay_gain = true;
}

fn parse_encoder_info(info: &mut MusepackInfo, payload: &[u8]) {
    let mut r = BitReader::new(payload);
    let (Ok(profile), Ok(pns), Ok(major), Ok(minor), Ok(build)) =
        (r.read(7), r.read(1), r.read(8), r.read(8), r.read(8))
    else {
        return;
    };
    info.profile = (profile / 8) as u8;
    info.pns = pns != 0;
    info.encoder_version = (major << 24) | (minor << 16) | (build << 8);
    info.has_encoder_info = true;
}

fn read_varint(bytes: &[u8], mut at: usize) -> Result<(u64, usize), MusepackError> {
    let mut size = 0u64;
    let mut count = 0usize;
    loop {
        let byte = *bytes
            .get(at)
            .ok_or_else(|| MusepackError("truncated SV8 size field".into()))?;
        at += 1;
        count += 1;
        size = size
            .checked_mul(128)
            .and_then(|s| s.checked_add((byte & 0x7f) as u64))
            .ok_or_else(|| MusepackError("SV8 size overflow".into()))?;
        if size > MAX_SIZE {
            return Err(MusepackError("SV8 size exceeds the accepted bound".into()));
        }
        if byte & 0x80 == 0 {
            return Ok((size, count));
        }
        if count > 10 {
            return Err(MusepackError("SV8 size field too long".into()));
        }
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read(&mut self, nbits: u32) -> Result<u32, MusepackError> {
        debug_assert!(nbits <= 24);
        let mut value = 0u32;
        for _ in 0..nbits {
            let byte = self
                .data
                .get(self.bit >> 3)
                .ok_or_else(|| MusepackError("truncated SV8 header field".into()))?;
            let shift = 7 - (self.bit & 7);
            value = (value << 1) | ((byte >> shift) & 1) as u32;
            self.bit += 1;
        }
        Ok(value)
    }

    fn get_size(&mut self) -> Result<u64, MusepackError> {
        let mut size = 0u64;
        loop {
            let byte = self.read(8)?;
            size = size
                .checked_mul(128)
                .and_then(|s| s.checked_add((byte & 0x7f) as u64))
                .ok_or_else(|| MusepackError("SV8 size overflow".into()))?;
            if size > MAX_SIZE {
                return Err(MusepackError("SV8 size exceeds the accepted bound".into()));
            }
            if byte & 0x80 == 0 {
                return Ok(size);
            }
        }
    }
}

/// CRC-32 (IEEE, reflected, init/final `0xFFFF_FFFF`) — the `mpc_crc32`
/// variant the SV8 header check uses.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vector() {
        // Standard check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn varint_round_trips() {
        let (value, n) = read_varint(&[0x0e], 0).unwrap();
        assert_eq!((value, n), (14, 1));
        let (value, n) = read_varint(&[0x81, 0x02], 0).unwrap();
        assert_eq!((value, n), ((1 << 7) | 2, 2));
    }

    #[test]
    fn rejects_non_magic_and_truncation() {
        assert!(parse(b"XXXX").is_err());
        assert!(parse(b"MPCK").is_err());
        assert!(parse(b"MPCK\x53\x48").is_err());
    }

    #[test]
    fn varint_overflow_is_rejected() {
        let long = [0xffu8; 16];
        assert!(read_varint(&long, 0).is_err());
    }

    #[test]
    fn crc_mismatch_is_rejected() {
        // MPCK + SH + size 9 + CRC(4) + 5 garbage bytes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MPCK");
        bytes.extend_from_slice(b"SH");
        bytes.push(9);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&[8, 0, 0, 0, 0]);
        assert!(parse(&bytes).is_err());
    }
}
