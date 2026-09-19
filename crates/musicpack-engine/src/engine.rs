//! The deterministic decoder-backed [`Engine`] implementation.
//!
//! Owns the current and standby [`DecodeSession`]s, applies gain at the output
//! seam, and performs the equal-power crossfade mix with the reference swap
//! accounting. It never touches a clock, device, network, filesystem, or
//! browser API; a host drives it by pumping and consuming frames.

use musicpack_core::player::engine::{
    CrossfadeResult, CrossfadeStart, Engine, EngineCapabilities, EngineError, EngineResult,
};
use musicpack_core::player::types::{PlaybackItem, StreamInfo, same_item_identity};

use crate::decoder::DecoderFactory;
use crate::mixer::{Mixer, mix_interleaved, rebase_delta};
use crate::session::{CALLBACK_MARGIN_FRAMES, DecodeSession, HIGH_WATER, RING_SECONDS};

/// Engine configuration (output format and capability honesty).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecoderEngineConfig {
    /// Output sample rate the host consumes.
    pub output_rate: u32,
    /// Output channel count (1 or 2).
    pub output_channels: u32,
    /// Main ring capacity in seconds.
    pub ring_seconds: f64,
    /// Standby preload support.
    pub preload_next: bool,
    /// Sample-exact gapless handoff.
    pub sample_accurate_gapless: bool,
    /// Decode-gate (pump) control is meaningful.
    pub decode_gate: bool,
    /// Overlapped crossfade support.
    pub crossfade: bool,
}

impl Default for DecoderEngineConfig {
    fn default() -> Self {
        Self {
            output_rate: 44_100,
            output_channels: 2,
            ring_seconds: RING_SECONDS,
            preload_next: true,
            sample_accurate_gapless: true,
            decode_gate: true,
            crossfade: true,
        }
    }
}

impl DecoderEngineConfig {
    fn ring_capacity_frames(&self) -> usize {
        ((self.output_rate as f64) * self.ring_seconds)
            .round()
            .max(1.0) as usize
    }
}

struct XfadeState {
    mixer: Mixer,
    incoming: DecodeSession,
    incoming_item: PlaybackItem,
}

/// A deterministic, decoder-backed engine.
pub struct DecoderEngine {
    factory: Box<dyn DecoderFactory>,
    config: DecoderEngineConfig,
    current: Option<DecodeSession>,
    current_item: Option<PlaybackItem>,
    standby: Option<(PlaybackItem, DecodeSession)>,
    xfade: Option<XfadeState>,
    crossfade_result: Option<CrossfadeResult>,
    gain: f64,
    pumping: bool,
    backpressured: bool,
    last_error: Option<EngineError>,
}

impl DecoderEngine {
    /// Creates an engine over a decoder factory.
    pub fn new(factory: Box<dyn DecoderFactory>, config: DecoderEngineConfig) -> Self {
        Self {
            factory,
            config,
            current: None,
            current_item: None,
            standby: None,
            xfade: None,
            crossfade_result: None,
            gain: 1.0,
            pumping: false,
            backpressured: false,
            last_error: None,
        }
    }

    /// The configured output rate.
    pub fn output_rate(&self) -> u32 {
        self.config.output_rate
    }

    /// The configured output channel count.
    pub fn output_channels(&self) -> u32 {
        self.config.output_channels
    }

    /// The current item, if any.
    pub fn current_item(&self) -> Option<&PlaybackItem> {
        self.current_item.as_ref()
    }

    /// Takes the most recent decode/source error, if any.
    pub fn take_error(&mut self) -> Option<EngineError> {
        self.last_error.take()
    }

    /// True while a crossfade lane is primed or mixing.
    pub fn crossfade_pending(&self) -> bool {
        self.xfade.is_some()
    }

    /// Takes the completed crossfade result (for `Player::on_crossfade_complete`).
    pub fn take_crossfade_result(&mut self) -> Option<CrossfadeResult> {
        self.crossfade_result.take()
    }

    /// True while the decode pump is backpressured by the high watermark.
    pub fn backpressured(&self) -> bool {
        self.backpressured
    }

    fn open_session(&self, item: &PlaybackItem) -> EngineResult<DecodeSession> {
        let decoder = self.factory.open_decoder(item)?;
        DecodeSession::new(
            decoder,
            self.config.output_rate,
            self.config.output_channels as usize,
            self.config.ring_capacity_frames(),
        )
    }

    fn session_info(&self) -> StreamInfo {
        self.current
            .as_ref()
            .map(|s| s.info())
            .unwrap_or(StreamInfo {
                rate: self.config.output_rate,
                channels: self.config.output_channels,
                version: 0,
                length_samples: 0,
            })
    }

    fn cancel_crossfade(&mut self) {
        self.xfade = None;
        self.crossfade_result = None;
    }

