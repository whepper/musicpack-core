//! SV8 block payloads and the stream-level block writers.
//!
//! Each block type the reference encoder emits is built here as a deterministic
//! payload, then framed through [`crate::sv8::write_block`]. Field order and
//! widths follow the reference exactly:
//!
//! | Block | Payload |
//! |---|---|
//! | `SH` | version 8, sample counts, rate index, `MaxBand-1`, `Channels-1`, MS flag, `frames_per_block_pwr>>1`; framed with a CRC |
//! | `RG` | version 1, title gain/peak, album gain/peak (16-bit each) |
//! | `EI` | `profile*8+0.5` (7-bit), PNS flag, major/minor/build (8-bit each) |
//! | `SO` | 40 reserved bits for the seek-table offset, patched after the audio blocks |
//! | `ST` | seek position, power, first two absolute offsets, then Golomb-coded second differences |
//! | `SE` | empty |
//!
//! `AP` (audio) is deliberately not written here: audio-frame coding belongs to
//! a later phase.

use crate::bitwriter::BitWriter;
use crate::error::EncoderError;
use crate::sv8::{BlockKey, encode_golomb, encode_size, seek_delta, write_block};

/// The SV8 `SH` stream-header fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// Total samples per channel.
    pub samples: u64,
    /// Samples to skip at the start (always 0 in reference encoder output).
    pub beg_silence: u64,
    /// One of 44100, 48000, 37800 or 32000.
    pub sample_rate: u32,
    /// `1..=32`.
    pub max_band: u32,
    /// `1..=16` (the encoder emits 1 or 2).
    pub channels: u32,
    /// M/S coding flag.
    pub ms: bool,
    /// `1 << frames_per_block_pwr` frames per `AP` block; `0..=14`.
    pub frames_per_block_pwr: u32,
}

/// The SV8 `RG` replay-gain fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GainInfo {
    /// Title gain (16-bit).
    pub title_gain: u16,
    /// Title peak (16-bit).
    pub title_peak: u16,
    /// Album gain (16-bit).
    pub album_gain: u16,
    /// Album peak (16-bit).
    pub album_peak: u16,
}

/// The SV8 `EI` encoder-info fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderInfo {
    /// Encoder profile (the reference passes its `FullQual`).
    pub profile: f32,
    /// Whether PNS is enabled.
    pub pns: bool,
    /// Encoder version major.
    pub major: u32,
    /// Encoder version minor.
    pub minor: u32,
    /// Encoder version build.
    pub build: u32,
}

const SAMPLE_RATES: [u32; 4] = [44100, 48000, 37800, 32000];

fn sample_rate_index(rate: u32) -> Result<u32, EncoderError> {
    SAMPLE_RATES
        .iter()
        .position(|&candidate| candidate == rate)
        .map(|index| index as u32)
        .ok_or(EncoderError::InvalidSampleRate(rate))
}

/// Builds the `SH` payload (without the block key, size or CRC).
pub fn stream_info_payload(info: &StreamInfo) -> Result<Vec<u8>, EncoderError> {
    let freq = sample_rate_index(info.sample_rate)?;
    if !(1..=32).contains(&info.max_band) {
        return Err(EncoderError::InvalidBandCount(info.max_band));
    }
    if !(1..=16).contains(&info.channels) {
        return Err(EncoderError::InvalidChannelCount(info.channels));
    }
    if info.frames_per_block_pwr > 14 {
        return Err(EncoderError::InvalidBlockPower(info.frames_per_block_pwr));
    }

    let mut w = BitWriter::new();
    w.write_bits(8, 8); // stream version
    for &byte in encode_size(info.samples).as_bytes() {
        w.write_bits(u32::from(byte), 8);
    }
    for &byte in encode_size(info.beg_silence).as_bytes() {
        w.write_bits(u32::from(byte), 8);
    }
    w.write_bits(freq, 3);
    w.write_bits(info.max_band - 1, 5);
    w.write_bits(info.channels - 1, 4);
    w.write_bits(info.ms as u32, 1);
    w.write_bits(info.frames_per_block_pwr >> 1, 3);
    Ok(w.finish())
}

/// Builds the `RG` payload (without the block key or size).
pub fn gain_info_payload(info: &GainInfo) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(1, 8); // version
    w.write_bits(u32::from(info.title_gain), 16);
    w.write_bits(u32::from(info.title_peak), 16);
    w.write_bits(u32::from(info.album_gain), 16);
    w.write_bits(u32::from(info.album_peak), 16);
    w.finish()
}

/// Builds the `EI` payload (without the block key or size).
pub fn encoder_info_payload(info: &EncoderInfo) -> Result<Vec<u8>, EncoderError> {
    // Reference: `(mpc_uint32_t)(profile * 8 + .5)`, computed in double.
    let scaled = f64::from(info.profile) * 8.0 + 0.5;
    if !scaled.is_finite() || !(0.0..128.0).contains(&scaled) {
        return Err(EncoderError::InvalidProfile(f64::from(info.profile)));
    }
    let mut w = BitWriter::new();
    w.write_bits(scaled as u32, 7);
    w.write_bits(info.pns as u32, 1);
    w.write_bits(info.major, 8);
    w.write_bits(info.minor, 8);
    w.write_bits(info.build, 8);
    Ok(w.finish())
}

