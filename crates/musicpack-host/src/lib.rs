//! `musicpack-host`: the thinnest practical host adapter for the deterministic
//! engine.
//!
//! It is the layer that is allowed to own host concerns: a source backend, a
//! shared (`Rc<RefCell<…>>`) engine handle, the pull loop a device or
//! AudioWorklet callback would drive, and the mapping from the core
//! representation policy to concrete source URLs.
//!
//! ```text
//! host source (bytes / package member / range)      ← SourceBackend
//!        │
//!        ▼
//! musicpack-engine DecoderEngine (decode · ring · resample · mix)
//!        │
//!        ▼  Host::render(frames)  (the device/worklet pull)
//!    host output
//! ```
//!
//! The engine stays device-free: this crate calls `consume()` when the host
//! asks for frames and feeds every engine fact back into the player. Nothing
//! here is in the core or the engine.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod host;
pub mod selection;

pub use host::{Host, HostConfig};
pub use selection::{SelectedSource, SourceCandidate, TrackSources, select_source};

pub use musicpack_engine::{
    DecoderEngine, DecoderEngineConfig, MemorySourceBackend, PackageSourceBackend, SourceBackend,
    SourceError,
};
