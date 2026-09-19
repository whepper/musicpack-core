//! Level-4 host integration: source → decoder → engine → `consume()` → host
//! output, through the reusable [`Host`] the browser/native adapter would use.
//!
//! These tests cover the Phase 12 integration scenario list. Deterministic
//! engine behavior is asserted here and in `musicpack-engine`; nothing here
//! depends on a real clock or device.

use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;

use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::format::mpak::{MemorySource, PackMember, PackSource, write_mpak};
use musicpack_core::player::engine::{CrossfadeResult, Engine};
use musicpack_core::player::player::PlayerState;
use musicpack_core::player::types::{PlaybackItem, PlaybackSource, SourceKind, StreamInfo};
use musicpack_core::policy::{AudioPreference, Candidate};
use musicpack_core::storage::mpak::MpakBackend;
use musicpack_host::{
    Host, HostConfig, MemorySourceBackend, PackageSourceBackend, SourceCandidate, TrackSources,
};

const RATE: u32 = 44_100;

fn build_wav(channels: u16, frames: usize, tone: f32) -> Vec<u8> {
    let block = channels * 2;
    let data = (frames * channels as usize * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let v = (i as f32 * tone).sin() * 0.5 * 32767.0;
        for _ in 0..channels {
            out.extend_from_slice(&(v as i16).to_le_bytes());
        }
    }
    out
}

fn url(n: u64) -> String {
    format!("/host/{n}.wav")
}

fn item(n: u64, frames: usize) -> PlaybackItem {
    PlaybackItem {
        id: format!("t{n}"),
        track_id: n as i64,
        source: PlaybackSource {
            kind: SourceKind::HttpRange,
            url: url(n),
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

fn backend(tracks: &[(u64, usize)]) -> MemorySourceBackend {
    let mut backend = MemorySourceBackend::new();
    for &(n, frames) in tracks {
        backend.insert(url(n), build_wav(2, frames, 0.01 + n as f32 * 0.001));
    }
    backend
}

fn host(tracks: &[(u64, usize)]) -> Host {
    Host::new(HostConfig::default(), Box::new(backend(tracks)))
}

fn default_config() -> HostConfig {
    HostConfig::default()
}

#[test]
fn load_play_reaches_eos_and_ends() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    assert_eq!(host.player().model().state, PlayerState::Playing);
    let out = host.render_until_drained(1024, 200);
    assert!(out.iter().all(|s| s.is_finite()));
    assert_eq!(host.player().model().state, PlayerState::Ended);
}

#[test]
fn pause_freezes_and_resume_continues() {
    let frames = 2 * RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.render(4410);
    host.player_mut().pause();
    assert_eq!(host.player().model().state, PlayerState::Paused);
    // A paused host suspends the device clock: no pull, so the position is
    // frozen even though the ring still holds buffered frames.
    let paused = host.player().model().position_seconds;
    assert!((paused - 0.1).abs() < 1e-3);
    host.player_mut().resume();
    host.render(4410);
    assert!(host.player().model().position_seconds > paused);
}

#[test]
fn seek_repositions_through_the_host() {
    let frames = 4 * RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.render(4410);
    host.player_mut().seek(2.0);
    assert!((host.player().model().position_seconds - 2.0).abs() < 0.02);
    host.render(4410);
    assert!((host.player().model().position_seconds - 2.1).abs() < 0.02);
}

#[test]
fn underrun_renders_silence_after_drain() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.render_until_drained(1024, 200);
    assert_eq!(host.player().model().state, PlayerState::Ended);
    let after = host.render(1024);
    assert!(after.iter().all(|s| *s == 0.0));
    assert!(host.engine().borrow_mut().take_error().is_none());
}

#[test]
fn gapless_advance_promotes_the_standby() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames), (2, frames)]);
    host.load_and_play(vec![item(1, frames), item(2, frames)], 0);
    host.render_until_drained(1024, 200);
    assert_eq!(host.player().queue().index(), 1);
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t2");
    host.render_until_drained(1024, 400);
    assert_eq!(host.player().model().state, PlayerState::Ended);
}

