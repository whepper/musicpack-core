//! The platform-independent player orchestrator.
//!
//! Port of `web/player-core/src/player.ts` (BSD-3-Clause). Owns the transport
//! state machine, queue orchestration, engine lifecycle, album-absolute
//! position bookkeeping, normalization application, and the crossfade/EOS
//! boundary logic — all over injected ports.
//!
//! # Deliberate Phase 10 deviations from TypeScript
//!
//! - The engine seam is synchronous; the asynchronous `await` gaps are
//!   replaced by explicit event injections (`on_primed`, `on_eos`, …) and by
//!   [`Player::on_crossfade_complete`] for an in-flight fade.
//! - Mutating operations **return** `Vec<PlayerEvent>` instead of invoking a
//!   subscription sink; ordering is the observable contract.
//! - Persistence scheduling (`persist`/`persistThrottled`) and Media Session
//!   binding are host concerns and are not ported; [`Player::take_snapshot`]
//!   exposes the snapshot the host persists.
//! - `play_sequence` with an empty item list is a silent no-op (the host
//!   builds non-empty sequences); the TypeScript implementation throws.

use std::collections::HashMap;

use super::engine::{CrossfadeResult, CrossfadeStart, Engine, EngineError};
use super::events::PlayerEvent;
use super::gain::{combined_gain, normalization_gain};
use super::queue::QueueModel;
use super::snapshot::SessionSnapshot;
use super::transition::{PRIME_LEAD_SECONDS, TransitionPlan, TransitionQuery};
use super::types::{
    EngineKind, NormalizationMode, PlaybackItem, RepeatMode, StreamInfo, same_item_identity,
};

/// End-of-album tolerance (samples).
pub const END_TOLERANCE_SAMPLES: u64 = 256;

/// Transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerState {
    /// No session.
    Idle,
    /// A load is in progress.
    Loading,
    /// Output is not yet running.
    Buffering,
    /// Audible.
    Playing,
    /// Paused by the user.
    Paused,
    /// Reached the end of the queue.
    Ended,
    /// Failed.
    Error,
}

/// The observable player model (`PlayerModel`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerModel {
    /// Transport state.
    pub state: PlayerState,
    /// The current item.
    pub current: Option<PlaybackItem>,
    /// Album-absolute position in seconds.
    pub position_seconds: f64,
    /// Album duration in seconds.
    pub duration_seconds: f64,
    /// Album-absolute start of the current track.
    pub current_track_start_seconds: f64,
    /// Current track duration in seconds (0 when unknown).
    pub current_track_duration_seconds: f64,
    /// User volume.
    pub volume: f64,
    /// Normalization mode.
    pub normalize_mode: NormalizationMode,
    /// Last applied normalization gain (dB).
    pub norm_db: f64,
    /// Repeat policy (mirrored from the queue).
    pub repeat: RepeatMode,
    /// Shuffle flag (mirrored from the queue).
    pub shuffle: bool,
    /// Crossfade length in seconds (0 = off).
    pub crossfade_seconds: f64,
    /// Last error message.
    pub error: Option<String>,
}

/// Construction options.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerOptions {
    /// Initial volume (default 0.8).
    pub initial_volume: Option<f64>,
    /// Initial normalization mode (default `Album`).
    pub initial_normalize: Option<NormalizationMode>,
}

/// Builds an engine for a resolved kind.
pub type EngineFactory = dyn FnMut(EngineKind) -> Box<dyn Engine>;
/// Resolves which engine kind an item needs.
pub type ResolveKind = dyn Fn(&PlaybackItem) -> Result<EngineKind, String>;
/// Content-aware transition policy (Sweet Fades).
pub type TransitionPlanner = dyn Fn(&TransitionQuery) -> TransitionPlan;

/// Host-provided dependencies (architectural inversion preserved from the
/// TypeScript `PlayerPorts`, without async/callback machinery).
pub struct PlayerPorts {
    /// Builds an engine for a resolved kind.
    pub engine_factory: Box<EngineFactory>,
    /// Resolves which engine kind an item needs (host policy; `Err` fails
    /// the load like the TypeScript `resolveKind` throw).
    pub resolve_kind: Box<ResolveKind>,
    /// Optional content-aware transition policy (Sweet Fades).
    pub plan_transition: Option<Box<TransitionPlanner>>,
}

#[derive(Debug, Clone, Copy)]
struct BoundaryCheck {
    idx: i64,
    seq: u64,
}

#[derive(Debug, Clone)]
struct EosRace {
    outgoing_track_id: i64,
    raw_len: u64,
    remaining_at_eos: u64,
}

#[derive(Debug, Clone)]
struct XfadeInFlight {
    seq: u64,
    epoch: u64,
    target: PlaybackItem,
    prev_item: Option<PlaybackItem>,
    prev_len: Option<u64>,
    eos_race: Option<EosRace>,
}

/// The player.
pub struct Player {
    queue: QueueModel,
    ports: PlayerPorts,
    model: PlayerModel,
    engine: Option<Box<dyn Engine>>,
    engine_kind: Option<EngineKind>,
    lengths: HashMap<i64, u64>,
    reset_offset: u64,
    pending_ended: bool,
    eos_in_flight: bool,
    pending_boundary_check: Option<BoundaryCheck>,
    loading_seq: u64,
    transport_seq: u64,
    engine_epoch: u64,
    mutating: bool,
    pause_intent: bool,
    restored_within: Option<(i64, u64)>,
    crossfade_seconds: f64,
    crossfade_armed: bool,
    crossfade_in_progress: bool,
    xfade: Option<XfadeInFlight>,
    output_rate: u32,
    events: Vec<PlayerEvent>,
}

