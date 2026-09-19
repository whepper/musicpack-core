//! `musicpack-engine`: the deterministic PCM adapter between `musicpack-core`
//! and a host's audio output.
//!
//! It implements the core's platform-neutral
//! [`musicpack_core::player::engine::Engine`] trait over
//! [`musicpack_core::audio::AudioDecoder`] using only safe, `std`-only,
//! dependency-free code, and compiles natively and for
//! `wasm32-unknown-unknown`.
//!
//! # Layering
//!
//! ```text
//! PlaybackItem → SourceBackend → Box<dyn Read> → AudioDecoder
//!                                                        │
//!                              DecodeSession (decoder + ring + resampler)
//!                                                        │
//!                                     DecoderEngine (Engine trait)
//!                                         │           │
//!                                    consume()     prepare_next / advance
//!                                         │       begin_crossfade (lane + mixer)
//!                                    host output
//! ```
//!
//! The adapter deliberately contains **no** HTTP, OPFS, filesystem, device,
//! browser, timer, thread, or async machinery. A host owns all of those and
//! drives this crate by pumping and consuming frames; the player core stays
//! sample-blind.
//!
//! # Rendered samples
//!
//! [`musicpack_core::player::engine::Engine::rendered_samples`] is "output-rate frames actually consumed since
//! the last open/seek reset" — the ring's read playhead. After a crossfade the
//! promoted session's playhead is rebased to the boundary where mixing began
//! (matching the reference worklet's swap accounting), so the value stays
//! continuous in the player's compressed album clock.
//!
//! # Musepack
//!
//! Musepack SV8 is decoded natively in the core (`musicpack_core::audio::musepack`)
//! behind the same [`DecoderFactory`]; it flows through this session/engine
//! with no adapter-specific branch.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod decoder;
pub mod engine;
pub mod mixer;
pub mod resampler;
pub mod ring;
pub mod session;
pub mod source;

pub use decoder::{DecoderFactory, SniffingDecoderFactory};
pub use engine::{DecoderEngine, DecoderEngineConfig};
pub use mixer::{Mixer, SwapFacts, equal_power_gains, mix_interleaved, rebase_delta};
pub use resampler::StreamingResampler;
pub use ring::RingBuffer;
pub use session::{
    CALLBACK_MARGIN_FRAMES, DECODE_CHUNK_FRAMES, DecodeSession, HIGH_WATER, LOW_WATER,
    PRIME_FRACTION, RING_SECONDS,
};
pub use source::{MemorySourceBackend, PackageSourceBackend, SourceBackend, SourceError};
