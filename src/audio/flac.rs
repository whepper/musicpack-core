//! FLAC decoding through the [`claxon`] crate, behind the core's seam.
//!
//! The reference implementation vendors `dr_flac` (C) and calls it through
//! `musicpack_audio_*`. The Rust core keeps the same layering:
//! `AudioDecoder` ← this adapter ← `claxon`. No `claxon` type appears in the
//! public API, so the decoder can be swapped without touching consumers.
//!
//! See `docs/architecture.md` §6 for the dependency assessment (Apache-2.0,
//! zero runtime dependencies, wasm32-clean) and the audit of `claxon`'s five
//! internal `unsafe` blocks; the core's own `#![forbid(unsafe_code)]` applies
//! to this crate, and no unsafe enters it.
//!
//! ## Representation
//!
//! `claxon` returns decoded samples in native bit depth. This adapter
//! exposes them exactly as the reference does:
//!
//! - `read_s32` = `sample << (32 - bits_per_sample)` (left-aligned).
//! - `read_f32` = `(sample as f32) * 2^-(bits_per_sample-1)`, which is the
//!   exact value of the reference's `dr_flac` conversion for every bit
//!   depth (it normalizes through a 24-bit container for ≤ 24-bit content
//!   and through `2^31` for 32-bit content).
//!
//! Differences from `dr_flac` that are metadata-level, not PCM-level, are
//! documented in `docs/architecture.md`.
//!
//! Calls into the dependency are wrapped in [`std::panic::catch_unwind`] so a
//! decoder panic on malformed input becomes [`Error::Invalid`] instead of
//! unwinding through the core. This holds for unwind builds (the default,
//! and what tests/fuzzing use); a build compiled with `panic = "abort"`
//! cannot intercept a panic by construction.

use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{AudioDecoder, AudioInfo, Codec, int_pcm_to_f32, invalid};
use crate::{Error, Result};

/// A FLAC decoding session.
pub struct FlacDecoder {
    reader: claxon::FlacReader<Box<dyn Read>>,
    info: AudioInfo,
    /// Interleaved native samples of the current decoded block.
    pending: Vec<i32>,
    /// Index of the next frame in [`Self::pending`].
    pending_frame: usize,
    /// Reusable block buffer (claxon's channel-consecutive layout).
    block: Vec<i32>,
    /// Set once EOF or a terminal error has been observed.
    finished: bool,
}

impl FlacDecoder {
    /// Opens a FLAC stream. The source must be positioned at the `fLaC`
    /// magic.
    ///
    /// Malformed streams are [`Error::Invalid`]; source failures are
    /// [`Error::Io`]. A panic inside the third-party decoder (which should
    /// not happen for a well-behaved decoder, but is not something the core
    /// can rule out for arbitrary bytes) is caught and reported as
    /// [`Error::Invalid`] so the no-panic contract of the core holds for
    /// untrusted input.
    pub fn new(source: Box<dyn Read>) -> Result<Self> {
        // `read_vorbis_comment: false` avoids retaining tag memory; it does
        // not change the decoded PCM.
        let options = claxon::FlacReaderOptions {
            metadata_only: false,
            read_vorbis_comment: false,
        };
        let reader = catch_unwind(AssertUnwindSafe(|| {
            claxon::FlacReader::new_ext(source, options)
        }))
        .map_err(|_| invalid("FLAC decoder panicked while reading the stream header"))?
        .map_err(map_claxon)?;

        let si = reader.streaminfo();
        if si.channels < 1
            || si.channels > super::MAX_CHANNELS as u32
            || si.sample_rate == 0
            || si.bits_per_sample == 0
            || si.bits_per_sample > 32
        {
            return Err(invalid("FLAC stream has an unsupported format"));
        }

        Ok(Self {
            reader,
            info: AudioInfo {
                sample_rate: si.sample_rate,
                channels: si.channels as u8,
                bits_per_sample: si.bits_per_sample as u8,
                total_frames: si.samples,
                codec: Codec::Flac,
                is_float: false,
            },
            pending: Vec::new(),
            pending_frame: 0,
            block: Vec::new(),
            finished: false,
        })
    }

    /// Number of frames currently buffered from the last decoded block.
    #[inline]
    fn pending_frames(&self) -> usize {
        self.pending.len() / self.info.channels as usize
    }