#[test]
fn crossfade_mixes_with_album_clock_continuity() {
    let frames = 8 * RATE as usize;
    let mut host = host(&[(1, frames), (2, frames)]);
    host.load_and_play(vec![item(1, frames), item(2, frames)], 0);
    host.player_mut().set_crossfade(4.0);
    let mut rendered = 0usize;
    while rendered < 10 * RATE as usize && host.player().queue().index() == 0 {
        host.render(4410);
        rendered += 4410;
    }
    assert_eq!(host.player().queue().index(), 1);
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t2");
    let start = host.player().model().current_track_start_seconds;
    assert!((start - 4.0).abs() < 0.2, "start {start}");
    assert!((host.player().model().duration_seconds - 12.0).abs() < 0.2);
}

#[test]
fn short_outgoing_track_still_advances() {
    let frames = RATE as usize / 2;
    let mut host = host(&[(1, frames), (2, frames)]);
    host.load_and_play(vec![item(1, frames), item(2, frames)], 0);
    host.player_mut().set_crossfade(4.0);
    host.render_until_drained(1024, 800);
    assert_eq!(host.player().model().state, PlayerState::Ended);
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn short_incoming_track_still_advances() {
    let long = 8 * RATE as usize;
    let short = RATE as usize / 2;
    let mut host = host(&[(1, long), (2, short)]);
    host.load_and_play(vec![item(1, long), item(2, short)], 0);
    host.player_mut().set_crossfade(4.0);
    let mut rendered = 0usize;
    while rendered < 12 * RATE as usize && host.player().model().state != PlayerState::Ended {
        host.render(4096);
        rendered += 4096;
    }
    assert_eq!(host.player().model().state, PlayerState::Ended);
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn pause_during_crossfade_keeps_the_lane() {
    let frames = 8 * RATE as usize;
    let mut host = host(&[(1, frames), (2, frames)]);
    host.load_and_play(vec![item(1, frames), item(2, frames)], 0);
    host.player_mut().set_crossfade(8.0);
    let mut rendered = 0usize;
    while rendered < RATE as usize && !host.engine().borrow().crossfade_pending() {
        host.render(4410);
        rendered += 4410;
    }
    assert!(host.engine().borrow().crossfade_pending());
    host.player_mut().pause();
    assert!(
        host.engine().borrow().crossfade_pending(),
        "pause keeps lane"
    );
    host.player_mut().resume();
    host.render_until_drained(4096, 500);
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn seek_cancels_a_pending_crossfade() {
    let frames = 8 * RATE as usize;
    let mut host = host(&[(1, frames), (2, frames)]);
    host.load_and_play(vec![item(1, frames), item(2, frames)], 0);
    host.player_mut().set_crossfade(8.0);
    let mut rendered = 0usize;
    while rendered < 2 * RATE as usize && !host.engine().borrow().crossfade_pending() {
        host.render(4410);
        rendered += 4410;
    }
    assert!(host.engine().borrow().crossfade_pending());
    host.player_mut().seek(0.0);
    assert!(!host.engine().borrow().crossfade_pending());
}

#[test]
fn stale_crossfade_completion_is_ignored() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.render(4410);
    let before = host.player().model().position_seconds;
    let index = host.player().queue().index();
    host.player_mut()
        .on_crossfade_complete(Some(CrossfadeResult {
            info: StreamInfo {
                rate: RATE,
                channels: 2,
                version: 0,
                length_samples: RATE as u64,
            },
            overlap_frames: 100,
        }));
    assert_eq!(host.player().queue().index(), index);
    assert!((host.player().model().position_seconds - before).abs() < 1e-9);
}

#[test]
fn gain_is_applied_by_the_engine() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.render(4410);
    let loud = host
        .render(4410)
        .iter()
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    host.player_mut().set_volume(0.0);
    let quiet = host
        .render(4410)
        .iter()
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    assert!(loud > 0.0);
    assert_eq!(quiet, 0.0);
}

#[test]
fn teardown_clears_host_and_engine() {
    let frames = RATE as usize;
    let mut host = host(&[(1, frames)]);
    host.load_and_play(vec![item(1, frames)], 0);
    host.player_mut().teardown();
    assert_eq!(host.player().model().state, PlayerState::Idle);
    assert!(host.player().model().current.is_none());
    assert!(!host.engine().borrow().crossfade_pending());
}

// ---- MPAK-backed playback --------------------------------------------------

struct SingleMember {
    manifest: Vec<u8>,
    member: PackMember,
    bytes: Vec<u8>,
}

impl PackSource for SingleMember {
    fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }
    fn members(&self) -> &[PackMember] {
        std::slice::from_ref(&self.member)
    }
    fn member_size(&self, _path: &str) -> Result<u64, musicpack_core::error::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_member(&self, _path: &str) -> Result<Box<dyn Read + '_>, musicpack_core::error::Error> {
        Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
    }
}

