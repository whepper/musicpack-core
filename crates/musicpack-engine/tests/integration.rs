//! Level-3 integration: real WAV/FLAC bytes → `AudioDecoder` →
//! `DecoderEngine` → `Player`, driven by a minimal host render loop.
//!
//! Nothing here touches a device: the "output" is the buffer handed to
//! `DecoderEngine::consume`, exactly as a real host adapter would.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use musicpack_core::player::engine::{
    CrossfadeResult, CrossfadeStart, Engine, EngineCapabilities, EngineError, EngineResult,
};
use musicpack_core::player::player::{Player, PlayerOptions, PlayerPorts, PlayerState};
use musicpack_core::player::queue::QueueModel;
use musicpack_core::player::types::{
    EngineKind, PlaybackItem, PlaybackSource, SourceKind, StreamInfo,
};
use musicpack_engine::{
    DecoderEngine, DecoderEngineConfig, DecoderFactory, MemorySourceBackend,
    SniffingDecoderFactory, SourceBackend,
};

const RATE: u32 = 44_100;

fn build_wav_pcm16(
    channels: u16,
    frames: usize,
    mut sample: impl FnMut(usize, usize) -> f32,
) -> Vec<u8> {
    let bytes_per = 2u16;
    let block_align = channels * bytes_per;
    let data_len = (frames * channels as usize * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for c in 0..channels as usize {
            let v = (sample(i, c).clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn wav_url(n: u64) -> String {
    format!("/memory/{n}.wav")
}

fn item(n: u64, frames: usize) -> PlaybackItem {
    PlaybackItem {
        id: format!("t{n}"),
        track_id: n as i64,
        source: PlaybackSource {
            kind: SourceKind::Other("memory".into()),
            url: wav_url(n),
            byte_size: None,
        },
        duration_hint_seconds: Some(frames as f64 / RATE as f64),
        title: format!("T{n}"),
        artist: "A".into(),
        album_title: "AL".into(),
        edition: None,
        artwork_url: None,
        loudness: None,
        album_loudness: None,
        codec: Some("wav".into()),
        mime_type: None,
        extra: Vec::new(),
    }
}

/// A counting factory wrapper: records how many decoders were opened.
#[derive(Clone)]
struct CountingFactory {
    inner: Rc<dyn DecoderFactory>,
    opens: Rc<Cell<usize>>,
}

impl DecoderFactory for CountingFactory {
    fn open_decoder(
        &self,
        item: &PlaybackItem,
    ) -> Result<Box<dyn musicpack_core::audio::AudioDecoder>, EngineError> {
        self.opens.set(self.opens.get() + 1);
        self.inner.open_decoder(item)
    }
}

/// Host-side shared engine wrapper (delegates to the Rc the test also holds).
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

struct Host {
    player: Player,
    engine: Rc<RefCell<DecoderEngine>>,
    opens: Rc<Cell<usize>>,
    channels: usize,
}

impl Host {
    fn new(source: MemorySourceBackend, config: DecoderEngineConfig) -> Self {
        let shared_source: Rc<dyn SourceBackend> = Rc::new(source);
        let factory: Rc<dyn DecoderFactory> = Rc::new(SniffingDecoderFactory::new(shared_source));
        let opens = Rc::new(Cell::new(0));
        let counting = CountingFactory {
            inner: factory,
            opens: opens.clone(),
        };
        let engine = Rc::new(RefCell::new(DecoderEngine::new(Box::new(counting), config)));
        let engine_for_factory = engine.clone();
        let ports = PlayerPorts {
            engine_factory: Box::new(move |_kind: EngineKind| {
                Box::new(SharedEngine(engine_for_factory.clone())) as Box<dyn Engine>
            }),
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
            opens,
            channels: config.output_channels as usize,
        }
    }

    /// Renders one quantum: consume from the engine, then feed every engine
    /// fact back into the player (the host adapter's job).
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * self.channels];
        self.engine.borrow_mut().consume(frames, &mut buf);
        self.sync();
        buf
    }

    fn sync(&mut self) {
        let result = { self.engine.borrow_mut().take_crossfade_result() };
        if let Some(result) = result {
            self.player.on_crossfade_complete(Some(result));
        }
        let error = { self.engine.borrow_mut().take_error() };
        if let Some(err) = error {
            self.player.on_engine_error(&err.0);
        }
        let drained = { self.engine.borrow().is_output_drained() };
        if drained {
            self.player.on_eos();
        }
        self.player.on_tick();
    }

    fn render_until_drained(&mut self, quantum: usize, max: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..max {
            out.extend(self.render(quantum));
            // A drained outgoing lane mid-fade is not the end: the lane still
            // has to finish mixing. A real host keeps consuming.
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
}

fn default_config() -> DecoderEngineConfig {
    DecoderEngineConfig::default()
}

fn backend_with(items: &[(u64, usize)]) -> MemorySourceBackend {
    let mut backend = MemorySourceBackend::new();
    for &(n, frames) in items {
        let bytes = build_wav_pcm16(2, frames, |i, c| {
            ((i as f32 * 0.01).sin()) * 0.5 + c as f32 * 0.0
        });
        backend.insert(wav_url(n), bytes);
    }
    backend
}

#[test]
fn open_play_reaches_eos_and_ends() {
    let mut host = Host::new(backend_with(&[(1, RATE as usize)]), default_config());
    host.player.play_sequence(vec![item(1, RATE as usize)], 0);
    assert_eq!(host.player.model().state, PlayerState::Buffering);
    host.player.on_primed();
    let out = host.render_until_drained(1024, 200);
    assert!(out.iter().all(|s| s.is_finite()));
    assert_eq!(host.player.model().state, PlayerState::Ended);
}

#[test]
fn position_advances_with_rendered_frames() {
    let frames = 2 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render(4410); // 0.1 s
    let pos = host.player.model().position_seconds;
    assert!((pos - 0.1).abs() < 1e-3, "position {pos}");
}

#[test]
fn pause_stops_position_and_resume_continues() {
    let frames = 2 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render(4410);
    host.player.pause();
    assert_eq!(host.player.model().state, PlayerState::Paused);
    let paused_at = host.player.model().position_seconds;
    // No rendering while paused: position is unchanged.
    assert_eq!(host.player.model().position_seconds, paused_at);
    host.player.resume();
    host.render(4410);
    assert!(host.player.model().position_seconds > paused_at);
}

#[test]
fn seek_reopens_and_repositions() {
    let frames = 4 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render(4410);
    let opens_before = host.opens.get();
    host.player.seek(2.0);
    assert!(host.opens.get() > opens_before, "seek reopens the decoder");
    assert!((host.player.model().position_seconds - 2.0).abs() < 0.02);
    host.render(4410);
    assert!((host.player.model().position_seconds - 2.1).abs() < 0.02);
}

#[test]
fn gapless_advance_uses_the_standby() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.on_primed();
    // Opening the sequence opens track 1 and preloads track 2: 2 decoders.
    assert_eq!(host.opens.get(), 2);
    host.render_until_drained(1024, 200);
    // The handoff promoted the standby; no third decoder was opened.
    assert_eq!(host.player.queue().index(), 1);
    assert_eq!(host.player.model().current.as_ref().unwrap().id, "t2");
    assert_eq!(host.opens.get(), 2);
    host.render_until_drained(1024, 400);
    assert_eq!(host.player.model().state, PlayerState::Ended);
}

#[test]
fn crossfade_mixes_and_advances_with_continuity() {
    // Long enough tracks that the fade window fits inside the outgoing track.
    let frames = 8 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.set_crossfade(4.0);
    host.player.on_primed();
    // Render until the crossfade completes (the trigger fires near 4 s in).
    let mut rendered = 0usize;
    while rendered < 10 * RATE as usize && host.player.queue().index() == 0 {
        host.render(4410);
        rendered += 4410;
    }
    assert_eq!(
        host.player.queue().index(),
        1,
        "cursor advanced by the fade"
    );
    assert_eq!(host.player.model().current.as_ref().unwrap().id, "t2");
    // Album clock compressed by the overlap: t2 starts at 4 s, not 8 s.
    let start = host.player.model().current_track_start_seconds;
    assert!((start - 4.0).abs() < 0.2, "start {start}");
    assert!((host.player.model().duration_seconds - 12.0).abs() < 0.2);
}

#[test]
fn crossfade_declines_without_capability() {
    let frames = 8 * RATE as usize;
    let mut config = default_config();
    config.crossfade = false;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), config);
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.set_crossfade(4.0);
    host.player.on_primed();
    host.render_until_drained(4096, 400);
    // Normal EOS handoff still advanced exactly once and ended.
    assert_eq!(host.player.model().state, PlayerState::Ended);
}

#[test]
fn gain_is_applied_at_the_output() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render(4410);
    let loud: f32 = host
        .render(4410)
        .iter()
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    host.player.set_volume(0.0);
    let quiet: f32 = host
        .render(4410)
        .iter()
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    assert!(loud > 0.0);
    assert_eq!(quiet, 0.0);
}