/// The reserved `SO` payload: 40 zero bits the seek-table offset is patched
/// into later.
pub fn seek_offset_payload() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 16);
    w.write_bits(0, 24);
    w.finish()
}

/// Builds the `ST` payload (without the block key or size).
///
/// `entries` are the absolute offsets the reference records when an `AP` block
/// is flushed; `seek_ref` is the stream start.
pub fn seek_table_payload(
    seek_ref: u64,
    seek_pos: u32,
    seek_pwr: u32,
    entries: &[u64],
) -> Result<Vec<u8>, EncoderError> {
    if seek_pos == 0 || entries.is_empty() {
        return Err(EncoderError::SeekTableEmpty);
    }
    if entries.len() != seek_pos as usize {
        return Err(EncoderError::SeekTableLengthMismatch {
            declared: seek_pos,
            entries: entries.len(),
        });
    }

    let mut w = BitWriter::new();
    for &byte in encode_size(u64::from(seek_pos)).as_bytes() {
        w.write_bits(u32::from(byte), 8);
    }
    w.write_bits(seek_pwr, 4);

    let write_offset = |w: &mut BitWriter, entry: u64| {
        for &byte in encode_size(entry.wrapping_sub(seek_ref)).as_bytes() {
            w.write_bits(u32::from(byte), 8);
        }
    };
    write_offset(&mut w, entries[0]);
    if seek_pos > 1 {
        write_offset(&mut w, entries[1]);
    }
    for i in 2..seek_pos as usize {
        let diff = entries[i]
            .wrapping_sub(entries[i - 1])
            .wrapping_sub(entries[i - 1])
            .wrapping_add(entries[i - 2]) as i64;
        encode_golomb(&mut w, seek_delta(diff), 12);
    }
    Ok(w.finish())
}

/// Writes the SV8 stream-level blocks in order (no audio).
///
/// The writer owns the output buffer and the seek-table placeholder so the
/// `SO`/`ST` patch is handled in one place. It does not know how to write `AP`.
#[derive(Debug, Default)]
pub struct Sv8StreamWriter {
    out: Vec<u8>,
    seek_ref: u64,
    seek_ptr: Option<u64>,
}

impl Sv8StreamWriter {
    /// Creates an empty writer with `seek_ref` 0 (a fresh stream).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty writer whose seek entries are relative to `seek_ref`.
    pub fn with_seek_ref(seek_ref: u64) -> Self {
        Self {
            out: Vec::new(),
            seek_ref,
            seek_ptr: None,
        }
    }

    /// Appends the `MPCK` magic.
    pub fn write_magic(&mut self) {
        self.out.extend_from_slice(b"MPCK");
    }

    /// Writes the `SH` block (framed with a CRC) and returns its payload length.
    pub fn write_stream_info(&mut self, info: &StreamInfo) -> Result<usize, EncoderError> {
        let payload = stream_info_payload(info)?;
        let block = write_block(&mut self.out, BlockKey::SH, &payload, true);
        Ok(block.data_len)
    }

    /// Writes the `RG` block.
    pub fn write_gain_info(&mut self, info: &GainInfo) -> Result<(), EncoderError> {
        let payload = gain_info_payload(info);
        write_block(&mut self.out, BlockKey::RG, &payload, false);
        Ok(())
    }

    /// Writes the `EI` block.
    pub fn write_encoder_info(&mut self, info: &EncoderInfo) -> Result<(), EncoderError> {
        let payload = encoder_info_payload(info)?;
        write_block(&mut self.out, BlockKey::EI, &payload, false);
        Ok(())
    }

    /// Reserves the `SO` placeholder for the seek-table offset.
    pub fn write_seek_offset(&mut self) -> Result<(), EncoderError> {
        self.seek_ptr = Some(self.out.len() as u64);
        let payload = seek_offset_payload();
        write_block(&mut self.out, BlockKey::SO, &payload, false);
        Ok(())
    }

    /// Patches the `SO` placeholder and appends the `ST` block.
    pub fn write_seek_table(
        &mut self,
        seek_pos: u32,
        seek_pwr: u32,
        entries: &[u64],
    ) -> Result<(), EncoderError> {
        let seek_ptr = self.seek_ptr.ok_or(EncoderError::SeekTableNotReserved)?;
        let patch = encode_size(self.out.len() as u64 - seek_ptr);
        let at = seek_ptr as usize + 3;
        if patch.len() > 5 || at + patch.len() > self.out.len() {
            return Err(EncoderError::SeekPatchOutOfRange);
        }
        self.out[at..at + patch.len()].copy_from_slice(patch.as_bytes());

        let payload = seek_table_payload(self.seek_ref, seek_pos, seek_pwr, entries)?;
        write_block(&mut self.out, BlockKey::ST, &payload, false);
        Ok(())
    }

    /// Appends the `SE` end-of-stream block.
    pub fn write_end(&mut self) -> Result<(), EncoderError> {
        write_block(&mut self.out, BlockKey::SE, &[], false);
        Ok(())
    }