fn build_container(member_path: &str, wav: &[u8]) -> Vec<u8> {
    let sha = sha256_hex(wav);
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"Host","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"t","audio":{{"path":"{member_path}","sha256":"{sha}"}}}}]}}]}}"#
    )
    .into_bytes();
    let source = SingleMember {
        manifest,
        member: PackMember {
            path: member_path.into(),
            sha256_hex: sha,
        },
        bytes: wav.to_vec(),
    };
    let mut out = Vec::new();
    write_mpak(&source, &mut out).expect("pack");
    out
}

#[test]
fn mpak_backed_playback_through_the_host() {
    let container = build_container("audio/01.wav", &build_wav(2, RATE as usize, 0.03));
    let backend =
        MpakBackend::open(Arc::new(MemorySource::new(container))).expect("open container");
    let mut host = Host::new(
        default_config(),
        Box::new(PackageSourceBackend::new(Rc::new(backend))),
    );
    let mut it = item(1, RATE as usize);
    it.source.kind = SourceKind::Other("mpak".into());
    it.source.url = "audio/01.wav".into();
    host.load_and_play(vec![it], 0);
    let out = host.render_until_drained(4096, 400);
    assert!(
        out.iter().any(|s| s.abs() > 0.01),
        "decoded member is non-silent"
    );
    assert_eq!(host.player().model().state, PlayerState::Ended);
}

// ---- representation selection → source → playback --------------------------

fn sources() -> TrackSources {
    TrackSources {
        primary: SourceCandidate {
            id: 0.0,
            url: "/primary-musepack.bin".into(),
            byte_size: Some(1024),
            codec: Some("musepack-sv8".into()),
            mime_type: Some("audio/musepack".into()),
        },
        representations: vec![
            SourceCandidate {
                id: 10.0,
                url: "/rep10.wav".into(),
                byte_size: Some(2048),
                codec: Some("flac".into()),
                mime_type: Some("audio/flac".into()),
            },
            SourceCandidate {
                id: 11.0,
                url: "/rep11.wav".into(),
                byte_size: Some(4096),
                codec: Some("wav".into()),
                mime_type: Some("audio/wav".into()),
            },
        ],
    }
}

fn wants_flac(c: &Candidate<'_>) -> bool {
    c.codec
        .map(|s| s.eq_ignore_ascii_case("flac"))
        .unwrap_or(false)
}