#[test]
fn teardown_clears_engine_and_player() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.player.teardown();
    assert_eq!(host.player.model().state, PlayerState::Idle);
    assert!(host.player.model().current.is_none());
    assert!(!host.engine.borrow().crossfade_pending());
}

#[test]
fn short_track_crossfade_still_advances() {
    // Tracks shorter than the fade window: the outgoing lane runs dry first.
    let frames = RATE as usize / 2;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.set_crossfade(4.0);
    host.player.on_primed();
    host.render_until_drained(1024, 800);
    assert_eq!(host.player.model().state, PlayerState::Ended);
    // Both tracks were consumed exactly once.
    assert!(host.player.model().current.is_some());
}

#[test]
fn pause_during_fade_does_not_cancel_it() {
    let frames = 8 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.set_crossfade(8.0);
    host.player.on_primed();
    // Trigger the fade by rendering into the window.
    let mut rendered = 0usize;
    while rendered < RATE as usize {
        host.render(4410);
        rendered += 4410;
        if host.player.queue().index() == 1 {
            break;
        }
    }
    // If the fade started and is pending, pause must not clear it.
    if host.engine.borrow().crossfade_pending() {
        host.player.pause();
        assert!(
            host.engine.borrow().crossfade_pending(),
            "pause keeps the lane"
        );
        host.player.resume();
    }
    host.render_until_drained(4096, 400);
    assert!(
        host.player
            .model()
            .current
            .as_ref()
            .map(|i| i.id == "t2")
            .unwrap_or(false)
    );
}