    /// Pumps the current session up to the high watermark (or EOF), recording
    /// any decoder error.
    fn fill_current(&mut self) {
        if !self.pumping {
            return;
        }
        let Some(session) = self.current.as_mut() else {
            return;
        };
        let mut guard = 0usize;
        while guard < 4096 {
            let free = session.ring().free_frames() as usize;
            if free == 0 || session.fill_fraction() >= HIGH_WATER || session.exhausted() {
                break;
            }
            let before = session.ring().available_frames();
            if let Err(e) = session.pump(free) {
                self.last_error = Some(e);
                break;
            }
            if session.ring().available_frames() == before && !session.has_pending() {
                break;
            }
            guard += 1;
        }
        self.backpressured = session.fill_fraction() >= HIGH_WATER;
    }

    /// Consumes `frames` output frames into `dst` (interleaved). Missing data
    /// renders silence (underrun), never an error.
    pub fn consume(&mut self, frames: usize, dst: &mut [f32]) -> usize {
        let channels = self.config.output_channels as usize;
        let n = frames.min(dst.len() / channels);
        if n == 0 {
            return 0;
        }
        let mut written = 0usize;
        if self.xfade.is_some() {
            while written < n {
                let finished = {
                    let state = self.xfade.as_mut().expect("xfade present");
                    match state.mixer.next_gains() {
                        Some((out_gain, in_gain)) => {
                            let mut oframe = [0.0f32; 8];
                            let mut iframe = [0.0f32; 8];
                            let has_out = self
                                .current
                                .as_mut()
                                .map(|s| {
                                    s.ring_mut().read_interleaved(&mut oframe[..channels], 1) == 1
                                })
                                .unwrap_or(false);
                            let has_in = state
                                .incoming
                                .ring_mut()
                                .read_interleaved(&mut iframe[..channels], 1)
                                == 1;
                            let out = &mut dst[written * channels..(written + 1) * channels];
                            mix_interleaved(
                                out,
                                if has_out {
                                    Some(&oframe[..channels])
                                } else {
                                    None
                                },
                                if has_in {
                                    Some(&iframe[..channels])
                                } else {
                                    None
                                },
                                out_gain,
                                in_gain,
                            );
                            written += 1;
                            false
                        }
                        None => true,
                    }
                };
                if finished {
                    self.finish_crossfade_swap();
                    break;
                }
            }
        }
        if written < n {
            self.fill_current();
            if let Some(session) = self.current.as_mut() {
                written += session
                    .ring_mut()
                    .read_interleaved(&mut dst[written * channels..], n - written);
            }
        }
        let gain = self.gain as f32;
        for sample in &mut dst[..written * channels] {
            *sample *= gain;
        }
        for sample in &mut dst[written * channels..n * channels] {
            *sample = 0.0;
        }
        n
    }

    fn finish_crossfade_swap(&mut self) {
        let Some(state) = self.xfade.take() else {
            return;
        };
        let outgoing_rendered = self
            .current
            .as_ref()
            .map(|s| s.ring().rendered_frames())
            .unwrap_or(0);
        let incoming_rendered = state.incoming.ring().rendered_frames();
        let facts = state.mixer.swap_facts(outgoing_rendered, incoming_rendered);
        let delta = rebase_delta(&facts);
        let mut incoming = state.incoming;
        if delta > 0 {
            incoming.ring_mut().continue_playhead_from(delta);
        }
        let info = incoming.info();
        self.current_item = Some(state.incoming_item.clone());
        self.current = Some(incoming);
        self.crossfade_result = Some(CrossfadeResult {
            info,
            overlap_frames: facts.overlap_frames,
        });
        self.standby = None;
        self.backpressured = false;
    }
}

