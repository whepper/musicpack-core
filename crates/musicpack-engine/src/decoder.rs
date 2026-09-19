//! Decoder construction.
//!
//! The engine owns decoders; the host injects a [`DecoderFactory`]. The
//! default factory resolves the item's source through a [`SourceBackend`] and
//! reuses the core's content-sniffing [`musicpack_core::audio::open`] — no
//! codec detection is duplicated here, and no codec-specific type escapes.
//!
//! Musepack (a future `AudioDecoder`) plugs in here without touching the
//! engine, the player, or any consumer.

use musicpack_core::audio::{self, AudioDecoder};
use musicpack_core::player::engine::EngineError;
use musicpack_core::player::types::PlaybackItem;

use crate::source::SourceBackend;

/// Turns a queue item into a decoder.
pub trait DecoderFactory {
    /// Opens a decoder for `item`.
    fn open_decoder(&self, item: &PlaybackItem) -> Result<Box<dyn AudioDecoder>, EngineError>;
}

/// The default factory: source backend + content sniffing (WAV/FLAC today).
pub struct SniffingDecoderFactory<S: SourceBackend> {
    source: S,
}

impl<S: SourceBackend> SniffingDecoderFactory<S> {
    /// Wraps a source backend.
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: SourceBackend> DecoderFactory for SniffingDecoderFactory<S> {
    fn open_decoder(&self, item: &PlaybackItem) -> Result<Box<dyn AudioDecoder>, EngineError> {
        let reader = self
            .source
            .open_source(&item.source)
            .map_err(|e| EngineError(e.0))?;
        audio::open(reader).map_err(|e| EngineError(e.to_string()))
    }
}
