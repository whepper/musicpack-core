//! Fuzz target: the Phase 10 player core.
//!
//! Properties under test:
//!
//! - no panic on arbitrary bytes (hostile snapshots, queue op sequences,
//!   command orderings, RNG values including exactly `1.0`);
//! - termination;
//! - bounded resources (queue/history caps, snapshot item count);
//! - frame/time arithmetic never overflows into invalid state.
#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use musicpack_core::player::engine::{Engine, EngineCapabilities, EngineResult};
use musicpack_core::player::player::{Player, PlayerOptions, PlayerPorts};
use musicpack_core::player::queue::QueueModel;
use musicpack_core::player::snapshot::{decode_snapshot, encode_snapshot};
use musicpack_core::player::types::{
    EngineKind, PlaybackItem, PlaybackSource, SourceKind, StreamInfo,
};

struct NullEngine;

impl Engine for NullEngine {
    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            preload_next: true,
            sample_accurate_gapless: false,
            decode_gate: true,
            crossfade: true,
        }
    }
    fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo> {
        Ok(StreamInfo {
            rate: 44_100,
            channels: 2,
            version: 0,
            length_samples: item.duration_hint_seconds.unwrap_or(0.0) as u64 * 44_100,
        })
    }
    fn play(&mut self) -> EngineResult<()> {
        Ok(())
    }
    fn pause(&mut self) {}
    fn seek(&mut self, _samples: u64) {}
    fn set_gain(&mut self, _linear: f64) {}
    fn rendered_samples(&self) -> u64 {
        0
    }
    fn close(&mut self) {}
}

fn item(n: i64) -> PlaybackItem {
    PlaybackItem {
        id: format!("t{n}"),
        track_id: n,
        source: PlaybackSource {
            kind: SourceKind::HttpRange,
            url: format!("/t/{n}"),
            byte_size: None,
        },
        duration_hint_seconds: Some(1.0),
        title: "t".into(),
        artist: "a".into(),
        album_title: "al".into(),
        edition: None,
        artwork_url: None,
        loudness: None,
        album_loudness: None,
        codec: None,
        mime_type: None,
        extra: Vec::new(),
    }
}

fuzz_target!(|data: &[u8]| {
    // Hostile snapshot decode/encode round trip.
    let raw = String::from_utf8_lossy(data);
    if let Some(snapshot) = decode_snapshot(&raw) {
        let encoded = encode_snapshot(&snapshot);
        let _ = decode_snapshot(&encoded);
    }

    // Queue driven by arbitrary bytes, with an adversarial RNG that always
    // returns 1.0 (the clamp path).
    let mut queue = QueueModel::new(Box::new(|| 1.0));
    queue
        .play_sequence((1..=8).map(item).collect(), 0)
        .ok();
    for (i, byte) in data.iter().enumerate() {
        match byte % 8 {
            0 => {
                queue.next();
            }
            1 => {
                queue.previous();
            }
            2 => queue.set_shuffle(i % 2 == 0),
            3 => queue.set_repeat(match byte % 3 {
                0 => musicpack_core::player::types::RepeatMode::Off,
                1 => musicpack_core::player::types::RepeatMode::One,
                _ => musicpack_core::player::types::RepeatMode::All,
            }),
            4 => queue.move_item((*byte % 9) as i64, ((byte / 2) % 9) as i64),
            5 => queue.remove_at((*byte % 9) as i64),
            6 => queue.play_next(item(*byte as i64)),
            _ => queue.enqueue(item(*byte as i64)),
        }
    }

    // Player driven by arbitrary command order.
    let ports = PlayerPorts {
        engine_factory: Box::new(|_kind: EngineKind| Box::new(NullEngine) as Box<dyn Engine>),
        resolve_kind: Box::new(|_item: &PlaybackItem| Ok(EngineKind::Musepack)),
        plan_transition: None,
    };
    let mut player = Player::new(
        QueueModel::new(Box::new(|| 0.5)),
        ports,
        PlayerOptions::default(),
    );
    if let Some(snapshot) = decode_snapshot(&raw) {
        player.restore(&snapshot);
    }
    player.play_sequence((1..=4).map(item).collect(), 0);
    for byte in data.iter().take(256) {
        match byte % 11 {
            0 => {
                player.on_primed();
            }
            1 => {
                player.on_buffering();
            }
            2 => {
                player.on_tick();
            }
            3 => {
                player.on_eos();
            }
            4 => {
                player.pause();
            }
            5 => {
                player.resume();
            }
            6 => {
                player.next();
            }
            7 => {
                player.previous();
            }
            8 => {
                player.seek(*byte as f64 * 1e6);
            }
            9 => {
                player.set_volume(*byte as f64 / 255.0);
            }
            _ => {
                player.on_queue_changed();
            }
        }
        let _ = player.take_snapshot();
    }
    player.teardown();
    let _ = Cursor::new(data.to_vec());
});