impl Engine for DecoderEngine {
    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            preload_next: self.config.preload_next,
            sample_accurate_gapless: self.config.sample_accurate_gapless,
            decode_gate: self.config.decode_gate,
            crossfade: self.config.crossfade,
        }
    }

    fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo> {
        self.cancel_crossfade();
        self.standby = None;
        self.current = None;
        self.current_item = None;
        self.backpressured = false;
        self.last_error = None;
        let session = self.open_session(item)?;
        self.current_item = Some(item.clone());
        self.current = Some(session);
        self.pumping = false;
        Ok(self.session_info())
    }

    fn play(&mut self) -> EngineResult<()> {
        self.pumping = true;
        Ok(())
    }

    fn pause(&mut self) {
        self.pumping = false;
    }

    fn seek(&mut self, samples: u64) {
        let Some(item) = self.current_item.clone() else {
            return;
        };
        // `Player::load` always issues a seek (to 0 on a fresh load). When the
        // current session has not consumed anything yet, that is a no-op; only
        // a real reposition (or a forward skip) reopens the decoder.
        if samples == 0
            && self.rendered_samples() == 0
            && self
                .current
                .as_ref()
                .map(|s| !s.has_pending())
                .unwrap_or(false)
        {
            return;
        }
        self.cancel_crossfade();
        let channels = self.config.output_channels as usize;
        let mut session = match self.open_session(&item) {
            Ok(s) => s,
            Err(e) => {
                self.last_error = Some(e);
                return;
            }
        };
        let mut remaining = samples;
        let mut scratch = vec![0.0f32; 4096 * channels];
        let mut guard = 0usize;
        while remaining > 0 && guard < 1_000_000 {
            // A read/pump failure (e.g. a failed range fetch) must abort the
            // skip instead of being discarded: otherwise the loop spins to its
            // guard limit re-issuing the same failing read, and the seek never
            // surfaces an error. Leave `current` untouched so the host reports
            // the recorded error via `take_error()`.
            if let Err(e) = session.pump(session.ring().free_frames() as usize) {
                self.last_error = Some(e);
                return;
            }
            let available = session.ring().available_frames();
            if available == 0 {
                if session.exhausted() {
                    break;
                }
                guard += 1;
                continue;
            }
            let take = (remaining.min(available) as usize).min(4096);
            session
                .ring_mut()
                .read_interleaved(&mut scratch[..take * channels], take);
            remaining -= take as u64;
            guard += 1;
        }
        // The ring playhead restarts at the seek target: the player carries
        // the absolute position through its reset offset, exactly as the
        // reference engine resets the worklet ring and sets `resetBase`.
        session.rebase_playhead();
        self.current = Some(session);
        self.backpressured = false;
    }

    fn set_gain(&mut self, linear: f64) {
        self.gain = linear;
    }

    fn rendered_samples(&self) -> u64 {
        self.current
            .as_ref()
            .map(|s| s.ring().rendered_frames())
            .unwrap_or(0)
    }

    fn close(&mut self) {
        self.cancel_crossfade();
        self.standby = None;
        self.current = None;
        self.current_item = None;
        self.pumping = false;
        self.backpressured = false;
    }

    fn prepare_next(&mut self, item: &PlaybackItem) -> Option<StreamInfo> {
        if !self.config.preload_next {
            return None;
        }
        match self.open_session(item) {
            Ok(session) => {
                let info = session.info();
                self.standby = Some((item.clone(), session));
                Some(info)
            }
            Err(_) => {
                self.standby = None;
                None
            }
        }
    }

    fn advance(&mut self, expected: Option<&PlaybackItem>) -> Option<StreamInfo> {
        let expected = expected?;
        let (item, session) = self.standby.take()?;
        if !same_item_identity(&item, expected) {
            self.standby = Some((item, session));
            return None;
        }
        let info = session.info();
        self.current_item = Some(item);
        self.current = Some(session);
        self.backpressured = false;
        Some(info)
    }

    fn begin_crossfade(&mut self, next: &PlaybackItem, fade_seconds: f64) -> CrossfadeStart {
        if !self.config.crossfade || self.xfade.is_some() || self.current.is_none() {
            return CrossfadeStart::Declined;
        }
        let Some((standby_item, mut incoming)) = self.standby.take() else {
            return CrossfadeStart::Declined;
        };
        if !same_item_identity(&standby_item, next) {
            self.standby = Some((standby_item, incoming));
            return CrossfadeStart::Declined;
        }
        let clamped = fade_seconds.clamp(0.25, 15.0);
        let fade_frames = ((clamped * self.config.output_rate as f64).round() as u64).max(1);
        let declared = incoming.info().length_samples;
        let wanted = fade_frames.saturating_add(CALLBACK_MARGIN_FRAMES);
        let needed = if declared == 0 {
            wanted
        } else {
            declared.min(wanted)
        };
        let mut guard = 0usize;
        while incoming.ring().available_frames() < needed && !incoming.exhausted() && guard < 8192 {
            let free = incoming.ring().free_frames() as usize;
            if free == 0 {
                break;
            }
            let before = incoming.ring().available_frames();
            if incoming.pump(free).is_err() {
                self.standby = Some((standby_item, incoming));
                return CrossfadeStart::Declined;
            }
            if incoming.ring().available_frames() == before && !incoming.has_pending() {
                break;
            }
            guard += 1;
        }
        let mut mixer = Mixer::new(fade_frames);
        mixer.start(
            self.current
                .as_ref()
                .map(|s| s.ring().rendered_frames())
                .unwrap_or(0),
        );
        self.xfade = Some(XfadeState {
            mixer,
            incoming,
            incoming_item: standby_item,
        });
        CrossfadeStart::Pending
    }

    fn is_output_drained(&self) -> bool {
        self.current
            .as_ref()
            .map(|s| s.exhausted())
            .unwrap_or(false)
    }

    fn start_pumping(&mut self) {
        self.pumping = true;
    }

    fn pause_pumping(&mut self) {
        self.pumping = false;
    }
}
