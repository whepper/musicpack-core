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
//!
//! Besides decoding, this module hosts the read-only metadata seams that
//! fresh-album discovery and the authoring pipeline share —
//! [`read_vorbis_comments`] (tags) and [`read_pictures`] (embedded
//! artwork) — neither of which decodes any audio.

use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{AudioDecoder, AudioInfo, Codec, int_pcm_to_f32, invalid, read_full};
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

// ---------------------------------------------------------------------
// PICTURE metadata blocks (embedded artwork)
// ---------------------------------------------------------------------

/// Maximum accepted size of one embedded picture payload, in bytes — the
/// reference's `MUSICPACK_PICTURE_MAX` (32 MiB). A FLAC metadata block
/// length is a 24-bit field, so a well-formed block cannot reach this;
/// the bound is defence in depth against a lying `data length` field.
pub const MAX_PICTURE_BYTES: u64 = 32 * 1024 * 1024;
/// Bound on walked metadata blocks (the reference's `FLAC_BLOCK_MAX`).
const MAX_METADATA_BLOCKS: u32 = 4096;
/// Bound on a PICTURE block's MIME-type field (the reference's `mlen > 128`).
const MAX_MIME_BYTES: usize = 128;
/// Bound on a PICTURE block's description field (the reference's
/// `dlen > 4096`).
const MAX_DESCRIPTION_BYTES: usize = 4096;

/// A FLAC `PICTURE` metadata block with its image payload preserved
/// byte-for-byte (never decoded, transcoded or re-encoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedPicture {
    /// The FLAC picture type field (3 = front cover, 4 = back cover,
    /// 7 = leaflet page, 8 = media, …; anything else has no MusicPack
    /// role of its own).
    pub picture_type: u32,
    /// The block's declared MIME type (ASCII per spec, accepted as UTF-8;
    /// may be empty). A hint only — consumers validate signatures.
    pub mime: String,
    /// The original encoded image bytes.
    pub data: Vec<u8>,
}

/// Reads every `PICTURE` metadata block of a FLAC stream **without
/// decoding any audio**, preserving each image's bytes exactly.
///
/// This is the embedded-artwork seam for fresh-album discovery and the
/// authoring pipeline (`musicpack-author`). `claxon` skips PICTURE
/// blocks (they are not part of its public metadata surface), so the
/// blocks are walked here directly, with the same structural rules as
/// [`FlacDecoder::new`] / [`read_vorbis_comments`]: `fLaC` magic,
/// STREAMINFO first and only once, block type 127 rejected, block count
/// bounded. Every PICTURE payload is parsed with the reference's bounds
/// (reference `picture_parse`: MIME ≤ 128 bytes, description ≤ 4096
/// bytes, data ≤ [`MAX_PICTURE_BYTES`], all fields within the block).
/// Truncated or out-of-bounds metadata is [`Error::Invalid`]; no
/// third-party code is involved, so no panic can escape.
///
/// The description field is bounds-checked and discarded — MusicPack
/// never consumes it, so a real-world non-UTF-8 description cannot fail
/// an otherwise valid picture. The returned `data` is the original
/// payload; callers decide whether it is usable (the authoring layer
/// validates JPEG/PNG signatures before the bytes become artwork).
pub fn read_pictures(source: Box<dyn Read>) -> Result<Vec<EmbeddedPicture>> {
    let mut source = source;

    let mut magic = [0u8; 4];
    read_exact_or_invalid(source.as_mut(), &mut magic, "FLAC signature")?;
    if &magic != b"fLaC" {
        return Err(invalid("FLAC stream has no fLaC signature"));
    }

    let mut pictures = Vec::new();
    let mut blocks: u32 = 0;
    let mut saw_streaminfo = false;
    loop {
        let mut header = [0u8; 4];
        read_exact_or_invalid(source.as_mut(), &mut header, "FLAC metadata block header")?;
        blocks += 1;
        if blocks > MAX_METADATA_BLOCKS {
            return Err(invalid("FLAC stream has too many metadata blocks"));
        }
        let is_last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        // Same structural rules as the claxon walk (type 127 is reserved
        // to reject frame-sync confusion; STREAMINFO must come first and
        // only once).
        if block_type == 127 {
            return Err(invalid("FLAC metadata block type 127 is invalid"));
        }
        if !saw_streaminfo {
            if block_type != 0 {
                return Err(invalid("FLAC stream does not start with STREAMINFO"));
            }
            saw_streaminfo = true;
        } else if block_type == 0 {
            return Err(invalid("FLAC stream has a second STREAMINFO block"));
        }

        if block_type == 6 {
            let mut payload = vec![0u8; length as usize];
            read_exact_or_invalid(source.as_mut(), &mut payload, "FLAC PICTURE block")?;
            pictures.push(parse_picture(&payload)?);
        } else {
            skip_exact(source.as_mut(), length, "FLAC metadata block")?;
        }
        if is_last {
            break;
        }
    }
    Ok(pictures)
}