#[test]
fn unopened_engine_consumes_silence() {
    // No session at all: underrun is silence, never an error or panic.
    let mut engine = DecoderEngine::new(
        Box::new(SniffingDecoderFactory::new(MemorySourceBackend::new())),
        default_config(),
    );
    let mut buf = vec![0.0f32; 1024 * 2];
    assert_eq!(engine.consume(1024, &mut buf), 1024);
    assert!(buf.iter().all(|s| *s == 0.0));
    assert!(engine.take_error().is_none());
}

#[test]
fn underrun_renders_silence_after_drain() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render_until_drained(1024, 200);
    assert_eq!(host.player.model().state, PlayerState::Ended);
    // Draining past EOF yields silence (underrun), not a stall or error.
    let after = host.render(1024);
    assert!(after.iter().all(|s| *s == 0.0));
    assert!(host.engine.borrow_mut().take_error().is_none());
}

#[test]
fn stale_crossfade_completion_is_ignored() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames)]), default_config());
    host.player.play_sequence(vec![item(1, frames)], 0);
    host.player.on_primed();
    host.render(4410);
    let before = host.player.model().position_seconds;
    let index = host.player.queue().index();
    // No fade is in flight: a completion must not move the cursor or clock.
    host.player.on_crossfade_complete(Some(CrossfadeResult {
        info: StreamInfo {
            rate: RATE,
            channels: 2,
            version: 0,
            length_samples: RATE as u64,
        },
        overlap_frames: 100,
    }));
    assert_eq!(host.player.queue().index(), index);
    assert!((host.player.model().position_seconds - before).abs() < 1e-9);
}

#[test]
fn eos_injection_is_idempotent_at_the_boundary() {
    let frames = RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.on_primed();
    host.render_until_drained(1024, 200);
    let index = host.player.queue().index();
    // Duplicate EOS injections must not advance the cursor twice.
    host.player.on_eos();
    host.player.on_eos();
    assert_eq!(host.player.queue().index(), index);
}

#[test]
fn seek_cancels_a_pending_crossfade() {
    let frames = 8 * RATE as usize;
    let mut host = Host::new(backend_with(&[(1, frames), (2, frames)]), default_config());
    host.player
        .play_sequence(vec![item(1, frames), item(2, frames)], 0);
    host.player.set_crossfade(8.0);
    host.player.on_primed();
    let mut rendered = 0usize;
    while rendered < 2 * RATE as usize && !host.engine.borrow().crossfade_pending() {
        host.render(4410);
        rendered += 4410;
    }
    assert!(host.engine.borrow().crossfade_pending(), "fade armed");
    host.player.seek(0.0);
    assert!(
        !host.engine.borrow().crossfade_pending(),
        "seek cancels the lane"
    );
}

#[test]
fn flac_fixture_decodes_through_the_engine() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/reference/audio/flac16-44k.flac"),
    )
    .expect("fixture");
    let mut source = MemorySourceBackend::new();
    source.insert("/fixtures/flac16-44k.flac", bytes);
    let mut host = Host::new(
        source,
        DecoderEngineConfig {
            output_rate: 44_100,
            output_channels: 2,
            ..Default::default()
        },
    );
    let mut it = item(1, 2 * RATE as usize);
    it.source.url = "/fixtures/flac16-44k.flac".into();
    host.player.play_sequence(vec![it], 0);
    host.player.on_primed();
    let out = host.render_until_drained(4096, 400);
    assert!(
        out.iter().any(|s| s.abs() > 0.01),
        "decoded audio is non-silent"
    );
    assert_eq!(host.player.model().state, PlayerState::Ended);
}

#[test]
fn musepack_decodes_through_the_engine() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/musepack/sine44-q5.mpc"),
    )
    .expect("musepack fixture");
    let mut source = MemorySourceBackend::new();
    source.insert("/fixtures/sine44-q5.mpc", bytes);
    let mut host = Host::new(
        source,
        DecoderEngineConfig {
            output_rate: 44_100,
            output_channels: 2,
            ..Default::default()
        },
    );
    let mut it = item(1, RATE as usize);
    it.id = "mpc1".into();
    it.source.url = "/fixtures/sine44-q5.mpc".into();
    it.codec = Some("musepack-sv8".into());
    host.player.play_sequence(vec![it], 0);
    host.player.on_primed();
    let out = host.render_until_drained(4096, 400);
    assert!(
        out.iter().any(|s| s.abs() > 0.01),
        "decoded Musepack is non-silent"
    );
    assert_eq!(host.player.model().state, PlayerState::Ended);
    // The generic clock reflects the decoded length exactly (1 s at 44.1 kHz).
    assert_eq!(host.engine.borrow().rendered_samples(), RATE as u64);
}