impl Player {
    /// Creates a player over a queue and injected ports.
    pub fn new(queue: QueueModel, ports: PlayerPorts, options: PlayerOptions) -> Self {
        let model = PlayerModel {
            state: PlayerState::Idle,
            current: None,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            current_track_start_seconds: 0.0,
            current_track_duration_seconds: 0.0,
            volume: options.initial_volume.unwrap_or(0.8),
            normalize_mode: options
                .initial_normalize
                .unwrap_or(NormalizationMode::Album),
            norm_db: 0.0,
            repeat: RepeatMode::Off,
            shuffle: false,
            crossfade_seconds: 0.0,
            error: None,
        };
        Self {
            queue,
            ports,
            model,
            engine: None,
            engine_kind: None,
            lengths: HashMap::new(),
            reset_offset: 0,
            pending_ended: false,
            eos_in_flight: false,
            pending_boundary_check: None,
            loading_seq: 0,
            transport_seq: 0,
            engine_epoch: 0,
            mutating: false,
            pause_intent: false,
            restored_within: None,
            crossfade_seconds: 0.0,
            crossfade_armed: false,
            crossfade_in_progress: false,
            xfade: None,
            output_rate: 44100,
            events: Vec::new(),
        }
    }

    /// The observable model.
    pub fn model(&self) -> &PlayerModel {
        &self.model
    }

    /// A cloned model snapshot.
    pub fn state(&self) -> PlayerModel {
        self.model.clone()
    }

    /// The queue.
    pub fn queue(&self) -> &QueueModel {
        &self.queue
    }

    /// Mutable queue access (host queue-panel mutation path).
    pub fn queue_mut(&mut self) -> &mut QueueModel {
        &mut self.queue
    }

    /// The active engine kind, if any.
    pub fn engine_kind(&self) -> Option<EngineKind> {
        self.engine_kind
    }

    // ---- cross-reload persistence ---------------------------------------

    /// Builds the snapshot the host should persist, or `None` when there is
    /// nothing to save.
    pub fn take_snapshot(&self) -> Option<SessionSnapshot> {
        if self.model.current.is_none() || self.queue.items().is_empty() {
            return None;
        }
        Some(SessionSnapshot {
            v: super::snapshot::SNAPSHOT_VERSION,
            items: self.queue.items().iter().map(|i| i.to_value()).collect(),
            index: self.queue.index(),
            position_seconds: self.model.position_seconds,
            volume: Some(self.model.volume),
            normalize_mode: Some(self.model.normalize_mode),
            repeat: self.queue.repeat(),
            shuffle: self.queue.shuffling(),
            crossfade_seconds: self.crossfade_seconds,
        })
    }

    /// Rebuilds a paused session from a decoded snapshot.
    ///
    /// Never starts playback: the restored state is `paused`; the host
    /// resumes through the normal `toggle_play`/`resume` lifecycle.
    pub fn restore(&mut self, snapshot: &SessionSnapshot) -> Vec<PlayerEvent> {
        self.events.clear();
        self.queue.set_repeat(snapshot.repeat);
        if snapshot.crossfade_seconds > 0.0
            && [4.0, 8.0, 12.0].contains(&snapshot.crossfade_seconds)
        {
            self.crossfade_seconds = snapshot.crossfade_seconds;
        }
        let items: Vec<PlaybackItem> = snapshot
            .items
            .iter()
            .filter_map(PlaybackItem::from_value)
            .collect();
        if items.is_empty() {
            return self.take_events();
        }
        let index = super::snapshot::clamp_index(snapshot.index as f64, items.len());
        self.mutating = true;
        self.queue.set_all(items, index);
        self.mutating = false;
        if snapshot.shuffle {
            self.queue.set_shuffle(true);
        }
        let Some(item) = self.queue.current().cloned() else {
            return self.take_events();
        };

        let rate = self.rate();
        let offs = self.offsets();
        let start_samples = offs.get(index as usize).copied().unwrap_or(0);
        let within_samples = (snapshot.position_seconds * rate as f64)
            .floor()
            .min(self.length_of(index).saturating_sub(1) as f64);
        let within_samples = if within_samples.is_finite() {
            within_samples as u64
        } else {
            0
        };
        self.restored_within = Some((index, within_samples));
        self.model.current = Some(item.clone());
        self.model.state = PlayerState::Paused;
        self.model.position_seconds = (start_samples + within_samples) as f64 / rate as f64;
        self.model.current_track_start_seconds = start_samples as f64 / rate as f64;
        self.model.current_track_duration_seconds = self.length_of(index) as f64 / rate as f64;
        self.model.duration_seconds = self.total_length() as f64 / rate as f64;
        if let Some(v) = snapshot.volume {
            self.model.volume = v.clamp(0.0, 1.0);
        }
        if let Some(mode) = snapshot.normalize_mode {
            self.model.normalize_mode = mode;
        }
        self.model.repeat = self.queue.repeat();
        self.model.shuffle = self.queue.shuffling();
        self.model.crossfade_seconds = self.crossfade_seconds;
        self.push(PlayerEvent::Track { item: Some(item) });
        self.take_events()
    }

    // ---- queue actions ---------------------------------------------------

    /// Replaces the queue with a single item and loads it.
    pub fn play_item(&mut self, item: PlaybackItem) -> Vec<PlayerEvent> {
        self.events.clear();
        self.pause_intent = false;
        self.set_state(PlayerState::Loading);
        self.mutating = true;
        self.queue.play_now(item);
        self.mutating = false;
        self.load(None);
        self.take_events()
    }

    /// Plays a sequence starting at `start_index`.
    pub fn play_sequence(
        &mut self,
        items: Vec<PlaybackItem>,
        start_index: i64,
    ) -> Vec<PlayerEvent> {
        self.events.clear();
        if items.is_empty() {
            return self.take_events();
        }
        self.pause_intent = false;
        self.set_state(PlayerState::Loading);
        self.mutating = true;
        let _ = self.queue.play_sequence(items, start_index);
        self.mutating = false;
        self.load(None);
        self.take_events()
    }

