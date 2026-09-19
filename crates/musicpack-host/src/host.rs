//! The host pull loop: `Player` + `DecoderEngine` + a source backend.
//!
//! A device callback or AudioWorklet `process()` calls [`Host::render`] when it
//! needs frames; the host pulls decoded PCM from the engine and feeds every
//! engine fact back into the player. The engine never sees a clock, a device
//! or a browser API — it only sees `consume()` calls.

use std::cell::RefCell;
use std::rc::Rc;

use musicpack_core::player::engine::{
    CrossfadeResult, CrossfadeStart, Engine, EngineCapabilities, EngineResult,
};
use musicpack_core::player::events::PlayerEvent;
use musicpack_core::player::player::{Player, PlayerOptions, PlayerPorts};
use musicpack_core::player::queue::QueueModel;
use musicpack_core::player::types::{EngineKind, PlaybackItem, StreamInfo};
use musicpack_engine::{DecoderEngine, DecoderEngineConfig, SniffingDecoderFactory, SourceBackend};

/// Host output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostConfig {
    /// Output sample rate the host device consumes.
    pub output_rate: u32,
    /// Output channel count (1 or 2).
    pub output_channels: u32,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            output_rate: 44_100,
            output_channels: 2,
        }
    }
}

/// Host-side shared engine wrapper.
///
/// Hosts are single-threaded (browser main thread / one device callback); the
/// `Rc<RefCell<_>>` keeps the engine reachable by the test or device callback
/// while the player owns it through the `Engine` trait. This is intentionally
/// host-only: the engine crate remains lock-free and single-owner.
#[derive(Clone)]
struct SharedEngine(Rc<RefCell<DecoderEngine>>);

impl Engine for SharedEngine {
    fn capabilities(&self) -> EngineCapabilities {
        self.0.borrow().capabilities()
    }
    fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo> {
        self.0.borrow_mut().open(item)
    }
    fn play(&mut self) -> EngineResult<()> {
        self.0.borrow_mut().play()
    }
    fn pause(&mut self) {
        self.0.borrow_mut().pause();
    }
    fn seek(&mut self, samples: u64) {
        self.0.borrow_mut().seek(samples);
    }
    fn set_gain(&mut self, linear: f64) {
        self.0.borrow_mut().set_gain(linear);
    }
    fn rendered_samples(&self) -> u64 {
        self.0.borrow().rendered_samples()
    }
    fn close(&mut self) {
        self.0.borrow_mut().close();
    }
    fn prepare_next(&mut self, item: &PlaybackItem) -> Option<StreamInfo> {
        self.0.borrow_mut().prepare_next(item)
    }
    fn advance(&mut self, expected: Option<&PlaybackItem>) -> Option<StreamInfo> {
        self.0.borrow_mut().advance(expected)
    }
    fn begin_crossfade(&mut self, next: &PlaybackItem, fade_seconds: f64) -> CrossfadeStart {
        self.0.borrow_mut().begin_crossfade(next, fade_seconds)
    }
    fn is_output_drained(&self) -> bool {
        self.0.borrow().is_output_drained()
    }
    fn start_pumping(&mut self) {
        self.0.borrow_mut().start_pumping();
    }
    fn pause_pumping(&mut self) {
        self.0.borrow_mut().pause_pumping();
    }
}

/// A host that plays queue items through the deterministic engine.
pub struct Host {
    player: Player,
    engine: Rc<RefCell<DecoderEngine>>,
    channels: usize,
}

impl Host {
    /// Builds a host over a source backend.
    ///
    /// The backend is the only way the engine obtains bytes; this crate
    /// provides no network/filesystem/OPFS source of its own.
    pub fn new(config: HostConfig, source: Box<dyn SourceBackend>) -> Self {
        let engine = Rc::new(RefCell::new(DecoderEngine::new(
            Box::new(SniffingDecoderFactory::new(source)),
            DecoderEngineConfig {
                output_rate: config.output_rate,
                output_channels: config.output_channels,
                ..Default::default()
            },
        )));
        let engine_for_factory = engine.clone();
        let ports = PlayerPorts {
            engine_factory: Box::new(move |_kind: EngineKind| {
                Box::new(SharedEngine(engine_for_factory.clone())) as Box<dyn Engine>
            }),
            // The decoder engine reports the ring playhead rebased across a
            // swap; the "reset offset + rendered" album clock is the right
            // convention (the reference's Musepack pipeline path).
            resolve_kind: Box::new(|_item: &PlaybackItem| Ok(EngineKind::Musepack)),
            plan_transition: None,
        };
        let player = Player::new(
            QueueModel::new(Box::new(|| 0.5)),
            ports,
            PlayerOptions::default(),
        );
        Self {
            player,
            engine,
            channels: config.output_channels as usize,
        }
    }

    /// The player (read-only view).
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// The player (mutable), for host commands.
    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    /// The shared engine handle (host control: diagnostics, tests).
    pub fn engine(&self) -> &Rc<RefCell<DecoderEngine>> {
        &self.engine
    }

    /// Loads a queue and returns the resulting player events (no priming).
    pub fn load(&mut self, items: Vec<PlaybackItem>, start: i64) -> Vec<PlayerEvent> {
        self.player.play_sequence(items, start)
    }

    /// Loads a queue and primes it (the host reports a primed buffer).
    pub fn load_and_play(&mut self, items: Vec<PlaybackItem>, start: i64) -> Vec<PlayerEvent> {
        let mut events = self.load(items, start);
        events.extend(self.prime());
        events
    }

    /// Reports a primed buffer (`Player::on_primed`).
    pub fn prime(&mut self) -> Vec<PlayerEvent> {
        self.player.on_primed()
    }

    /// Pulls `frames` output frames (one device/worklet callback).
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut buffer = vec![0.0f32; frames * self.channels];
        self.engine.borrow_mut().consume(frames, &mut buffer);
        self.sync();
        buffer
    }

    /// Pulls into a caller-owned interleaved buffer (no allocation).
    ///
    /// Returns the number of frames written. This is the exact call a native
    /// device callback should make.
    pub fn render_into(&mut self, dst: &mut [f32]) -> usize {
        let frames = dst.len() / self.channels;
        self.engine.borrow_mut().consume(frames, dst);
        self.sync();
        frames
    }

    /// Renders until the current track drains (staying through an in-flight
    /// fade), bounded by `max` quanta.
    pub fn render_until_drained(&mut self, quantum: usize, max: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..max {
            out.extend(self.render(quantum));
            let done = {
                let engine = self.engine.borrow();
                engine.is_output_drained() && !engine.crossfade_pending()
            };
            if done {
                break;
            }
        }
        out
    }

    /// Feeds every engine fact back into the player (the host's job).
    fn sync(&mut self) {
        let result: Option<CrossfadeResult> = { self.engine.borrow_mut().take_crossfade_result() };
        if let Some(result) = result {
            self.player.on_crossfade_complete(Some(result));
        }
        let error = { self.engine.borrow_mut().take_error() };
        if let Some(error) = error {
            self.player.on_engine_error(&error.0);
        }
        let drained = { self.engine.borrow().is_output_drained() };
        if drained {
            self.player.on_eos();
        }
        self.player.on_tick();
    }
}