#[test]
fn representation_selection_is_deterministic_and_documented() {
    let sources = sources();
    let accept_all = |_: &Candidate<'_>| true;
    // Default -> primary when the primary is playable.
    let primary = sources.select(None, &accept_all);
    assert_eq!(primary.representation_id, None);
    assert_eq!(primary.url, "/primary-musepack.bin");
    assert_eq!(primary.item_id(14), "t14");
    // Default -> rescue to the first playable alternate when the primary is
    // not playable (the reference's availability fallback).
    let rescued = sources.select(None, &wants_flac);
    assert_eq!(rescued.representation_id, Some(10.0));
    assert_eq!(rescued.url, "/rep10.wav");
    assert_eq!(rescued.item_id(14), "t14r10");
    // Codec preference -> first playable flac.
    let flac = sources.select(
        Some(&AudioPreference::Codec {
            codec: "flac".into(),
        }),
        &wants_flac,
    );
    assert_eq!(flac.representation_id, Some(10.0));
    assert_eq!(flac.url, "/rep10.wav");
    // Lossless -> first lossless (flac at index 0).
    let lossless = sources.select(Some(&AudioPreference::Lossless), &accept_all);
    assert_eq!(lossless.representation_id, Some(10.0));
    // Explicit id.
    let explicit = sources.select(
        Some(&AudioPreference::Representation { id: 11.0 }),
        &accept_all,
    );
    assert_eq!(explicit.representation_id, Some(11.0));
    assert_eq!(explicit.url, "/rep11.wav");
}

#[test]
fn selected_representation_plays_through_the_engine() {
    let sources = sources();
    // The primary musepack is unplayable here; a flac alternate rescues it.
    let can_play = |c: &Candidate<'_>| {
        c.codec
            .map(|codec| codec.eq_ignore_ascii_case("flac"))
            .unwrap_or(false)
    };
    let selected = sources.select(None, &can_play);
    assert_eq!(selected.representation_id, Some(10.0));

    // Serve the selected source and play it.
    let mut backend = MemorySourceBackend::new();
    backend.insert(selected.url.clone(), build_wav(2, RATE as usize, 0.02));
    let mut host = Host::new(default_config(), Box::new(backend));
    let mut it = item(9, RATE as usize);
    it.id = selected.item_id(14);
    it.source.url = selected.url.clone();
    it.source.byte_size = selected.byte_size;
    it.codec = selected.codec.clone();
    it.mime_type = selected.mime_type.clone();
    host.load_and_play(vec![it], 0);
    let out = host.render_until_drained(4096, 400);
    assert!(
        out.iter().any(|s| s.abs() > 0.01),
        "selected rep is audible"
    );
    assert_eq!(host.player().model().current.as_ref().unwrap().id, "t14r10");
}

// ---- Musepack through the host (direct, MPAK, cross-codec) ------------------