    /// The bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    /// The current stream length, i.e. the file offset of the next block.
    pub fn position(&self) -> u64 {
        self.out.len() as u64
    }

    /// Appends raw bytes (used for the `AP` blocks produced by the frame
    /// encoder, which the reference writes through the same file position).
    pub fn append(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    /// Consumes the writer and returns the bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> StreamInfo {
        StreamInfo {
            samples: 44100,
            beg_silence: 0,
            sample_rate: 44100,
            max_band: 28,
            channels: 2,
            ms: true,
            frames_per_block_pwr: 6,
        }
    }

    #[test]
    fn stream_info_matches_the_reference_bytes() {
        // From container-sine44-q5.txt: 08 82 d8 44 00 1b 1b.
        assert_eq!(
            stream_info_payload(&info()).unwrap(),
            [0x08, 0x82, 0xd8, 0x44, 0x00, 0x1b, 0x1b]
        );
    }

    #[test]
    fn stream_info_rejects_unsupported_rates_and_ranges() {
        assert_eq!(
            stream_info_payload(&StreamInfo {
                sample_rate: 96000,
                ..info()
            }),
            Err(EncoderError::InvalidSampleRate(96000))
        );
        assert_eq!(
            stream_info_payload(&StreamInfo {
                max_band: 33,
                ..info()
            }),
            Err(EncoderError::InvalidBandCount(33))
        );
        assert_eq!(
            stream_info_payload(&StreamInfo {
                channels: 0,
                ..info()
            }),
            Err(EncoderError::InvalidChannelCount(0))
        );
        assert_eq!(
            stream_info_payload(&StreamInfo {
                frames_per_block_pwr: 15,
                ..info()
            }),
            Err(EncoderError::InvalidBlockPower(15))
        );
    }

    #[test]
    fn gain_info_is_nine_bytes_of_reference_values() {
        assert_eq!(
            gain_info_payload(&GainInfo::default()),
            [0x01, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn encoder_info_matches_the_reference_bytes() {
        // From container-sine44-q5.txt: a0 01 20 00 (profile 10, v1.32.0).
        let payload = encoder_info_payload(&EncoderInfo {
            profile: 10.0,
            pns: false,
            major: 1,
            minor: 32,
            build: 0,
        })
        .unwrap();
        assert_eq!(payload, [0xa0, 0x01, 0x20, 0x00]);
    }

    #[test]
    fn encoder_info_rejects_out_of_range_profiles() {
        for profile in [-1.0f32, 16.0, f32::NAN] {
            assert!(
                encoder_info_payload(&EncoderInfo {
                    profile,
                    pns: false,
                    major: 1,
                    minor: 0,
                    build: 0,
                })
                .is_err(),
                "profile {profile} should be rejected"
            );
        }
    }

    #[test]
    fn seek_offset_is_five_zero_bytes() {
        assert_eq!(seek_offset_payload(), [0, 0, 0, 0, 0]);
    }

    #[test]
    fn seek_table_rejects_empty_and_mismatched_input() {
        assert_eq!(
            seek_table_payload(0, 0, 1, &[]),
            Err(EncoderError::SeekTableEmpty)
        );
        assert_eq!(
            seek_table_payload(0, 2, 1, &[10]),
            Err(EncoderError::SeekTableLengthMismatch {
                declared: 2,
                entries: 1
            })
        );
    }

    #[test]
    fn seek_table_single_entry_matches_the_reference_bytes() {
        // container-sine44-q5.txt: seek_pwr=1, entries=[45] -> 01 12 d0.
        assert_eq!(
            seek_table_payload(0, 1, 1, &[45]).unwrap(),
            [0x01, 0x12, 0xd0]
        );
    }

    #[test]
    fn stream_writer_assembles_the_header_sequence() {
        let mut w = Sv8StreamWriter::new();
        w.write_magic();
        w.write_stream_info(&info()).unwrap();
        w.write_gain_info(&GainInfo::default()).unwrap();
        w.write_encoder_info(&EncoderInfo {
            profile: 10.0,
            pns: false,
            major: 1,
            minor: 32,
            build: 0,
        })
        .unwrap();
        w.write_seek_offset().unwrap();
        w.write_seek_table(1, 1, &[45]).unwrap();
        w.write_end().unwrap();

        let bytes = w.into_bytes();
        assert_eq!(&bytes[0..4], b"MPCK");
        assert_eq!(&bytes[4..6], b"SH");
        // SO patch: ST offset - SO offset, at the reserved payload.
        let so = 4 + bytes[4..].windows(2).position(|x| x == &b"SO"[..]).unwrap();
        let st = bytes.windows(2).position(|x| x == &b"ST"[..]).unwrap();
        let patch = encode_size((st - so) as u64);
        assert_eq!(&bytes[so + 3..so + 3 + patch.len()], patch.as_bytes());
    }

    #[test]
    fn seek_table_before_placeholder_is_rejected() {
        let mut w = Sv8StreamWriter::new();
        assert_eq!(
            w.write_seek_table(1, 1, &[0]),
            Err(EncoderError::SeekTableNotReserved)
        );
    }
}
