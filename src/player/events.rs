//! Player events.
//!
//! Port of `web/player-core/src/events.ts` (BSD-3-Clause). Deliberate Phase 10
//! deviation: events are **returned** (ordered, synchronous with the state
//! change) from each mutating operation, instead of being fanned out through
//! a subscription sink. Ordering is part of the observable contract; the core
//! never drops or coalesces events (position events are emitted per tick, as
//! in TypeScript).

use super::player::PlayerState;
use super::types::PlaybackItem;

/// The typed event union (`PlayerEvent`).
///
/// `Track` carries an owned item, which makes that variant larger than the
/// rest; the event is a transient value, so this is accepted rather than
/// boxing (which would complicate every consumer).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum PlayerEvent {
    /// Transport state changed.
    State {
        /// The new state.
        state: PlayerState,
    },
    /// The current track changed (`None` on teardown).
    Track {
        /// The new current item.
        item: Option<PlaybackItem>,
    },
    /// Coarse position change (track-relative seconds, per Media Session).
    Position {
        /// Position within the current track.
        position_seconds: f64,
        /// Album-absolute start of the current track.
        track_start_seconds: f64,
        /// Current track duration.
        track_duration_seconds: f64,
    },
    /// Repeat/shuffle policy changed.
    Policy {
        /// Active repeat mode.
        repeat: super::types::RepeatMode,
        /// Shuffle enabled.
        shuffle: bool,
    },
    /// Crossfade setting changed.
    Crossfade {
        /// Crossfade length in seconds (0 = off).
        seconds: f64,
    },
    /// Normalization gain changed.
    Gain {
        /// Normalization gain in dB.
        norm_db: f64,
    },
    /// An error occurred.
    Error {
        /// The error message.
        message: String,
    },
    /// Diagnostic: a post-crossfade boundary check disagreed with the
    /// position-based view. Fires at most once per boundary.
    BoundaryDrift {
        /// Index the boundary declared.
        expected_index: i64,
        /// Index the position describes.
        observed_index: i64,
        /// Album-absolute position in samples.
        position_samples: u64,
    },
}
