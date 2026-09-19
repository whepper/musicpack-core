//! Player domain: the platform-independent player core.
//!
//! Port of the TypeScript package `web/player-core` (BSD-3-Clause) — the
//! behavioural reference. The module is synchronous, deterministic and
//! platform-free: no clocks, no timers, no async runtime, no browser/audio
//! APIs, no PCM. The host owns engines, audio output, persistence, UI and
//! representation selection.
//!
//! # Purity laws (ported from `web/player-core/README.md`)
//!
//! 1. No ambient globals: no wall clock, no randomness, no timers. Randomness
//!    (shuffle) arrives through an injected RNG; time arrives as engine
//!    sample counts and explicit event injections.
//! 2. Port signatures stay plain data (the TS "JSON law"): no closures,
//!    handles or PCM buffers cross the engine boundary.
//! 3. No streaming PCM through the core: engines own audio; the core sees
//!    control flow and coarse metadata only.
//! 4. Single logical thread: commands in, ordered events out; stale injected
//!    facts are rejected by generations/epochs.
//!
//! # Deliberate Phase 10 deviations from TypeScript
//!
//! - Asynchronous engine promises → a synchronous [`engine::Engine`] trait;
//!   engine facts arrive through explicit `Player::on_*` injections, and an
//!   in-flight crossfade completes through `Player::on_crossfade_complete`.
//! - Callback event fan-out → mutating operations return
//!   `Vec<events::PlayerEvent>` in emission order.
//! - Persistence scheduling, Media Session, storage and item construction are
//!   host-only and are not ported.
//!
//! # Numeric constants ported exactly
//!
//! - `END_TOLERANCE_SAMPLES = 256` (output-rate samples)
//! - crossfade presets `{0, 4, 8, 12}` s; engine fade clamp `[0.25, 15]` s
//! - Sweet-Fades planner: silence threshold `0.02`, min trailing silence
//!   `1.2` s, loud fraction `0.25` of median, fade min `1` s, fade base
//!   `6` s, outro grace `0.5` s, prime lead `1` s, buckets `0.1` s
//! - gain: target `-16` LUFS, true-peak cap `-1` dBTP, default volume `0.8`
//! - "previous" restart threshold `3` s

pub mod engine;
pub mod events;
pub mod gain;
pub mod order;
#[allow(clippy::module_inception)]
pub mod player;
pub mod queue;
pub mod snapshot;
pub mod transition;
pub mod types;

pub use engine::{
    CrossfadeResult, CrossfadeStart, Engine, EngineCapabilities, EngineError, EngineResult,
};
pub use events::PlayerEvent;
pub use gain::{
    PLAYBACK_TARGET_LUFS, TRUE_PEAK_CAP_DB, combined_gain, db_to_linear, normalization_gain,
};
pub use order::{Rng, next_index_under_repeat, shuffle_order};
pub use player::{
    END_TOLERANCE_SAMPLES, Player, PlayerModel, PlayerOptions, PlayerPorts, PlayerState,
};
pub use queue::{QueueModel, QueueState};
pub use snapshot::{
    SNAPSHOT_VERSION, SNAPSHOT_VERSION_V1, SessionSnapshot, clamp_index, decode_snapshot,
    encode_snapshot,
};
pub use transition::{
    BoundaryProfile, FADE_BASE_SECONDS, FADE_MIN_SECONDS, LOUD_FRACTION,
    MIN_TRAILING_SILENCE_SECONDS, OUTRO_GRACE_SECONDS, PRIME_LEAD_SECONDS, SILENCE_THRESHOLD,
    TransitionPlan, TransitionQuery, decay_seconds, is_clean_loud_ending, is_fast_attack,
    plan_transition, trailing_silence_seconds,
};
pub use types::{
    AlbumLoudness, EngineKind, NormalizationMode, PlaybackItem, PlaybackSource, RepeatMode,
    SourceKind, StreamInfo, TrackLoudness, item_key, same_item_identity,
};
