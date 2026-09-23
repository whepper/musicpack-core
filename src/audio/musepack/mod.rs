//! Musepack (SV8) decoder seam.
//!
//! [`sv8`] parses the container/stream info; [`decoder`] implements the audio
//! path (bitstream, requantisation, synthesis filterbank) as a port of the
//! project's vendored `libmpcdec`. [`MusepackDecoder`] exposes both through the
//! standard [`AudioDecoder`] contract, so Musepack behaves like WAV/FLAC to the
//! engine, host and WASM layers.
//!
//! # PCM
//!
//! Musepack is a codec-native **float** format (`bits_per_sample == 0`,
//! `is_float == true`), like IEEE-float WAV: [`AudioDecoder::read_f32`]
//! produces samples and [`AudioDecoder::read_s32`] returns
//! [`Error::Unsupported`] (there is no integer representation).
//!
//! # Gapless and length
//!
//! The decoder emits exactly `samples - beg_silence` frames (the reference's
//! `mpc_streaminfo_get_length_samples`), hiding the encoder delay and leading
//! silence. The engine/player never see Musepack-specific rules.
//!
//! # Memory
//!
//! The decoder consumes the compressed stream **incrementally** from its
//! `Read`: it buffers at most one SV8 block payload (`MAX_BLOCK_BYTES`, the
//! reference demux bound) plus a tiny read scratch, independent of the member
//! size. PCM memory is bounded by the caller's read buffer.

pub mod apev2;
pub mod decoder;
pub mod sv8;
pub(crate) mod tables;

use std::io::Read;

use super::{AudioDecoder, AudioInfo, Codec};
use crate::{Error, Result};

pub use decoder::MpcDecoder;
pub use sv8::{MusepackError, MusepackInfo};

/// A Musepack decoder session.
pub struct MusepackDecoder {
    info: AudioInfo,
    stream: MusepackInfo,
    decoder: MpcDecoder,
}

impl MusepackDecoder {
    /// Opens a Musepack SV8 stream and prepares the decoder.
    ///
    /// Reads only the header blocks; audio blocks are streamed on demand.
    pub fn new(source: Box<dyn Read>) -> Result<Self> {
        let decoder = MpcDecoder::from_reader(source).map_err(|e| super::invalid(&e.0))?;
        let stream = decoder.info().clone();
        let info = AudioInfo {
            sample_rate: stream.sample_rate,
            channels: stream.channels,
            // Codec-native float: bits_per_sample 0 is the reserved marker.
            bits_per_sample: 0,
            total_frames: Some(stream.length_samples()),
            codec: Codec::Musepack,
            is_float: true,
        };
        Ok(Self {
            info,
            stream,
            decoder,
        })
    }

    /// The parsed SV8 stream facts.
    pub fn stream_info(&self) -> &MusepackInfo {
        &self.stream
    }
}

impl AudioDecoder for MusepackDecoder {
    fn info(&self) -> &AudioInfo {
        &self.info
    }

    fn read_f32(&mut self, interleaved: &mut [f32]) -> Result<usize> {
        self.decoder
            .read_f32(interleaved)
            .map_err(|e| super::invalid(&e.0))
    }

    fn read_s32(&mut self, _interleaved: &mut [i32]) -> Result<usize> {
        Err(Error::Unsupported {
            what: "Musepack is float-only (no integer PCM representation)".into(),
        })
    }
}