/// Parses one PICTURE block payload with checked, bounded reads; every
/// failure is [`Error::Invalid`] and the caller discards the payload.
fn parse_picture(payload: &[u8]) -> Result<EmbeddedPicture> {
    let mut at = 0usize;

    let picture_type = take_be32(payload, &mut at)?;

    let mime_len = take_be32(payload, &mut at)? as usize;
    if mime_len > MAX_MIME_BYTES {
        return Err(invalid("FLAC PICTURE MIME type is too long"));
    }
    let mime = std::str::from_utf8(take(payload, &mut at, mime_len)?)
        .map_err(|_| invalid("FLAC PICTURE MIME type is not valid UTF-8"))?
        .to_string();

    let description_len = take_be32(payload, &mut at)? as usize;
    if description_len > MAX_DESCRIPTION_BYTES {
        return Err(invalid("FLAC PICTURE description is too long"));
    }
    // Bounds-checked, then discarded: never consumed by MusicPack.
    let _description = take(payload, &mut at, description_len)?;

    // width, height, depth, colors (pixel facts MusicPack never consumes).
    take(payload, &mut at, 16)?;

    let data_len = take_be32(payload, &mut at)?;
    if u64::from(data_len) > MAX_PICTURE_BYTES {
        return Err(invalid("FLAC PICTURE data exceeds the picture size bound"));
    }
    let data = take(payload, &mut at, data_len as usize)?.to_vec();

    Ok(EmbeddedPicture {
        picture_type,
        mime,
        data,
    })
}

/// Reads one big-endian `u32`, failing closed on a short payload.
fn take_be32(payload: &[u8], at: &mut usize) -> Result<u32> {
    let bytes = take(payload, at, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Advances `at` by `n` bytes and returns the slice, using checked
/// arithmetic; a short payload is [`Error::Invalid`], never a panic or
/// an out-of-bounds read.
fn take<'a>(payload: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8]> {
    let end = at
        .checked_add(n)
        .ok_or_else(|| invalid("FLAC PICTURE length overflows"))?;
    let bytes = payload
        .get(*at..end)
        .ok_or_else(|| invalid("FLAC PICTURE metadata is truncated"))?;
    *at = end;
    Ok(bytes)
}

/// Reads `buf` exactly; a short stream is truncated metadata
/// ([`Error::Invalid`]), while a real source failure stays
/// [`Error::Io`] through [`read_full`].
fn read_exact_or_invalid(source: &mut dyn Read, buf: &mut [u8], what: &str) -> Result<()> {
    match read_full(source, buf)? {
        n if n == buf.len() => Ok(()),
        _ => Err(invalid(&format!("{what} is truncated"))),
    }
}

/// Reads and discards `remaining` bytes in bounded chunks (portable
/// skip for non-seekable sources), failing closed on truncation.
fn skip_exact(source: &mut dyn Read, mut remaining: u32, what: &str) -> Result<()> {
    let mut buffer = [0u8; 4096];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len() as u32) as usize;
        read_exact_or_invalid(source, &mut buffer[..chunk], what)?;
        remaining -= chunk as u32;
    }
    Ok(())
}
