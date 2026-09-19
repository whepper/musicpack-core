//! The platform-neutral playback engine seam.
//!
//! Port of `web/player-core/src/engine.ts` (BSD-3-Clause), reshaped from an
//! asynchronous callback port into a **synchronous** trait (Phase 10
//! deliberate deviation — the core has no async runtime). A host adapter that
//! wraps genuinely asynchronous platform engines drives this trait from its
//! own event loop and injects engine facts through the player's `on_*`
//! methods.
//!
//! Capability concepts from the TypeScript port are preserved:
//!
//! - `preload_next` gates the standby (`prepare_next`/`advance`) path;
//! - `crossfade` gates `begin_crossfade`;
//! - `decode_gate` is informational; the pump calls (`start_pumping` /
//!   `pause_pumping`) are always available and default to no-ops, exactly
//!   like the TypeScript `gate()` fallback.
//!
//! All positions are OUTPUT-rate samples within the currently open track.

use super::types::{PlaybackItem, StreamInfo};

/// Engine capability flags (`EngineCapabilities`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EngineCapabilities {
    /// Standby `prepare_next`/`advance` support.
    pub preload_next: bool,
    /// Track handoff is sample-exact.
    pub sample_accurate_gapless: bool,
    /// Decode-gate control is meaningful (pull-style pipelines).
    pub decode_gate: bool,
    /// Overlapped `begin_crossfade` transitions are supported.
    pub crossfade: bool,
}

/// Result of a taken crossfade transition (`CrossfadeResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossfadeResult {
    /// The incoming track's stream facts.
    pub info: StreamInfo,
    /// Output-rate frames of overlap actually mixed.
    pub overlap_frames: u64,
}

/// Outcome of a `begin_crossfade` request.
///
/// TypeScript's `beginCrossfade` returns a `Promise` that may resolve
/// immediately or later. The synchronous seam represents that as three
/// cases: declined, completed now, or in flight (the host later calls
/// `Player::on_crossfade_complete`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CrossfadeStart {
    /// No fade: fall back to the normal boundary path.
    Declined,
    /// The fade completed synchronously.
    Completed(CrossfadeResult),
    /// The fade is in flight; the host will report completion.
    Pending,
}

/// An engine failure message (the TypeScript `error(message)` payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

/// Engine operation result.
pub type EngineResult<T> = Result<T, EngineError>;

/// A host-implementable abstraction over "the thing that turns one or more
/// queued sources into audible, seekable output".
pub trait Engine {
    /// Declared capabilities (honest; nothing claims more than it does).
    fn capabilities(&self) -> EngineCapabilities;

    /// Opens a source; returns its stream facts.
    fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo>;

    /// Starts/resumes output.
    fn play(&mut self) -> EngineResult<()>;

    /// Pauses output.
    fn pause(&mut self);

    /// Seeks within the OPEN track (output-rate samples).
    fn seek(&mut self, samples: u64);

    /// Sets the combined linear gain.
    fn set_gain(&mut self, linear: f64);

    /// Output-rate frames rendered since the last open/seek reset.
    fn rendered_samples(&self) -> u64;

    /// Releases the engine's resources.
    fn close(&mut self);

    /// Opens the next source ahead of time (standby). Default: unsupported.
    fn prepare_next(&mut self, _item: &PlaybackItem) -> Option<StreamInfo> {
        None
    }

    /// Promotes a previously prepared standby. `expected` is the current
    /// policy target; a standby prepared for anything else must be refused
    /// (`None`) without promoting. Default: unsupported.
    fn advance(&mut self, _expected: Option<&PlaybackItem>) -> Option<StreamInfo> {
        None
    }

    /// Begins an overlapped transition. Default: unsupported.
    fn begin_crossfade(&mut self, _next: &PlaybackItem, _fade_seconds: f64) -> CrossfadeStart {
        CrossfadeStart::Declined
    }

    /// True when the decoder is exhausted and the output has run dry.
    fn is_output_drained(&self) -> bool {
        false
    }

    /// Starts the decode pump (no-op when the engine has no gate).
    fn start_pumping(&mut self) {}

    /// Stops the decode pump (no-op when the engine has no gate).
    fn pause_pumping(&mut self) {}
}