fn read_fixture(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn host_with_musepack() -> Host {
    let mut backend = MemorySourceBackend::new();
    backend.insert(
        "/mus.mpc",
        read_fixture("tests/fixtures/musepack/sine44-q5.mpc"),
    );
    Host::new(default_config(), Box::new(backend))
}

#[test]
fn musepack_plays_through_the_host() {
    let mut host = host_with_musepack();
    let mut it = item(1, RATE as usize);
    it.id = "mpc1".into();
    it.source.url = "/mus.mpc".into();
    it.codec = Some("musepack-sv8".into());
    host.load_and_play(vec![it], 0);
    let out = host.render_until_drained(4096, 400);
    assert!(out.iter().any(|s| s.abs() > 0.01), "Musepack is audible");
    assert_eq!(host.player().model().state, PlayerState::Ended);
    assert_eq!(host.engine().borrow().rendered_samples(), RATE as u64);
}

#[test]
fn mpak_backed_musepack_plays_through_the_host() {
    let container = build_container(
        "audio/01.mpc",
        &read_fixture("tests/fixtures/musepack/sine44-q5.mpc"),
    );
    let backend =
        MpakBackend::open(Arc::new(MemorySource::new(container))).expect("open container");
    let mut host = Host::new(
        default_config(),
        Box::new(PackageSourceBackend::new(Rc::new(backend))),
    );
    let mut it = item(1, RATE as usize);
    it.id = "mpak-mpc".into();
    it.source.kind = SourceKind::Other("mpak".into());
    it.source.url = "audio/01.mpc".into();
    it.codec = Some("musepack-sv8".into());
    host.load_and_play(vec![it], 0);
    let out = host.render_until_drained(4096, 400);
    assert!(
        out.iter().any(|s| s.abs() > 0.01),
        "MPAK Musepack is audible"
    );
    assert_eq!(host.player().model().state, PlayerState::Ended);
}

/// Builds a host with a Musepack, a FLAC and a WAV source available.
fn cross_codec_host() -> Host {
    let mut backend = MemorySourceBackend::new();
    backend.insert(
        "/c/mus.mpc",
        read_fixture("tests/fixtures/musepack/sine44-q5.mpc"),
    );
    backend.insert(
        "/c/tone.flac",
        read_fixture("fixtures/reference/audio/flac16-44k.flac"),
    );
    backend.insert("/c/tone.wav", build_wav(2, RATE as usize, 0.02));
    Host::new(default_config(), Box::new(backend))
}

fn codec_item(id: &str, url: &str, codec: &str, mime: &str) -> PlaybackItem {
    let mut it = item(1, RATE as usize);
    // Distinct track identity: the player keys its length map by `track_id`.
    it.track_id = id
        .bytes()
        .fold(0i64, |acc, b| (acc * 31 + b as i64) % 1_000_003);
    it.id = id.into();
    it.source.url = url.into();
    it.codec = Some(codec.into());
    it.mime_type = Some(mime.into());
    it
}

fn cross_codec_sequence(a: (&str, &str, &str, &str), b: (&str, &str, &str, &str)) -> Host {
    let mut host = cross_codec_host();
    let first = codec_item(a.0, a.1, a.2, a.3);
    let second = codec_item(b.0, b.1, b.2, b.3);
    host.load_and_play(vec![first, second], 0);
    host
}

#[test]
fn cross_codec_transitions_advance_generically() {
    let scenarios = [
        (
            ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
            ("m2", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
        ),
        (
            ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
            ("f1", "/c/tone.flac", "flac", "audio/flac"),
        ),
        (
            ("f1", "/c/tone.flac", "flac", "audio/flac"),
            ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
        ),
        (
            ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
            ("w1", "/c/tone.wav", "wav", "audio/wav"),
        ),
        (
            ("w1", "/c/tone.wav", "wav", "audio/wav"),
            ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
        ),
    ];
    for (first, second) in scenarios {
        let mut host = cross_codec_sequence(first, second);
        let out = host.render_until_drained(4096, 800);
        assert!(
            out.iter().any(|s| s.abs() > 0.01),
            "{} -> {}: audible",
            first.0,
            second.0
        );
        assert_eq!(
            host.player().queue().index(),
            1,
            "{} -> {}: advanced exactly once",
            first.0,
            second.0
        );
        assert!(
            host.player()
                .model()
                .current
                .as_ref()
                .map(|i| i.id == second.0)
                .unwrap_or(false),
            "{} -> {}: current is the second item (state={:?} index={} pos={} current={:?})",
            first.0,
            second.0,
            host.player().model().state,
            host.player().queue().index(),
            host.player().model().position_seconds,
            host.player().model().current.as_ref().map(|i| i.id.clone()),
        );
        assert_eq!(
            host.player().model().state,
            PlayerState::Ended,
            "{} -> {}: reached the end",
            first.0,
            second.0
        );
    }
}

#[test]
fn cross_codec_crossfade_uses_only_generic_machinery() {
    let mut host = cross_codec_sequence(
        ("m1", "/c/mus.mpc", "musepack-sv8", "audio/musepack"),
        ("f1", "/c/tone.flac", "flac", "audio/flac"),
    );
    host.player_mut().set_crossfade(4.0);
    // The fixtures are short; the fade triggers early and completes.
    let mut rendered = 0usize;
    while rendered < 12 * RATE as usize && host.player().model().state != PlayerState::Ended {
        host.render(4096);
        rendered += 4096;
    }
    assert_eq!(host.player().model().state, PlayerState::Ended);
    assert_eq!(
        host.player().model().current.as_ref().unwrap().id,
        "f1",
        "crossfade advanced to the incoming FLAC"
    );
}