    /// Decodes the next block into [`Self::pending`].
    ///
    /// Returns `Ok(false)` at clean EOF. Decoding exactly one block per
    /// `FrameReader` keeps the adapter non-self-referential: the reader
    /// borrows `self.reader` only for the duration of the call, and the
    /// block owns its sample buffer, which is recycled through
    /// [`claxon::Block::into_buffer`].
    fn refill(&mut self) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        let buffer = std::mem::take(&mut self.block);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut frames = self.reader.blocks();
            frames.read_next_or_eof(buffer)
        }));
        let block = match result {
            Ok(Ok(Some(block))) => block,
            Ok(Ok(None)) => {
                self.finished = true;
                return Ok(false);
            }
            Ok(Err(e)) => {
                self.finished = true;
                return Err(map_claxon(e));
            }
            Err(_) => {
                self.finished = true;
                return Err(invalid("FLAC decoder panicked on malformed input"));
            }
        };

        let channels = block.channels() as usize;
        let frames = block.duration() as usize;
        self.pending.clear();
        self.pending.reserve(frames * channels);
        for frame in 0..frames {
            for channel in 0..channels {
                self.pending.push(block.channel(channel as u32)[frame]);
            }
        }
        self.block = block.into_buffer();
        self.pending_frame = 0;
        // A well-formed frame always has at least one sample; treat an empty
        // one as EOF so a hostile stream cannot spin the public read loop.
        if self.pending.is_empty() {
            self.finished = true;
            return Ok(false);
        }
        Ok(true)
    }

    /// Copies and converts up to `requested` frames from the block buffer.
    ///
    /// `map` is applied per interleaved sample; it is monomorphized at the
    /// two public call sites, so no intermediate buffer is allocated.
    #[inline]
    fn read_mapped<T>(
        &mut self,
        out: &mut [T],
        requested: usize,
        map: impl Fn(i32) -> T,
    ) -> Result<usize> {
        let channels = self.info.channels as usize;
        let mut produced = 0;
        while produced < requested {
            if self.pending_frame >= self.pending_frames() {
                if !self.refill()? {
                    break;
                }
                continue;
            }
            let available = self.pending_frames() - self.pending_frame;
            let take = available.min(requested - produced);
            for frame in 0..take {
                let src = (self.pending_frame + frame) * channels;
                let dst = (produced + frame) * channels;
                for channel in 0..channels {
                    out[dst + channel] = map(self.pending[src + channel]);
                }
            }
            self.pending_frame += take;
            produced += take;
        }
        Ok(produced)
    }
}

impl AudioDecoder for FlacDecoder {
    fn info(&self) -> &AudioInfo {
        &self.info
    }

    fn read_f32(&mut self, interleaved: &mut [f32]) -> Result<usize> {
        let channels = self.info.channels as usize;
        if interleaved.len() % channels != 0 {
            return Err(invalid("destination length is not a multiple of channels"));
        }
        let requested = interleaved.len() / channels;
        let bits = self.info.bits_per_sample;
        self.read_mapped(interleaved, requested, |sample| {
            int_pcm_to_f32(sample, bits)
        })
    }

    fn read_s32(&mut self, interleaved: &mut [i32]) -> Result<usize> {
        let channels = self.info.channels as usize;
        if interleaved.len() % channels != 0 {
            return Err(invalid("destination length is not a multiple of channels"));
        }
        let requested = interleaved.len() / channels;
        let shift = 32 - self.info.bits_per_sample as u32;
        self.read_mapped(interleaved, requested, |sample| sample << shift)
    }
}

/// Maps a `claxon` error onto the core's taxonomy.
///
/// I/O failures become [`Error::Io`]; malformed/unsupported stream content
/// becomes [`Error::Invalid`], matching the reference's `MUSICPACK_ERR_INVALID`
/// for bad FLAC input.
fn map_claxon(error: claxon::Error) -> Error {
    match error {
        claxon::Error::IoError(e) => Error::Io {
            detail: e.to_string(),
        },
        other => invalid(&format!("FLAC stream error: {other}")),
    }
}

/// Reads a FLAC stream's Vorbis comments **without decoding any audio**.
///
/// This is the tag seam for fresh-album discovery (`musicpack-author`): the
/// stream is opened, every metadata block is walked exactly like
/// [`FlacDecoder::new`] does (so a malformed comment/application block
/// fails here the same way it would at decode time), and the comments are
/// returned as `(key, value)` pairs in file order with the keys' original
/// casing. An empty `Vec` means the stream carried no `VORBIS_COMMENT`
/// block. No panic may escape — the same `catch_unwind` contract as
/// [`FlacDecoder::new`] holds for untrusted input.
pub fn read_vorbis_comments(source: Box<dyn Read>) -> Result<Vec<(String, String)>> {
    // `read_vorbis_comment: true` retains the parsed comments; decoding
    // stays lazy, so no frame data is read.
    let options = claxon::FlacReaderOptions {
        metadata_only: false,
        read_vorbis_comment: true,
    };
    let reader = catch_unwind(AssertUnwindSafe(|| {
        claxon::FlacReader::new_ext(source, options)
    }))
    .map_err(|_| invalid("FLAC decoder panicked while reading the stream header"))?
    .map_err(map_claxon)?;
    Ok(reader
        .tags()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}
