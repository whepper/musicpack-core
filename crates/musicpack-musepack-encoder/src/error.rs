//! Errors produced by the SV8 container/bitstream writers.
//!
//! These are ordinary invalid-input and impossible-construction errors; the
//! writers never panic on caller data. Field widths mirror the reference
//! encoder's bit widths (for example `MaxBand - 1` is five bits), so
//! out-of-range values are rejected rather than silently truncated.

use std::fmt;

/// An error from the SV8 container/bitstream layer.
#[derive(Debug, Clone, PartialEq)]
pub enum EncoderError {
    /// A block key must be two ASCII uppercase letters (the decoder rejects
    /// anything else).
    InvalidBlockKey([u8; 2]),
    /// A size could not be represented within the ten-byte SV8 size field.
    SizeOverflow,
    /// `SH` only encodes 44100, 48000, 37800 or 32000 Hz.
    InvalidSampleRate(u32),
    /// `MaxBand` must fit the five-bit `MaxBand - 1` field (`1..=32`).
    InvalidBandCount(u32),
    /// Channels must fit the four-bit `Channels - 1` field (`1..=16`).
    InvalidChannelCount(u32),
    /// `frames_per_block_pwr` must fit the three-bit `>> 1` field (`0..=14`).
    InvalidBlockPower(u32),
    /// The `EI` profile must fit the seven-bit field.
    InvalidProfile(f64),
    /// A seek table cannot be written without any entries.
    SeekTableEmpty,
    /// The declared `seek_pos` and the supplied entry count disagree.
    SeekTableLengthMismatch {
        /// The count encoded in the seek table.
        declared: u32,
        /// The number of entries actually supplied.
        entries: usize,
    },
    /// A seek table was requested before the `SO` placeholder was written.
    SeekTableNotReserved,
    /// The reserved `SO` placeholder cannot hold the seek-table patch.
    SeekPatchOutOfRange,
    /// A PCM block is shorter than the analysis filterbank requires.
    PcmBlockTooShort {
        /// Minimum samples per channel.
        needed: usize,
        /// Samples supplied.
        got: usize,
    },
    /// `max_band` must be in `0..=31`.
    InvalidMaxBand(usize),
    /// The psychoacoustic model only has frozen tables for the oracle's
    /// `(quality, sample rate)` combinations.
    UnsupportedPsyConfig {
        /// Requested quality.
        qual: f32,
        /// Requested sample rate in Hz.
        sample_rate: f32,
    },
}

impl fmt::Display for EncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBlockKey(key) => {
                write!(
                    f,
                    "invalid SV8 block key {:?}",
                    String::from_utf8_lossy(key)
                )
            }
            Self::SizeOverflow => write!(f, "size does not fit the SV8 size field"),
            Self::InvalidSampleRate(rate) => write!(
                f,
                "unsupported sample rate {rate} (expected 44100, 48000, 37800 or 32000)"
            ),
            Self::InvalidBandCount(bands) => {
                write!(
                    f,
                    "band count {bands} does not fit the 5-bit MaxBand-1 field"
                )
            }
            Self::InvalidChannelCount(channels) => write!(
                f,
                "channel count {channels} does not fit the 4-bit Channels-1 field"
            ),
            Self::InvalidBlockPower(power) => write!(
                f,
                "frames_per_block_pwr {power} does not fit the 3-bit block-power field"
            ),
            Self::InvalidProfile(profile) => {
                write!(f, "encoder profile {profile} does not fit the 7-bit field")
            }
            Self::SeekTableEmpty => write!(f, "seek table has no entries"),
            Self::SeekTableLengthMismatch { declared, entries } => write!(
                f,
                "seek table declares {declared} entries but {entries} were supplied"
            ),
            Self::SeekTableNotReserved => {
                write!(f, "seek table written before the SO placeholder")
            }
            Self::SeekPatchOutOfRange => {
                write!(f, "the SO placeholder cannot hold the seek-table patch")
            }
            Self::PcmBlockTooShort { needed, got } => write!(
                f,
                "PCM block too short: analysis needs {needed} samples per channel, got {got}"
            ),
            Self::InvalidMaxBand(band) => {
                write!(f, "max_band {band} is out of range (0..=31)")
            }
            Self::UnsupportedPsyConfig { qual, sample_rate } => write!(
                f,
                "no frozen psychoacoustic tables for quality {qual} at {sample_rate} Hz"
            ),
        }
    }
}

impl std::error::Error for EncoderError {}