    /// Jumps to an existing queue item without touching the rest of the
    /// queue.
    pub fn play_queue_index(&mut self, i: i64) -> Vec<PlayerEvent> {
        self.events.clear();
        if self.queue.at(i).is_none() || i == self.queue.index() {
            return self.take_events();
        }
        self.pause_intent = false;
        self.set_state(PlayerState::Loading);
        self.mutating = true;
        self.queue.move_to(i);
        self.mutating = false;
        self.load(None);
        self.take_events()
    }

    /// Advances under the active policy.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.loading_seq += 1;
        self.mutating = true;
        let item = self.queue.next();
        self.mutating = false;
        if item.is_some() {
            self.load(None);
        }
        self.take_events()
    }

    /// History-aware back navigation (or restart the current track).
    pub fn previous(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        let restart = self.model.current.is_some()
            && self.model.position_seconds - self.model.current_track_start_seconds > 3.0;
        if restart {
            self.restart_current_track();
            return self.take_events();
        }
        self.loading_seq += 1;
        self.mutating = true;
        let item = self.queue.previous();
        self.mutating = false;
        if item.is_some() {
            self.load(None);
        }
        self.take_events()
    }

    /// Seeks to an album-absolute position in seconds.
    pub fn seek(&mut self, seconds: f64) -> Vec<PlayerEvent> {
        self.events.clear();
        self.seek_inner(seconds);
        self.take_events()
    }

    /// Stops playback but keeps the engine (a later play reloads).
    pub fn stop(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.stop_inner();
        self.take_events()
    }

    /// Stops playback and disposes the engine + exact lengths.
    pub fn teardown(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.teardown_inner();
        self.take_events()
    }

    /// Toggles play/pause.
    pub fn toggle_play(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.toggle_play_inner();
        self.take_events()
    }

    /// Pauses.
    pub fn pause(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.pause_inner();
        self.take_events()
    }

    /// Resumes.
    pub fn resume(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.resume_inner();
        self.take_events()
    }

    /// Sets the user volume (clamped to `[0, 1]`).
    pub fn set_volume(&mut self, v: f64) -> Vec<PlayerEvent> {
        self.events.clear();
        self.model.volume = v.clamp(0.0, 1.0);
        let current = self.model.current.clone();
        self.apply_gain(current.as_ref());
        self.take_events()
    }

    /// Sets the normalization mode.
    pub fn set_normalize_mode(&mut self, mode: NormalizationMode) -> Vec<PlayerEvent> {
        self.events.clear();
        self.model.normalize_mode = mode;
        let current = self.model.current.clone();
        self.apply_gain(current.as_ref());
        self.take_events()
    }

    /// Sets the repeat policy.
    pub fn set_repeat(&mut self, mode: RepeatMode) -> Vec<PlayerEvent> {
        self.events.clear();
        self.queue.set_repeat(mode);
        self.sync_policy();
        self.take_events()
    }

    /// Enables/disables shuffle.
    pub fn set_shuffle(&mut self, on: bool) -> Vec<PlayerEvent> {
        self.events.clear();
        self.queue.set_shuffle(on);
        self.reset_offset = self
            .offsets()
            .get(self.queue.index() as usize)
            .copied()
            .unwrap_or(0);
        if self.model.current.is_some() {
            self.model.current_track_start_seconds = self.reset_offset as f64 / self.rate() as f64;
            self.model.position_seconds = self.album_position() as f64 / self.rate() as f64;
        }
        self.sync_policy();
        self.take_events()
    }

    /// Sets the crossfade length; only `{0, 4, 8, 12}` are accepted.
    pub fn set_crossfade(&mut self, seconds: f64) -> Vec<PlayerEvent> {
        self.events.clear();
        self.crossfade_seconds = if [0.0, 4.0, 8.0, 12.0].contains(&seconds) {
            seconds
        } else {
            0.0
        };
        self.model.crossfade_seconds = self.crossfade_seconds;
        self.push(PlayerEvent::Crossfade {
            seconds: self.crossfade_seconds,
        });
        self.take_events()
    }

    // ---- engine event injections ----------------------------------------

    /// Engine reports enough output buffered.
    pub fn on_primed(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.on_primed_inner();
        self.take_events()
    }

    /// Engine reports output starved.
    pub fn on_buffering(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        let state = self.model.state;
        if matches!(
            state,
            PlayerState::Loading | PlayerState::Buffering | PlayerState::Playing
        ) {
            self.set_state(PlayerState::Buffering);
        }
        self.take_events()
    }

    /// Engine reports the current source finished.
    pub fn on_eos(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.on_eos_inner();
        self.take_events()
    }

    /// Engine reports a coarse position change.
    pub fn on_tick(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        self.on_tick_inner();
        self.take_events()
    }

    /// Engine reports an error.
    pub fn on_engine_error(&mut self, message: &str) -> Vec<PlayerEvent> {
        self.events.clear();
        self.fail(message);
        self.take_events()
    }

    /// Completes a fade started by an engine that returned
    /// [`CrossfadeStart::Pending`].
    pub fn on_crossfade_complete(&mut self, result: Option<CrossfadeResult>) -> Vec<PlayerEvent> {
        self.events.clear();
        if let Some(xf) = self.xfade.take() {
            if let Some(res) = result {
                let taken = self.apply_crossfade_result(
                    xf.seq,
                    xf.epoch,
                    &xf.target,
                    xf.prev_item.as_ref(),
                    xf.prev_len,
                    &res,
                );
                if taken {
                    self.finish_eos_race(xf.eos_race);
                }
            }
            self.crossfade_in_progress = false;
        }
        self.take_events()
    }

    /// Host hook for queue mutations made outside the player.
    pub fn on_queue_changed(&mut self) -> Vec<PlayerEvent> {
        self.events.clear();
        if !self.mutating {
            let cur = self.queue.current().cloned();
            let state = self.model.state;
            if cur.is_none() && state != PlayerState::Idle && state != PlayerState::Error {
                self.stop_inner();
            } else if let Some(cur) = cur {
                if let Some(current) = &self.model.current {
                    if cur.id != current.id && state != PlayerState::Loading {
                        self.load(None);
                    }
                }
            }
        }
        self.take_events()
    }

    // ---- internals -------------------------------------------------------

    fn take_events(&mut self) -> Vec<PlayerEvent> {
        std::mem::take(&mut self.events)
    }

    fn push(&mut self, event: PlayerEvent) {
        self.events.push(event);
    }

    fn set_state(&mut self, state: PlayerState) {
        self.model.state = state;
        self.push(PlayerEvent::State { state });
    }

    fn fail(&mut self, message: &str) {
        if let Some(e) = self.engine.as_mut() {
            e.pause_pumping();
        }
        self.model.state = PlayerState::Error;
        self.model.error = Some(message.to_string());
        self.push(PlayerEvent::State {
            state: PlayerState::Error,
        });
        self.push(PlayerEvent::Error {
            message: message.to_string(),
        });
    }

    fn rate(&self) -> u32 {
        self.output_rate
    }

    fn length_of(&self, i: i64) -> u64 {
        let Some(item) = self.queue.at(i) else {
            return 0;
        };
        if let Some(exact) = self.lengths.get(&item.track_id) {
            return *exact;
        }
        let seconds = item.duration_hint_seconds.unwrap_or(0.0);
        let samples = seconds * self.rate() as f64;
        if samples.is_finite() && samples > 0.0 {
            samples.floor() as u64
        } else {
            0
        }
    }

    fn offsets(&self) -> Vec<u64> {
        let items = self.queue.items();
        let mut offs = Vec::with_capacity(items.len());
        let mut acc: u64 = 0;
        for i in 0..items.len() {
            offs.push(acc);
            acc = acc.saturating_add(self.length_of(i as i64));
        }
        offs
    }

    fn total_length(&self) -> u64 {
        let offs = self.offsets();
        let Some(last) = offs.last() else {
            return 0;
        };
        last.saturating_add(self.length_of(offs.len() as i64 - 1))
    }

    fn album_position(&self) -> u64 {
        let Some(engine) = self.engine.as_ref() else {
            return 0;
        };
        let rendered = engine.rendered_samples();
        if self.engine_kind == Some(EngineKind::Musepack) {
            self.reset_offset.saturating_add(rendered)
        } else {
            let idx = self.queue.index();
            let base = if idx >= 0 {
                self.offsets().get(idx as usize).copied().unwrap_or(0)
            } else {
                0
            };
            base.saturating_add(rendered)
        }
    }

    fn current_index_at(&self, pos: u64) -> i64 {
        let offs = self.offsets();
        let mut idx = 0i64;
        for (i, &start) in offs.iter().enumerate() {
            if pos >= start {
                idx = i as i64;
            }
        }
        idx
    }

    fn track_remaining_samples(&self, idx: i64, pos: u64) -> u64 {
        let offs = self.offsets();
        let start = offs.get(idx as usize).copied().unwrap_or(0);
        start
            .saturating_add(self.length_of(idx))
            .saturating_sub(pos)
    }

    fn peek_preload_target(&self, qi: i64) -> Option<PlaybackItem> {
        let items = self.queue.items();
        if self.queue.shuffling() {
            let order = self.queue.presentation_order();
            let pos = order.iter().position(|&x| x as i64 == qi);
            if let Some(pos) = pos {
                if pos + 1 < order.len() {
                    return self.queue.at(order[pos + 1] as i64).cloned();
                }
            }
            if self.queue.repeat() == RepeatMode::All {
                let first = order.first().copied();
                return first.and_then(|f| self.queue.at(f as i64).cloned());
            }
            return None;
        }
        if qi + 1 < items.len() as i64 {
            return self.queue.at(qi + 1).cloned();
        }
        if self.queue.repeat() == RepeatMode::All && !items.is_empty() {
            return self.queue.at(0).cloned();
        }
        None
    }

    fn ensure_engine(&mut self, kind: EngineKind) {
        if self.engine.is_some() && self.engine_kind == Some(kind) {
            return;
        }
        self.detach_engine_listeners();
        if let Some(e) = self.engine.as_mut() {
            e.close();
        }
        self.engine = None;
        self.engine_kind = Some(kind);
        self.engine_epoch += 1;
        let engine = (self.ports.engine_factory)(kind);
        self.engine = Some(engine);
    }

    fn detach_engine_listeners(&mut self) {
        self.engine_epoch += 1;
    }

    fn load(&mut self, resume_within: Option<u64>) -> bool {
        let Some(item) = self.queue.current().cloned() else {
            return false;
        };
        self.loading_seq += 1;
        let seq = self.loading_seq;
        self.transport_seq += 1;
        self.pending_ended = false;
        self.restored_within = None;
        self.crossfade_armed = false;
        self.model.state = PlayerState::Loading;
        self.model.error = None;

        let kind = match (self.ports.resolve_kind)(&item) {
            Ok(kind) => kind,
            Err(e) => {
                if seq == self.loading_seq {
                    self.fail(&e);
                }
                return false;
            }
        };
        self.ensure_engine(kind);
        if seq != self.loading_seq {
            return false;
        }
        let info = match self.engine.as_mut() {
            Some(engine) => match engine.open(&item) {
                Ok(info) => info,
                Err(EngineError(e)) => {
                    if seq == self.loading_seq {
                        self.fail(&e);
                    }
                    return false;
                }
            },
            None => return false,
        };
        if seq != self.loading_seq {
            return false;
        }

        let qi = self.queue.index();
        self.lengths.insert(item.track_id, info.length_samples);
        self.output_rate = info.rate;
        let offs = self.offsets();
        self.model.current_track_start_seconds =
            offs.get(qi as usize).copied().unwrap_or(0) as f64 / self.rate() as f64;
        self.model.current_track_duration_seconds = self.length_of(qi) as f64 / self.rate() as f64;

        if let Some(next) = self.peek_preload_target(qi) {
            let preload = self
                .engine
                .as_ref()
                .map(|e| e.capabilities().preload_next)
                .unwrap_or(false);
            if preload {
                let ni = self.engine.as_mut().and_then(|e| e.prepare_next(&next));
                if seq != self.loading_seq {
                    return false;
                }
                if let Some(ni) = ni {
                    self.lengths.insert(next.track_id, ni.length_samples);
                }
            }
        }

        let resume_at = match resume_within {
            Some(w) if w > 0 => w.min(self.length_of(qi).saturating_sub(1)),
            _ => 0,
        };
        self.reset_offset = offs
            .get(qi as usize)
            .copied()
            .unwrap_or(0)
            .saturating_add(resume_at);
        self.apply_gain(Some(&item));
        self.model.current = Some(item.clone());
        self.model.state = if self.pause_intent {
            PlayerState::Paused
        } else {
            PlayerState::Buffering
        };
        self.model.position_seconds = self.reset_offset as f64 / self.rate() as f64;
        self.model.duration_seconds = self.total_length() as f64 / self.rate() as f64;
        self.push(PlayerEvent::Track {
            item: Some(item.clone()),
        });
        if let Some(engine) = self.engine.as_mut() {
            engine.seek(resume_at);
        }
        if seq != self.loading_seq {
            return false;
        }
        if !self.pause_intent {
            if let Some(engine) = self.engine.as_mut() {
                engine.start_pumping();
            }
        }
        true
    }

    fn on_primed_inner(&mut self) {
        let state = self.model.state;
        if !matches!(state, PlayerState::Loading | PlayerState::Buffering) {
            return;
        }
        let seq = self.loading_seq;
        let transport_seq = self.transport_seq;
        let epoch = self.engine_epoch;
        let result = match self.engine.as_mut() {
            Some(engine) => engine.play(),
            None => return,
        };
        if result.is_ok() {
            let state = self.model.state;
            if seq == self.loading_seq
                && transport_seq == self.transport_seq
                && !self.pause_intent
                && self.engine_epoch == epoch
                && matches!(state, PlayerState::Loading | PlayerState::Buffering)
            {
                self.set_state(PlayerState::Playing);
            }
        }
    }

    fn on_eos_inner(&mut self) {
        if self.engine.is_none() {
            return;
        }
        let seq = self.loading_seq;
        self.eos_in_flight = true;
        self.run_eos_boundary(seq);
        self.eos_in_flight = false;
    }

    fn run_eos_boundary(&mut self, seq: u64) {
        if self.engine.is_none() {
            return;
        }
        if self.try_fade_at_eos(seq) {
            return;
        }
        if self.crossfade_in_progress {
            return;
        }
        if self.pause_intent {
            return;
        }
        let expected = self.peek_preload_target(self.queue.index());
        let preload = self
            .engine
            .as_ref()
            .map(|e| e.capabilities().preload_next)
            .unwrap_or(false);
        let mut info: Option<StreamInfo> = None;
        if preload {
            info = self
                .engine
                .as_mut()
                .and_then(|e| e.advance(expected.as_ref()));
        }
        if seq != self.loading_seq {
            return;
        }
        if self.pause_intent {
            return;
        }

        if self.queue.repeat() == RepeatMode::One {
            let idx = self.queue.index();
            self.mutating = true;
            self.queue.move_to(idx);
            self.mutating = false;
            self.load(Some(0));
            return;
        }

        self.mutating = true;
        let item = self.queue.next();
        self.mutating = false;
        let Some(item) = item else {
            self.pending_ended = true;
            if let Some(engine) = self.engine.as_mut() {
                engine.pause_pumping();
            }
            self.on_tick_inner();
            return;
        };
        let expected_matches = expected
            .as_ref()
            .map(|e| same_item_identity(&item, e))
            .unwrap_or(false);
        if info.is_none() || !expected_matches {
            self.load(None);
            return;
        }
        let info = info.unwrap();
        let qi = self.queue.index();
        self.lengths.insert(item.track_id, info.length_samples);
        self.output_rate = info.rate;
        let offs = self.offsets();
        self.model.current_track_start_seconds =
            offs.get(qi as usize).copied().unwrap_or(0) as f64 / self.rate() as f64;
        self.model.current_track_duration_seconds = self.length_of(qi) as f64 / self.rate() as f64;
        self.apply_gain(Some(&item));
        if let Some(next) = self.peek_preload_target(qi) {
            if preload {
                let ni = self.engine.as_mut().and_then(|e| e.prepare_next(&next));
                if seq != self.loading_seq {
                    return;
                }
                if let Some(ni) = ni {
                    self.lengths.insert(next.track_id, ni.length_samples);
                }
            }
        }
        self.model.current = Some(item.clone());
        self.model.state = if self.pause_intent || self.model.state == PlayerState::Paused {
            PlayerState::Paused
        } else {
            PlayerState::Buffering
        };
        self.model.duration_seconds = self.total_length() as f64 / self.rate() as f64;
        self.push(PlayerEvent::Track {
            item: Some(item.clone()),
        });

        if !self.pause_intent {
            let epoch = self.engine_epoch;
            let transport_seq = self.transport_seq;
            let same_engine =
                |this: &Self| this.engine_epoch == epoch && this.transport_seq == transport_seq;
            if let Some(engine) = self.engine.as_mut() {
                engine.start_pumping();
            }
            let result = match self.engine.as_mut() {
                Some(engine) => engine.play(),
                None => return,
            };
            if result.is_ok() {
                if seq == self.loading_seq
                    && !self.pause_intent
                    && self.engine_epoch == epoch
                    && transport_seq == self.transport_seq
                {
                    self.set_state(PlayerState::Playing);
                }
            } else if seq == self.loading_seq && same_engine(self) {
                self.set_state(PlayerState::Paused);
            }
        }
    }

    fn try_fade_at_eos(&mut self, seq: u64) -> bool {
        if self.crossfade_seconds <= 0.0 || self.queue.repeat() == RepeatMode::One {
            return false;
        }
        if self.pause_intent || self.pending_ended || self.crossfade_in_progress {
            return false;
        }
        let Some(caps) = self.engine.as_ref().map(|e| e.capabilities()) else {
            return false;
        };
        if !caps.crossfade || !caps.preload_next {
            return false;
        }
        let qi = self.queue.index();
        let Some(target) = self.peek_preload_target(qi) else {
            return false;
        };
        let remaining = self.track_remaining_samples(qi, self.album_position());
        if remaining <= END_TOLERANCE_SAMPLES {
            return false;
        }
        let max_overlap = self
            .crossfade_seconds
            .min(remaining as f64 / self.rate() as f64);
        let mut overlap_seconds = max_overlap;
        let mut decline = false;
        if let Some(plan_fn) = self.ports.plan_transition.as_ref() {
            let outgoing = self.queue.at(qi).cloned();
            let plan = outgoing.map(|outgoing| {
                plan_fn(&TransitionQuery {
                    outgoing,
                    incoming: target.clone(),
                    max_fade_seconds: max_overlap,
                    repeat_one: false,
                    same_release: None,
                })
            });
            match plan {
                Some(TransitionPlan::SweetFade { overlap_seconds: o }) => {
                    overlap_seconds = o.min(max_overlap);
                }
                _ => decline = true,
            }
        }
        if decline || overlap_seconds <= 0.0 {
            return false;
        }
        let raw_len = self.length_of(qi);
        let outgoing_track_id = self.queue.at(qi).map(|i| i.track_id).unwrap_or(0);
        let eos_race = EosRace {
            outgoing_track_id,
            raw_len,
            remaining_at_eos: remaining,
        };
        self.begin_crossfade_transition(Some(overlap_seconds), Some(seq), Some(eos_race))
    }

    fn begin_crossfade_transition(
        &mut self,
        overlap_seconds: Option<f64>,
        seq_hint: Option<u64>,
        eos_race: Option<EosRace>,
    ) -> bool {
        let Some(caps) = self.engine.as_ref().map(|e| e.capabilities()) else {
            return false;
        };
        if !caps.crossfade {
            return false;
        }
        let requested = overlap_seconds
            .unwrap_or(self.crossfade_seconds)
            .clamp(0.25, 15.0);
        let seq_at_start = seq_hint.unwrap_or(self.loading_seq);
        let qi = self.queue.index();
        let Some(target) = self.peek_preload_target(qi) else {
            return false;
        };
        let prev_item = self.queue.at(qi).cloned();
        let prev_len = prev_item
            .as_ref()
            .and_then(|it| self.lengths.get(&it.track_id).copied());
        let epoch = self.engine_epoch;
        self.crossfade_in_progress = true;
        let start = self
            .engine
            .as_mut()
            .map(|e| e.begin_crossfade(&target, requested))
            .unwrap_or(CrossfadeStart::Declined);
        match start {
            CrossfadeStart::Declined => {
                self.crossfade_in_progress = false;
                false
            }
            CrossfadeStart::Completed(result) => {
                let taken = self.apply_crossfade_result(
                    seq_at_start,
                    epoch,
                    &target,
                    prev_item.as_ref(),
                    prev_len,
                    &result,
                );
                if taken {
                    self.finish_eos_race(eos_race);
                }
                self.crossfade_in_progress = false;
                taken
            }
            CrossfadeStart::Pending => {
                self.xfade = Some(XfadeInFlight {
                    seq: seq_at_start,
                    epoch,
                    target,
                    prev_item,
                    prev_len,
                    eos_race,
                });
                true
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_crossfade_result(
        &mut self,
        seq_at_start: u64,
        epoch: u64,
        target: &PlaybackItem,
        prev_item: Option<&PlaybackItem>,
        prev_len: Option<u64>,
        result: &CrossfadeResult,
    ) -> bool {
        if seq_at_start != self.loading_seq || self.engine_epoch != epoch {
            return false;
        }
        let Some(idx_after) = self
            .queue
            .items()
            .iter()
            .position(|it| same_item_identity(it, target))
        else {
            return false;
        };
        if let (Some(prev), Some(plen)) = (prev_item, prev_len) {
            if result.overlap_frames > 0 {
                self.lengths
                    .insert(prev.track_id, plen.saturating_sub(result.overlap_frames));
            }
        }
        self.mutating = true;
        self.queue.move_to(idx_after as i64);
        self.mutating = false;
        self.pending_boundary_check = Some(BoundaryCheck {
            idx: idx_after as i64,
            seq: seq_at_start,
        });
        self.lengths
            .insert(target.track_id, result.info.length_samples);
        self.output_rate = result.info.rate;
        let nqi = self.queue.index();
        let offs = self.offsets();
        self.model.current_track_start_seconds =
            offs.get(nqi as usize).copied().unwrap_or(0) as f64 / self.rate() as f64;
        self.model.current_track_duration_seconds = self.length_of(nqi) as f64 / self.rate() as f64;
        self.model.duration_seconds = self.total_length() as f64 / self.rate() as f64;
        self.model.current = Some(target.clone());
        self.push(PlayerEvent::Track {
            item: Some(target.clone()),
        });
        self.apply_gain(Some(target));
        self.crossfade_armed = false;
        let preload = self
            .engine
            .as_ref()
            .map(|e| e.capabilities().preload_next)
            .unwrap_or(false);
        if let Some(next2) = self.peek_preload_target(nqi) {
            if preload {
                let ni = self.engine.as_mut().and_then(|e| e.prepare_next(&next2));
                if let Some(ni) = ni {
                    self.lengths.insert(next2.track_id, ni.length_samples);
                }
            }
        }
        true
    }

    fn finish_eos_race(&mut self, race: Option<EosRace>) {
        let Some(race) = race else {
            return;
        };
        if race.raw_len == 0 {
            return;
        }
        self.lengths.insert(
            race.outgoing_track_id,
            race.raw_len.saturating_sub(race.remaining_at_eos),
        );
        let qi = self.queue.index();
        let offs = self.offsets();
        self.model.current_track_start_seconds =
            offs.get(qi as usize).copied().unwrap_or(0) as f64 / self.rate() as f64;
        self.model.current_track_duration_seconds = self.length_of(qi) as f64 / self.rate() as f64;
        self.model.duration_seconds = self.total_length() as f64 / self.rate() as f64;
    }

    fn on_tick_inner(&mut self) {
        if self.engine.is_none() {
            return;
        }
        let pos = self.album_position();
        let dur = self.total_length();
        let qi = self.queue.index();
        let idx = self.current_index_at(pos);

        if let Some(check) = self.pending_boundary_check.take() {
            if check.seq == self.loading_seq && idx != check.idx {
                self.push(PlayerEvent::BoundaryDrift {
                    expected_index: check.idx,
                    observed_index: idx,
                    position_samples: pos,
                });
            }
        }

        if self.model.state == PlayerState::Playing
            && idx > qi
            && !self.pending_ended
            && !self.crossfade_in_progress
            && !self.eos_in_flight
        {
            let target = idx.min(qi + 1);
            if self.length_of(target) > 0 {
                self.mutating = true;
                self.queue.move_to(target);
                self.mutating = false;
            }
        }

        let offs = self.offsets();
        let track_start = offs.get(idx as usize).copied().unwrap_or(0);
        let track_dur = self.length_of(idx);
        let rate = self.rate();

        // Synchronous part of tick(): geometry + model update + position
        // event + end handling. In TypeScript the crossfade trigger is
        // dispatched here (asynchronously); its bookkeeping continuation runs
        // afterwards and therefore wins the final model fields. The Rust port
        // mirrors that by running the trigger *after* this block.
        let drained = self
            .engine
            .as_ref()
            .map(|e| e.is_output_drained())
            .unwrap_or(false);
        let ended =
            self.pending_ended && (pos.saturating_add(END_TOLERANCE_SAMPLES) >= dur || drained);
        self.model.position_seconds = pos as f64 / rate as f64;
        self.model.duration_seconds = dur as f64 / rate as f64;
        self.model.current_track_start_seconds = track_start as f64 / rate as f64;
        self.model.current_track_duration_seconds = track_dur as f64 / rate as f64;
        if ended {
            self.model.state = PlayerState::Ended;
        }
        let within = pos.saturating_sub(track_start);
        self.push(PlayerEvent::Position {
            position_seconds: within as f64 / rate as f64,
            track_start_seconds: track_start as f64 / rate as f64,
            track_duration_seconds: track_dur as f64 / rate as f64,
        });
        if ended {
            self.pending_ended = false;
            if let Some(engine) = self.engine.as_mut() {
                engine.pause();
                engine.pause_pumping();
            }
        }

        // Crossfade trigger (the asynchronous continuation in TypeScript).
        if !self.crossfade_armed
            && !self.crossfade_in_progress
            && !self.pending_ended
            && self.crossfade_seconds > 0.0
            && self.queue.repeat() != RepeatMode::One
            && self.model.state == PlayerState::Playing
        {
            let remaining = self.track_remaining_samples(idx, pos);
            let fade_frames = (self.crossfade_seconds * rate as f64).floor();
            let single_track_repeat_all =
                self.queue.repeat() == RepeatMode::All && self.queue.items().len() == 1;
            let crossfade_capable = self
                .engine
                .as_ref()
                .map(|e| e.capabilities().crossfade)
                .unwrap_or(false);
            if remaining as f64 <= fade_frames
                && remaining > 0
                && !single_track_repeat_all
                && crossfade_capable
            {
                if let Some(target) = self.peek_preload_target(qi) {
                    let plan = match self.ports.plan_transition.as_ref() {
                        Some(plan_fn) => {
                            let outgoing =
                                self.queue.at(qi).cloned().unwrap_or_else(|| target.clone());
                            plan_fn(&TransitionQuery {
                                outgoing,
                                incoming: target.clone(),
                                max_fade_seconds: self.crossfade_seconds,
                                repeat_one: false,
                                same_release: None,
                            })
                        }
                        None => TransitionPlan::SweetFade {
                            overlap_seconds: self.crossfade_seconds,
                        },
                    };
                    if let TransitionPlan::SweetFade { overlap_seconds } = plan {
                        let lead = ((overlap_seconds + PRIME_LEAD_SECONDS) * rate as f64).floor();
                        if remaining as f64 <= lead {
                            self.crossfade_armed = true;
                            self.begin_crossfade_transition(Some(overlap_seconds), None, None);
                        }
                    }
                }
            }
        }
    }

    fn restart_current_track(&mut self) {
        if self.engine.is_none() || self.model.current.is_none() {
            return;
        }
        self.transport_seq += 1;
        self.loading_seq += 1;
        let seq = self.loading_seq;
        let base = self
            .offsets()
            .get(self.queue.index() as usize)
            .copied()
            .unwrap_or(0);
        self.reset_offset = base;
        self.set_state(if self.pause_intent {
            PlayerState::Paused
        } else {
            PlayerState::Buffering
        });
        self.model.position_seconds = base as f64 / self.rate() as f64;
        if let Some(engine) = self.engine.as_mut() {
            engine.seek(0);
        }
        if seq != self.loading_seq {
            return;
        }
        if !self.pause_intent {
            if let Some(engine) = self.engine.as_mut() {
                engine.start_pumping();
            }
        }
    }

    fn seek_inner(&mut self, seconds: f64) {
        if self.engine.is_none() || self.model.current.is_none() {
            return;
        }
        self.transport_seq += 1;
        self.loading_seq += 1;
        let mut seq = self.loading_seq;
        let pos_samples = (seconds * self.rate() as f64).max(0.0).floor();
        let pos_samples = if pos_samples.is_finite() {
            pos_samples as u64
        } else {
            return;
        };
        let items_len = self.queue.items().len();
        if items_len == 0 {
            return;
        }
        let offs = self.offsets();
        let mut qi = 0i64;
        for (i, &start) in offs.iter().enumerate() {
            if pos_samples >= start {
                qi = i as i64;
            }
        }
        let base = offs.get(qi as usize).copied().unwrap_or(0);
        let track_len = self.length_of(qi).saturating_sub(1);
        let within = pos_samples.saturating_sub(base).min(track_len);

        if qi != self.queue.index() {
            self.mutating = true;
            self.queue.move_to(qi);
            self.mutating = false;
            if !self.load(None) {
                return;
            }
            seq = self.loading_seq;
            if qi == self.queue.index() {
                self.reset_offset = base.saturating_add(within);
                self.set_state(if self.pause_intent {
                    PlayerState::Paused
                } else {
                    PlayerState::Buffering
                });
                self.model.position_seconds = self.reset_offset as f64 / self.rate() as f64;
                if let Some(engine) = self.engine.as_mut() {
                    engine.seek(within);
                }
                if seq != self.loading_seq {
                    return;
                }
                if !self.pause_intent {
                    if let Some(engine) = self.engine.as_mut() {
                        engine.start_pumping();
                    }
                }
            }
            return;
        }

        self.reset_offset = base.saturating_add(within);
        self.set_state(if self.pause_intent {
            PlayerState::Paused
        } else {
            PlayerState::Buffering
        });
        self.model.position_seconds = self.reset_offset as f64 / self.rate() as f64;
        if let Some(engine) = self.engine.as_mut() {
            engine.seek(within);
        }
        if seq != self.loading_seq {
            return;
        }
        if !self.pause_intent {
            if let Some(engine) = self.engine.as_mut() {
                engine.start_pumping();
            }
        }
    }

    fn stop_inner(&mut self) {
        self.loading_seq += 1;
        let seq = self.loading_seq;
        self.transport_seq += 1;
        let epoch = self.engine_epoch;
        self.pause_intent = true;
        if let Some(engine) = self.engine.as_mut() {
            engine.pause_pumping();
        }
        self.pending_ended = false;
        self.model.state = PlayerState::Idle;
        self.model.position_seconds = 0.0;
        if let Some(engine) = self.engine.as_mut() {
            engine.pause();
        }
        if seq != self.loading_seq || self.engine_epoch != epoch {
            return;
        }
        if let Some(engine) = self.engine.as_mut() {
            engine.seek(0);
        }
    }

    fn teardown_inner(&mut self) {
        self.loading_seq += 1;
        self.transport_seq += 1;
        self.pause_intent = false;
        if let Some(engine) = self.engine.as_mut() {
            engine.pause_pumping();
        }
        self.pending_ended = false;
        self.lengths.clear();
        self.reset_offset = 0;
        self.restored_within = None;
        self.xfade = None;
        self.crossfade_in_progress = false;
        if let Some(engine) = self.engine.as_mut() {
            engine.close();
        }
        self.engine = None;
        self.engine_kind = None;
        self.model.state = PlayerState::Idle;
        self.model.current = None;
        self.model.position_seconds = 0.0;
        self.model.duration_seconds = 0.0;
        self.model.error = None;
        self.push(PlayerEvent::State {
            state: PlayerState::Idle,
        });
        self.push(PlayerEvent::Track { item: None });
    }

    fn toggle_play_inner(&mut self) {
        match self.model.state {
            PlayerState::Playing => self.pause_inner(),
            PlayerState::Paused => {
                if self.engine.is_none() && self.model.current.is_some() {
                    let within = self
                        .restored_within
                        .filter(|(idx, _)| *idx == self.queue.index())
                        .map(|(_, w)| w);
                    self.load(within);
                    return;
                }
                self.resume_inner();
            }
            PlayerState::Ended | PlayerState::Idle | PlayerState::Error => {
                if self.model.current.is_some() {
                    self.pause_intent = false;
                    self.load(None);
                }
            }
            _ => self.resume_inner(),
        }
    }

    fn pause_inner(&mut self) {
        self.transport_seq += 1;
        self.pause_intent = true;
        self.set_state(PlayerState::Paused);
        if let Some(engine) = self.engine.as_mut() {
            engine.pause_pumping();
        }
        if let Some(engine) = self.engine.as_mut() {
            engine.pause();
        }
    }

    fn resume_inner(&mut self) {
        if self.engine.is_none() {
            if self.model.state == PlayerState::Paused && self.model.current.is_some() {
                self.toggle_play_inner();
            }
            return;
        }
        self.transport_seq += 1;
        let seq = self.transport_seq;
        let epoch = self.engine_epoch;
        self.pause_intent = false;
        if let Some(engine) = self.engine.as_mut() {
            engine.start_pumping();
        }
        let result = match self.engine.as_mut() {
            Some(engine) => engine.play(),
            None => return,
        };
        if seq != self.transport_seq || self.pause_intent || self.engine_epoch != epoch {
            return;
        }
        if result.is_ok() {
            self.set_state(PlayerState::Playing);
        }
    }

    fn sync_policy(&mut self) {
        self.model.repeat = self.queue.repeat();
        self.model.shuffle = self.queue.shuffling();
        self.push(PlayerEvent::Policy {
            repeat: self.model.repeat,
            shuffle: self.model.shuffle,
        });
    }

    fn apply_gain(&mut self, item: Option<&PlaybackItem>) {
        let norm_db = normalization_gain(
            self.model.normalize_mode,
            item.and_then(|i| i.loudness.as_ref()),
            item.and_then(|i| i.album_loudness.as_ref()),
        );
        let gain = combined_gain(self.model.volume, norm_db);
        if let Some(engine) = self.engine.as_mut() {
            engine.set_gain(gain);
        }
        self.model.norm_db = norm_db;
        self.push(PlayerEvent::Gain { norm_db });
    }
}
