//! Player-core behavioral tests.
//!
//! The TypeScript `web/player-core` unit suites are the behavioral oracle.
//! Pure-logic suites (transition, gain, snapshot, queue/order) are ported
//! directly here; player scenarios are driven through a scripted fake engine
//! that mirrors the TypeScript tests' `FakeBackend`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use musicpack_core::player::engine::{
    CrossfadeResult, CrossfadeStart, Engine, EngineCapabilities, EngineError, EngineResult,
};
use musicpack_core::player::events::PlayerEvent;
use musicpack_core::player::player::{
    END_TOLERANCE_SAMPLES, Player, PlayerOptions, PlayerPorts, PlayerState,
};
use musicpack_core::player::queue::QueueModel;
use musicpack_core::player::snapshot::{
    SNAPSHOT_VERSION, clamp_index, decode_snapshot, encode_snapshot,
};
use musicpack_core::player::transition::{
    BoundaryProfile, TransitionPlan, TransitionQuery, is_clean_loud_ending, is_fast_attack,
    plan_transition, trailing_silence_seconds,
};
use musicpack_core::player::types::{
    EngineKind, NormalizationMode, PlaybackItem, PlaybackSource, RepeatMode, SourceKind,
    StreamInfo, TrackLoudness, same_item_identity,
};
use musicpack_core::player::{combined_gain, db_to_linear, normalization_gain};

// ---------------------------------------------------------------------------
// Fixtures and the scripted fake engine
// ---------------------------------------------------------------------------

const RATE: u32 = 44_100;

fn item(n: i64) -> PlaybackItem {
    PlaybackItem {
        id: format!("t{n}"),
        track_id: n,
        source: PlaybackSource {
            kind: SourceKind::HttpRange,
            url: format!("/api/v1/tracks/{n}/audio"),
            byte_size: Some(100),
        },
        duration_hint_seconds: None,
        title: format!("T{n}"),
        artist: "A".into(),
        album_title: "AL".into(),
        edition: None,
        artwork_url: None,
        loudness: None,
        album_loudness: None,
        codec: Some("musepack-sv8".into()),
        mime_type: None,
        extra: Vec::new(),
    }
}

fn mk(n: i64, secs: f64) -> PlaybackItem {
    let mut it = item(n);
    it.duration_hint_seconds = Some(secs);
    it
}

#[derive(Clone, Copy, PartialEq)]
enum FadeMode {
    Immediate,
    Deferred,
    Null,
}

struct Shared {
    caps: EngineCapabilities,
    default_len: u64,
    length_overrides: HashMap<String, u64>,
    rendered: u64,
    opened: Vec<String>,
    prepared: Vec<String>,
    seeks: Vec<u64>,
    gains: Vec<f64>,
    plays: u64,
    paused: bool,
    closed: bool,
    fail_next_prepare: bool,
    fail_play_once: Option<String>,
    fade_mode: FadeMode,
    overlap_frames: u64,
    standby_item: Option<PlaybackItem>,
    standby_info: Option<StreamInfo>,
    begin_crossfade_calls: Vec<(String, f64)>,
    standby_id_override: Option<String>,
    plan: Option<TransitionPlan>,
    plan_calls: Vec<TransitionQuery>,
}

impl Shared {
    fn new(caps: EngineCapabilities, fade_mode: FadeMode, default_secs: f64) -> Self {
        Self {
            caps,
            default_len: (default_secs * RATE as f64) as u64,
            length_overrides: HashMap::new(),
            rendered: 0,
            opened: Vec::new(),
            prepared: Vec::new(),
            seeks: Vec::new(),
            gains: Vec::new(),
            plays: 0,
            paused: false,
            closed: false,
            fail_next_prepare: false,
            fail_play_once: None,
            fade_mode,
            overlap_frames: 0,
            standby_item: None,
            standby_info: None,
            begin_crossfade_calls: Vec::new(),
            standby_id_override: None,
            plan: None,
            plan_calls: Vec::new(),
        }
    }

    fn length_for(&self, url: &str) -> u64 {
        self.length_overrides
            .get(url)
            .copied()
            .unwrap_or(self.default_len)
    }
}

struct FakeEngine {
    shared: Rc<RefCell<Shared>>,
}

fn length_for_item(s: &Shared, it: &PlaybackItem) -> u64 {
    if let Some(len) = s.length_overrides.get(&it.source.url) {
        return *len;
    }
    match it.duration_hint_seconds {
        Some(secs) if secs.is_finite() && secs > 0.0 => (secs * RATE as f64) as u64,
        _ => s.default_len,
    }
}

impl FakeEngine {
    fn info_for(&self, it: &PlaybackItem) -> StreamInfo {
        StreamInfo {
            rate: RATE,
            channels: 2,
            version: 0,
            length_samples: length_for_item(&self.shared.borrow(), it),
        }
    }
}

impl Engine for FakeEngine {
    fn capabilities(&self) -> EngineCapabilities {
        self.shared.borrow().caps
    }

    fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo> {
        let info = self.info_for(item);
        let mut s = self.shared.borrow_mut();
        s.opened.push(item.source.url.clone());
        s.rendered = 0;
        Ok(info)
    }

    fn play(&mut self) -> EngineResult<()> {
        let mut s = self.shared.borrow_mut();
        s.plays += 1;
        if let Some(msg) = s.fail_play_once.take() {
            return Err(EngineError(msg));
        }
        Ok(())
    }

    fn pause(&mut self) {
        self.shared.borrow_mut().paused = true;
    }

    fn seek(&mut self, samples: u64) {
        let mut s = self.shared.borrow_mut();
        s.seeks.push(samples);
        s.rendered = 0;
    }

    fn set_gain(&mut self, linear: f64) {
        self.shared.borrow_mut().gains.push(linear);
    }

    fn rendered_samples(&self) -> u64 {
        self.shared.borrow().rendered
    }

    fn close(&mut self) {
        self.shared.borrow_mut().closed = true;
    }

    fn prepare_next(&mut self, item: &PlaybackItem) -> Option<StreamInfo> {
        let info = self.info_for(item);
        let mut s = self.shared.borrow_mut();
        s.prepared.push(item.source.url.clone());
        if s.fail_next_prepare {
            s.fail_next_prepare = false;
            s.standby_item = None;
            s.standby_info = None;
            return None;
        }
        s.standby_item = Some(item.clone());
        s.standby_info = Some(info);
        Some(info)
    }

    fn advance(&mut self, expected: Option<&PlaybackItem>) -> Option<StreamInfo> {
        let mut s = self.shared.borrow_mut();
        let (Some(expected), Some(standby)) = (expected, s.standby_item.as_ref()) else {
            return None;
        };
        if !same_item_identity(expected, standby) {
            return None;
        }
        s.standby_item = None;
        s.standby_info.take()
    }

    fn begin_crossfade(&mut self, next: &PlaybackItem, fade_seconds: f64) -> CrossfadeStart {
        let mut s = self.shared.borrow_mut();
        s.begin_crossfade_calls
            .push((next.id.clone(), fade_seconds));
        if let Some(id) = s.standby_id_override.clone() {
            if id != next.id {
                return CrossfadeStart::Declined;
            }
        }
        let info = {
            let len = length_for_item(&s, next);
            StreamInfo {
                rate: RATE,
                channels: 2,
                version: 0,
                length_samples: len,
            }
        };
        match s.fade_mode {
            FadeMode::Null => CrossfadeStart::Declined,
            FadeMode::Immediate => CrossfadeStart::Completed(CrossfadeResult {
                info,
                overlap_frames: s.overlap_frames,
            }),
            FadeMode::Deferred => CrossfadeStart::Pending,
        }
    }

    fn is_output_drained(&self) -> bool {
        false
    }

    fn start_pumping(&mut self) {
        self.shared.borrow_mut().paused = false;
    }

    fn pause_pumping(&mut self) {
        self.shared.borrow_mut().paused = true;
    }
}

fn musepack_caps() -> EngineCapabilities {
    EngineCapabilities {
        preload_next: true,
        sample_accurate_gapless: true,
        decode_gate: true,
        crossfade: false,
    }
}

fn capped_crossfade() -> EngineCapabilities {
    EngineCapabilities {
        crossfade: true,
        ..musepack_caps()
    }
}

struct Harness {
    player: Player,
    shared: Rc<RefCell<Shared>>,
}

fn make_with(
    caps: EngineCapabilities,
    fade_mode: FadeMode,
    default_secs: f64,
    plan: Option<TransitionPlan>,
) -> Harness {
    let shared = Rc::new(RefCell::new(Shared::new(caps, fade_mode, default_secs)));
    shared.borrow_mut().plan = plan;
    let factory_shared = shared.clone();
    let plan_shared = shared.clone();
    let ports = PlayerPorts {
        engine_factory: Box::new(move |_kind: EngineKind| {
            Box::new(FakeEngine {
                shared: factory_shared.clone(),
            }) as Box<dyn Engine>
        }),
        resolve_kind: Box::new(|_item: &PlaybackItem| Ok(EngineKind::Musepack)),
        plan_transition: if plan.is_some() {
            Some(Box::new(move |q: &TransitionQuery| {
                plan_shared.borrow_mut().plan_calls.push(q.clone());
                plan_shared.borrow().plan.unwrap()
            }))
        } else {
            None
        },
    };
    let player = Player::new(
        QueueModel::new(Box::new(|| 0.5)),
        ports,
        PlayerOptions::default(),
    );
    Harness {
        player,
        shared: shared.clone(),
    }
}

fn make() -> Harness {
    make_with(musepack_caps(), FadeMode::Null, 10.0, None)
}

fn make_crossfade(fade_mode: FadeMode, plan: Option<TransitionPlan>) -> Harness {
    make_with(capped_crossfade(), fade_mode, 30.0, plan)
}

impl Harness {
    fn set_rendered(&self, frames: u64) {
        self.shared.borrow_mut().rendered = frames;
    }
    fn shared_ref(&self) -> std::cell::Ref<'_, Shared> {
        self.shared.borrow()
    }
}

// ---------------------------------------------------------------------------
// Sweet-Fade transition planner (transition.test.ts)
// ---------------------------------------------------------------------------

fn tq(max_fade: f64, repeat_one: bool, same_release: Option<bool>) -> TransitionQuery {
    TransitionQuery {
        outgoing: item(1),
        incoming: item(2),
        max_fade_seconds: max_fade,
        repeat_one,
        same_release,
    }
}

fn profile(tail: Vec<f64>, head: Vec<f64>, length: f64) -> BoundaryProfile {
    BoundaryProfile {
        length_seconds: length,
        tail: Some(tail),
        head: Some(head),
    }
}

const BPS: f64 = 0.1;

#[test]
fn planner_never_fades_when_off_or_repeat_one() {
    let out = profile(vec![0.8, 0.8, 0.8], vec![], 200.0);
    assert_eq!(
        plan_transition(&tq(0.0, false, None), Some(&out), Some(&out), BPS),
        TransitionPlan::Gapless
    );
    assert_eq!(
        plan_transition(&tq(8.0, true, None), Some(&out), Some(&out), BPS),
        TransitionPlan::Gapless
    );
}

#[test]
fn planner_falls_back_to_fixed_fade_when_profiles_missing() {
    assert_eq!(
        plan_transition(&tq(8.0, false, None), None, None, BPS),
        TransitionPlan::SweetFade {
            overlap_seconds: 8.0
        }
    );
    let partial = profile(vec![], vec![], 200.0);
    assert_eq!(
        plan_transition(&tq(4.0, false, None), Some(&partial), None, BPS),
        TransitionPlan::SweetFade {
            overlap_seconds: 4.0
        }
    );
}

#[test]
fn planner_keeps_gapless_when_outgoing_ends_in_silence() {
    let mut tail = vec![0.7; 20];
    tail.extend(vec![0.0; 20]);
    tail.extend(vec![0.0; 30]);
    assert!(trailing_silence_seconds(&tail, BPS) >= 3.0);
    let plan = plan_transition(
        &tq(8.0, false, Some(true)),
        Some(&profile(tail, vec![], 200.0)),
        Some(&profile(vec![], vec![], 200.0)),
        BPS,
    );
    assert_eq!(plan, TransitionPlan::Gapless);
}

#[test]
fn planner_keeps_gapless_for_same_release_clean_loud_joins() {
    let plan = plan_transition(
        &tq(8.0, false, Some(true)),
        Some(&profile(vec![0.8; 100], vec![], 200.0)),
        Some(&profile(vec![], vec![0.7; 50], 200.0)),
        BPS,
    );
    assert_eq!(plan, TransitionPlan::Gapless);
}

#[test]
fn planner_chooses_hard_cut_for_loud_ending_and_fast_attack() {
    let plan = plan_transition(
        &tq(8.0, false, Some(false)),
        Some(&profile(vec![0.9; 100], vec![], 200.0)),
        Some(&profile(vec![], vec![0.85, 0.9, 0.9], 200.0)),
        BPS,
    );
    assert_eq!(plan, TransitionPlan::HardCut);
}

#[test]
fn planner_hugs_the_outro_decay() {
    let mut tail = vec![0.9; 50];
    for i in 0..50 {
        tail.push(0.5 * (1.0 - i as f64 / 50.0));
    }
    let plan = plan_transition(
        &tq(12.0, false, None),
        Some(&profile(tail, vec![], 200.0)),
        Some(&profile(vec![], vec![0.1, 0.2, 0.6], 200.0)),
        BPS,
    );
    match plan {
        TransitionPlan::SweetFade { overlap_seconds } => {
            assert!(overlap_seconds > 1.0);
            assert!(overlap_seconds <= 6.0);
        }
        other => panic!("expected sweet-fade, got {other:?}"),
    }
}

#[test]
fn planner_caps_overlap_at_user_setting() {
    let mut tail = vec![0.9; 80];
    tail.extend(vec![0.05; 20]);
    let plan = plan_transition(
        &tq(3.0, false, None),
        Some(&profile(tail, vec![], 200.0)),
        Some(&profile(vec![], vec![0.1], 200.0)),
        BPS,
    );
    match plan {
        TransitionPlan::SweetFade { overlap_seconds } => assert!(overlap_seconds <= 3.0),
        other => panic!("expected sweet-fade, got {other:?}"),
    }
}

#[test]
fn planner_is_deterministic() {
    let out = profile(vec![0.8; 60], vec![], 200.0);
    let inc = profile(vec![], vec![0.2, 0.5], 200.0);
    assert_eq!(
        plan_transition(&tq(8.0, false, None), Some(&out), Some(&inc), BPS),
        plan_transition(&tq(8.0, false, None), Some(&out), Some(&inc), BPS)
    );
}

#[test]
fn planner_classifies_endings_and_attacks() {
    assert!(is_clean_loud_ending(&vec![0.9; 40], BPS));
    let mut mixed = vec![0.9; 38];
    mixed.push(0.1);
    mixed.push(0.02);
    assert!(!is_clean_loud_ending(&mixed, BPS));
    assert!(is_fast_attack(&[0.01, 0.01, 0.9], BPS));
    assert!(!is_fast_attack(&[0.01, 0.01, 0.01], BPS));
}

// ---------------------------------------------------------------------------
// Gain policy (loudness.test.ts)
// ---------------------------------------------------------------------------

fn track_loudness() -> TrackLoudness {
    TrackLoudness {
        lufs: -7.19,
        true_peak_db: -4.19,
    }
}
fn album_loudness() -> musicpack_core::player::types::AlbumLoudness {
    musicpack_core::player::types::AlbumLoudness {
        album_lufs: -7.28,
        album_true_peak_db: -4.19,
    }
}

#[test]
fn gain_off_mode_is_zero() {
    assert_eq!(
        normalization_gain(
            NormalizationMode::Off,
            Some(&track_loudness()),
            Some(&album_loudness())
        ),
        0.0
    );
}

#[test]
fn gain_album_and_track_modes() {
    let t = track_loudness();
    let a = album_loudness();
    let album_gain = normalization_gain(NormalizationMode::Album, Some(&t), Some(&a));
    assert!((album_gain - (-16.0 - a.album_lufs)).abs() < 1e-9);
    let track_gain = normalization_gain(NormalizationMode::Track, Some(&t), Some(&a));
    assert!((track_gain - (-16.0 - t.lufs)).abs() < 1e-9);
}

#[test]
fn gain_missing_loudness_is_zero() {
    assert_eq!(
        normalization_gain(NormalizationMode::Album, Some(&track_loudness()), None),
        0.0
    );
    assert_eq!(
        normalization_gain(NormalizationMode::Track, None, Some(&album_loudness())),
        0.0
    );
}

#[test]
fn gain_true_peak_caps_the_gain() {
    let loud_album = musicpack_core::player::types::AlbumLoudness {
        album_lufs: -30.0,
        album_true_peak_db: -3.0,
    };
    let capped = normalization_gain(
        NormalizationMode::Album,
        Some(&track_loudness()),
        Some(&loud_album),
    );
    assert!((capped - (-1.0 - -3.0)).abs() < 1e-9);
    assert!(capped < (-16.0 - -30.0));
}

#[test]
fn gain_linear_conversion_and_combination() {
    assert_eq!(db_to_linear(0.0), 1.0);
    assert!((db_to_linear(6.0206) - 2.0).abs() < 1e-3);
    assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-3);
    let norm = normalization_gain(
        NormalizationMode::Album,
        Some(&track_loudness()),
        Some(&album_loudness()),
    );
    assert!((combined_gain(0.5, norm) - 0.5 * db_to_linear(norm)).abs() < 1e-9);
    assert_eq!(combined_gain(0.0, 10.0), 0.0);
}

// ---------------------------------------------------------------------------
// Queue / order (queue-model.test.ts, queue-policy.test.ts)
// ---------------------------------------------------------------------------

fn seeded(seed: f64) -> Box<dyn FnMut() -> f64> {
    let mut s = seed;
    Box::new(move || {
        s = (s * 1103515245.0 + 12345.0) % 2147483648.0;
        s / 2147483648.0
    })
}

#[test]
fn queue_play_sequence_keeps_full_sequence() {
    let mut q = QueueModel::new(seeded(42.0));
    let first = q.play_sequence(vec![item(1), item(2), item(3)], 1).unwrap();
    assert_eq!(first.title, "T2");
    assert_eq!(q.items().len(), 3);
    assert_eq!(q.index(), 1);
}

#[test]
fn queue_next_previous_boundaries() {
    let mut q = QueueModel::new(seeded(42.0));
    q.play_sequence(vec![item(1), item(2)], 0).unwrap();
    assert_eq!(q.next().unwrap().title, "T2");
    assert!(q.next().is_none());
    assert_eq!(q.previous().unwrap().title, "T1");
    assert!(q.previous().is_none());
}

#[test]
fn queue_play_next_insert_remove_clear() {
    let mut q = QueueModel::new(seeded(42.0));
    q.play_sequence(vec![item(1), item(3)], 0).unwrap();
    q.play_next(item(2));
    let titles: Vec<String> = q.items().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["T1", "T2", "T3"]);
    assert_eq!(q.index(), 0);
    q.remove_at(0);
    assert_eq!(q.index(), 0);
    q.remove_at(0);
    let titles: Vec<String> = q.items().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["T3"]);
    assert_eq!(q.index(), 0);
    q.clear();
    assert_eq!(q.items().len(), 0);
    assert_eq!(q.index(), -1);
}

#[test]
fn order_shuffle_is_a_deterministic_permutation() {
    let seq: Vec<i64> = (1..=8).collect();
    let mut rng_a = seeded(42.0);
    let shuffled = musicpack_core::player::order::shuffle_order(&seq, &mut *rng_a);
    let mut sorted = shuffled.clone();
    sorted.sort();
    assert_eq!(sorted, seq);
    let mut rng_b = seeded(42.0);
    let shuffled2 = musicpack_core::player::order::shuffle_order(&seq, &mut *rng_b);
    assert_eq!(shuffled, shuffled2);
    assert_eq!(seq, (1..=8).collect::<Vec<i64>>());
}

#[test]
fn order_next_index_under_repeat() {
    use musicpack_core::player::order::next_index_under_repeat;
    assert_eq!(next_index_under_repeat(0, 3, RepeatMode::Off), Some(1));
    assert_eq!(next_index_under_repeat(2, 3, RepeatMode::Off), None);
    assert_eq!(next_index_under_repeat(2, 3, RepeatMode::One), None);
    assert_eq!(next_index_under_repeat(2, 3, RepeatMode::All), Some(0));
    assert_eq!(next_index_under_repeat(1, 3, RepeatMode::All), Some(2));
    assert_eq!(next_index_under_repeat(0, 0, RepeatMode::All), None);
}

#[test]
fn queue_repeat_all_wraps_off_stops() {
    let mut q = QueueModel::new(seeded(7.0));
    q.play_sequence(vec![item(1), item(2), item(3)], 0).unwrap();
    q.set_repeat(RepeatMode::All);
    q.next();
    q.next();
    assert_eq!(q.index(), 2);
    assert_eq!(q.next().unwrap().title, "T1");
    assert_eq!(q.index(), 0);

    let mut q2 = QueueModel::new(seeded(3.0));
    q2.play_sequence(vec![item(1), item(2)], 0).unwrap();
    q2.next();
    assert!(q2.next().is_none());
    assert_eq!(q2.index(), 1);
}

#[test]
fn queue_shuffle_visits_every_item_once_with_current_first() {
    let mut q = QueueModel::new(seeded(42.0));
    q.play_sequence(vec![item(1), item(2), item(3), item(4), item(5)], 2)
        .unwrap();
    q.set_shuffle(true);
    let order = q.presentation_order();
    assert_eq!(order[0], 2);
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    let mut visited = vec![q.index()];
    for _ in 0..10 {
        match q.next() {
            Some(_) => visited.push(q.index()),
            None => break,
        }
    }
    let mut sorted = visited.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    assert_eq!(visited[0], 2);
    assert_eq!(visited.len(), 5);
}

#[test]
fn queue_toggling_shuffle_off_restores_canonical_navigation() {
    let mut q = QueueModel::new(seeded(1.0));
    q.play_sequence(vec![item(1), item(2), item(3)], 0).unwrap();
    q.set_shuffle(true);
    q.next();
    q.set_shuffle(false);
    assert!(q.presentation_order().is_empty());
    let at = q.index();
    if at < 2 {
        assert!(q.next().is_some());
        assert_eq!(q.index(), at + 1);
    }
}

#[test]
fn queue_history_aware_previous_retraces_shuffle() {
    let mut q = QueueModel::new(seeded(9.0));
    q.play_sequence(vec![item(1), item(2), item(3), item(4)], 0)
        .unwrap();
    q.set_shuffle(true);
    let first = q.index();
    q.next();
    assert!(q.previous().is_some());
    assert_eq!(q.index(), first);
}

#[test]
fn queue_move_reorders_and_follows_item() {
    let mut q = QueueModel::new(seeded(5.0));
    q.play_sequence(vec![item(1), item(2), item(3), item(4)], 1)
        .unwrap();
    q.move_item(1, 3);
    let titles: Vec<String> = q.items().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["T1", "T3", "T4", "T2"]);
    assert_eq!(q.index(), 3);
    q.move_item(3, 0);
    let titles: Vec<String> = q.items().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["T2", "T1", "T3", "T4"]);
    assert_eq!(q.index(), 0);
    q.move_item(-1, 2);
    q.move_item(0, 99);
    q.move_item(2, 2);
    let titles: Vec<String> = q.items().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["T2", "T1", "T3", "T4"]);
}

#[test]
fn queue_move_keeps_history_valid() {
    let mut q = QueueModel::new(seeded(5.0));
    q.play_sequence(vec![item(1), item(2), item(3), item(4)], 0)
        .unwrap();
    q.next();
    q.move_item(1, 0);
    assert_eq!(q.previous().unwrap().title, "T1");
}

#[test]
fn queue_remove_keeps_history_valid() {
    let mut q = QueueModel::new(seeded(5.0));
    q.play_sequence(vec![item(1), item(2), item(3), item(4)], 0)
        .unwrap();
    q.set_shuffle(true);
    q.next();
    q.set_shuffle(false);
    q.remove_at(0);
    assert!(q.index() >= 0 && q.index() < q.items().len() as i64);
}

#[test]
fn queue_repeat_all_reshuffles_at_end_of_pass() {
    let mut q = QueueModel::new(seeded(11.0));
    q.play_sequence(vec![item(1), item(2), item(3)], 0).unwrap();
    q.set_shuffle(true);
    q.set_repeat(RepeatMode::All);
    for _ in 0..5 {
        if q.next().is_none() {
            break;
        }
    }
    assert!(q.next().is_some());
}

// ---------------------------------------------------------------------------
// Snapshot codec (snapshot.test.ts)
// ---------------------------------------------------------------------------

const V1_RAW: &str = r#"{
  "v": 1,
  "items": [
    { "id": "t1", "source": {"kind":"http-range","url":"/api/v1/tracks/1/audio","byteSize":1000},
      "track": {"id":1,"title":"T1","artists":[],"duration":10,
        "codec":{"codec":"musepack-sv8","mimeType":"audio/musepack"},
        "audio":{"id":101,"size":1000,"url":"/api/v1/tracks/1/audio"}},
      "releaseId": 9, "albumId": 1, "albumTitle": "A", "artist": "Artist" },
    { "id": "t2", "source": {"kind":"http-range","url":"/api/v1/tracks/2/audio","byteSize":1000},
      "track": {"id":2,"audio":{"url":"/api/v1/tracks/2/audio"}},
      "releaseId": 9, "albumId": 1, "albumTitle": "A", "artist": "Artist" }
  ],
  "index": 1, "positionSeconds": 5.5, "volume": 0.8, "normalizeMode": "album"
}"#;

#[test]
fn snapshot_decodes_v1_and_clamps() {
    let s = decode_snapshot(V1_RAW).unwrap();
    assert_eq!(s.v, SNAPSHOT_VERSION);
    assert_eq!(s.items.len(), 2);
    assert_eq!(s.index, 1);
    assert_eq!(s.position_seconds, 5.5);
    assert_eq!(s.volume, Some(0.8));
    assert_eq!(s.normalize_mode, Some(NormalizationMode::Album));
    assert_eq!(s.repeat, RepeatMode::Off);
    assert!(!s.shuffle);
    assert_eq!(
        s.items[1].get("releaseId"),
        Some(&musicpack_core::json::Value::Number(9.0))
    );
}

#[test]
fn snapshot_decodes_v2_and_preserves_policy() {
    let v2 = r#"{"v":2,"items":[{"id":"t7","source":{"kind":"http-range","url":"/x","byteSize":1},"track":{"id":7,"audio":{"url":"/api/v1/tracks/7/audio"},"duration":20}}],"index":0,"positionSeconds":3,"volume":0.9,"normalizeMode":"track","repeat":"all","shuffle":true}"#;
    let s = decode_snapshot(v2).unwrap();
    assert_eq!(s.v, 2);
    assert_eq!(s.repeat, RepeatMode::All);
    assert!(s.shuffle);
    let bad = r#"{"v":2,"items":[{"id":"t7","source":{"kind":"http-range","url":"/x"},"track":{"id":7,"audio":{"url":"/x"}}}],"index":0,"positionSeconds":0,"volume":1,"normalizeMode":"off","repeat":"sometimes","shuffle":"yes"}"#;
    let s = decode_snapshot(bad).unwrap();
    assert_eq!(s.repeat, RepeatMode::Off);
    assert!(!s.shuffle);
}

#[test]
fn snapshot_rejects_corrupt_and_wrong_versions() {
    assert!(decode_snapshot("").is_none());
    assert!(decode_snapshot("not json {").is_none());
    assert!(decode_snapshot(r#"{"v":2,"items":[]}"#).is_none());
    assert!(decode_snapshot(r#"{"v":2,"items":{}}"#).is_none());
}

#[test]
fn snapshot_rejects_pre_modern_bare_items() {
    let pre = r#"{"v":1,"items":[{"track":{"id":1,"audio":{"url":"/x"}}}],"index":0,"positionSeconds":0,"volume":0.8,"normalizeMode":"album"}"#;
    assert!(decode_snapshot(pre).is_none());
}

#[test]
fn snapshot_keeps_modern_items_from_mixed_payload() {
    let mixed = r#"{"v":1,"items":[{"track":{"id":1,"audio":{"url":"/x"}}},{"id":"t2","source":{"kind":"http-range","url":"/api/v1/tracks/2/audio","byteSize":100},"track":{"id":2,"audio":{"url":"/api/v1/tracks/2/audio"},"duration":20}}],"index":0,"positionSeconds":0,"volume":1,"normalizeMode":"album"}"#;
    let s = decode_snapshot(mixed).unwrap();
    assert_eq!(s.items.len(), 1);
    assert_eq!(
        s.items[0].get("id"),
        Some(&musicpack_core::json::Value::String("t2".into()))
    );
}

#[test]
fn snapshot_filters_broken_items_and_clamps_index() {
    let broken = r#"{"v":1,"items":[{"releaseId":9},{"track":{"id":3,"audio":{}}},{"track":{"audio":{"url":"/x"}}}],"index":0,"positionSeconds":0}"#;
    assert!(decode_snapshot(broken).is_none());
    let partial = r#"{"v":1,"items":[{"id":"t7","source":{"kind":"http-range","url":"/api/v1/tracks/7/audio","byteSize":10},"track":{"id":7,"audio":{"url":"/api/v1/tracks/7/audio"},"duration":15}}],"index":99,"positionSeconds":-4,"volume":5}"#;
    let s = decode_snapshot(partial).unwrap();
    assert_eq!(s.index, 0);
}

#[test]
fn snapshot_round_trips_through_encode() {
    let s = decode_snapshot(V1_RAW).unwrap();
    let encoded = encode_snapshot(&s);
    let s2 = decode_snapshot(&encoded).unwrap();
    assert_eq!(s2, s);
}

#[test]
fn snapshot_clamp_index_matches_history() {
    assert_eq!(clamp_index(0.0, 3), 0);
    assert_eq!(clamp_index(2.0, 3), 2);
    assert_eq!(clamp_index(99.0, 3), 2);
    assert_eq!(clamp_index(-5.0, 3), 0);
    assert_eq!(clamp_index(f64::NAN, 3), 0);
    assert_eq!(clamp_index(1.7, 3), 1);
    assert_eq!(clamp_index(0.0, 0), 0);
}

// ---------------------------------------------------------------------------
// Player scenarios (controller.test.ts core subset)
// ---------------------------------------------------------------------------

fn state_events(events: &[PlayerEvent]) -> Vec<PlayerState> {
    events
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::State { state } => Some(*state),
            _ => None,
        })
        .collect()
}

#[test]
fn player_loads_primes_and_plays() {
    let mut h = make();
    let events = h.player.play_sequence(vec![item(1), item(2)], 0);
    assert_eq!(state_events(&events), vec![PlayerState::Loading]);
    assert_eq!(h.player.model().state, PlayerState::Buffering);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T1");
    assert!(
        h.shared_ref()
            .prepared
            .contains(&"/api/v1/tracks/2/audio".to_string())
    );
    let events = h.player.on_primed();
    assert_eq!(state_events(&events), vec![PlayerState::Playing]);
}

#[test]
fn player_applies_album_gain_and_tracks_volume_separately() {
    let mut h = make();
    let mut t1 = item(1);
    t1.loudness = Some(track_loudness());
    t1.album_loudness = Some(album_loudness());
    h.player.play_sequence(vec![t1], 0);
    h.player.on_primed();
    let gain0 = *h.shared_ref().gains.last().unwrap();
    assert!(gain0 > 0.0 && gain0 < 1.0);
    assert!(h.player.model().norm_db < 0.0);
    h.player.set_volume(1.0);
    let gain1 = *h.shared_ref().gains.last().unwrap();
    assert!(gain1 > gain0);
    h.player.set_normalize_mode(NormalizationMode::Off);
    assert!((h.shared_ref().gains.last().unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn player_position_advances_with_rendered_samples() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    h.set_rendered(5000);
    h.player.on_tick();
    assert!((h.player.model().position_seconds - 5000.0 / RATE as f64).abs() < 1e-6);
    assert!((h.player.model().duration_seconds - 10.0).abs() < 1e-6);
}

#[test]
fn player_seek_maps_album_position_into_current_track() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.player.seek(5.0);
    assert_eq!(h.player.model().state, PlayerState::Buffering);
    h.player.on_primed();
    assert!((h.player.model().position_seconds - 5.0).abs() < 0.01);
    h.set_rendered(2 * RATE as u64);
    h.player.on_tick();
    assert!((h.player.model().position_seconds - 7.0).abs() < 0.02);
}

#[test]
fn player_gapless_advance_and_end() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    assert_eq!(h.player.queue().index(), 0);
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert_eq!(h.player.model().state, PlayerState::Playing);

    h.set_rendered(20 * RATE as u64);
    h.player.on_eos();
    h.player.on_tick();
    assert_eq!(h.player.model().state, PlayerState::Ended);
}

#[test]
fn player_ends_immediately_when_final_eos_after_position_reached() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.model().state, PlayerState::Ended);
}

#[test]
fn player_promoted_play_rejection_reports_paused() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.shared.borrow_mut().fail_play_once = Some("play rejected".into());
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert_eq!(h.player.model().state, PlayerState::Paused);
}

#[test]
fn player_pause_keeps_position_and_resumes() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    h.set_rendered(4000);
    h.player.on_tick();
    h.player.pause();
    assert_eq!(h.player.model().state, PlayerState::Paused);
    assert!((h.player.model().position_seconds - 4000.0 / RATE as f64).abs() < 1e-6);
    h.player.toggle_play();
    assert_eq!(h.player.model().state, PlayerState::Playing);
}

#[test]
fn player_recovers_from_underrun() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    assert_eq!(h.player.model().state, PlayerState::Playing);
    h.player.on_buffering();
    assert_eq!(h.player.model().state, PlayerState::Buffering);
    h.player.on_primed();
    assert_eq!(h.player.model().state, PlayerState::Playing);
}

#[test]
fn player_underrun_and_primed_cannot_override_pause() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    h.player.pause();
    h.player.on_buffering();
    h.player.on_primed();
    assert_eq!(h.player.model().state, PlayerState::Paused);
    assert!(h.shared_ref().paused);
}

#[test]
fn player_seek_across_track_boundary_switches_track() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.player.seek(15.0);
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert!((h.player.model().position_seconds - 15.0).abs() < 0.1);
    h.set_rendered(RATE as u64);
    h.player.on_tick();
    assert!((h.player.model().position_seconds - 16.0).abs() < 0.1);
    assert_ne!(h.player.model().state, PlayerState::Error);
}

#[test]
fn player_lengths_keyed_by_track_id_survive_replaced_queue() {
    let mut h = make();
    {
        let mut s = h.shared.borrow_mut();
        s.length_overrides
            .insert("/api/v1/tracks/1/audio".into(), 20 * RATE as u64);
        s.length_overrides
            .insert("/api/v1/tracks/2/audio".into(), 20 * RATE as u64);
    }
    let mut a1 = mk(1, 20.0);
    a1.id = "t1".into();
    let mut a2 = mk(2, 20.0);
    a2.id = "t2".into();
    h.player.play_sequence(vec![a1, a2], 0);
    h.player.on_primed();
    assert!((h.player.model().duration_seconds - 40.0).abs() < 0.1);

    let mut b1 = mk(101, 10.0);
    b1.id = "t101".into();
    let mut b2 = mk(102, 10.0);
    b2.id = "t102".into();
    h.player.play_sequence(vec![b1, b2], 1);
    h.player.on_primed();
    assert!((h.player.model().duration_seconds - 20.0).abs() < 0.1);
    h.player.seek(15.0);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T102");
}

#[test]
fn player_keeps_decoded_lengths_across_loads() {
    let mut h = make();
    {
        let mut s = h.shared.borrow_mut();
        s.length_overrides
            .insert("/api/v1/tracks/1/audio".into(), 39 * RATE as u64);
        s.length_overrides
            .insert("/api/v1/tracks/2/audio".into(), 5 * RATE as u64);
        s.length_overrides
            .insert("/api/v1/tracks/3/audio".into(), 7 * RATE as u64);
    }
    h.player
        .play_sequence(vec![mk(1, 48.0), mk(2, 48.0), mk(3, 48.0)], 0);
    h.player.on_primed();
    h.set_rendered(39 * RATE as u64);
    h.player.on_eos();
    h.player.on_tick();
    assert!((h.player.model().current_track_start_seconds - 39.0).abs() < 0.1);
    h.player.next();
    h.player.on_primed();
    h.player.on_tick();
    assert!((h.player.model().current_track_start_seconds - 44.0).abs() < 0.1);
    assert!((h.player.model().duration_seconds - 51.0).abs() < 0.1);
}

#[test]
fn player_previous_restarts_current_song() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    h.set_rendered(15 * RATE as u64);
    h.player.on_tick();
    assert!((h.player.model().position_seconds - 15.0).abs() < 0.1);
    h.player.previous();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert!((h.player.model().position_seconds - 10.0).abs() < 0.1);
    assert_eq!(*h.shared_ref().seeks.last().unwrap(), 0);
}

#[test]
fn player_play_queue_index_preserves_queue() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    assert_eq!(h.player.queue().items().len(), 3);
    h.player.play_queue_index(2);
    assert_eq!(h.player.queue().items().len(), 3);
    assert_eq!(h.player.queue().index(), 2);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T3");
    let opens = h.shared_ref().opened.len();
    h.player.play_queue_index(2);
    assert_eq!(h.shared_ref().opened.len(), opens);
}

#[test]
fn player_restores_paused_and_resumes_at_saved_spot() {
    let mut h = make();
    h.player
        .play_sequence(vec![mk(1, 10.0), mk(2, 10.0), mk(3, 10.0)], 0);
    h.player.on_primed();
    h.set_rendered(5 * RATE as u64);
    h.player.on_tick();
    let snap = h.player.take_snapshot().unwrap();

    let mut h2 = make();
    h2.player.restore(&snap);
    let m = h2.player.model();
    assert_eq!(m.state, PlayerState::Paused);
    assert_eq!(m.current.as_ref().unwrap().title, "T1");
    assert!((m.position_seconds - 5.0).abs() < 0.01);
    assert_eq!(h2.player.queue().items().len(), 3);

    h2.player.toggle_play();
    h2.player.on_primed();
    assert_eq!(h2.player.model().state, PlayerState::Playing);
    assert_eq!(*h2.shared_ref().seeks.last().unwrap(), 5 * RATE as u64);
    assert!((h2.player.model().position_seconds - 5.0).abs() < 0.01);
}

#[test]
fn player_restores_policy_v2() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    h.player.set_repeat(RepeatMode::All);
    h.player.set_shuffle(true);
    let snap = h.player.take_snapshot().unwrap();
    let encoded = encode_snapshot(&snap);
    let decoded = decode_snapshot(&encoded).unwrap();
    assert_eq!(decoded.repeat, RepeatMode::All);
    assert!(decoded.shuffle);

    let mut h2 = make();
    h2.player.restore(&decoded);
    assert_eq!(h2.player.model().repeat, RepeatMode::All);
    assert!(h2.player.model().shuffle);
    assert!(h2.player.queue().shuffling());
    assert_eq!(
        h2.player.queue().presentation_order()[0] as i64,
        h2.player.queue().index()
    );
}

#[test]
fn player_legacy_v1_restores_default_policy() {
    let mut h = make();
    let snap = decode_snapshot(V1_RAW).unwrap();
    h.player.restore(&snap);
    assert_eq!(h.player.model().state, PlayerState::Paused);
    // V1_RAW carries index 1, so the restored current is the second item.
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    assert_eq!(h.player.model().repeat, RepeatMode::Off);
    assert!(!h.player.model().shuffle);
}

#[test]
fn player_teardown_clears_state() {
    let mut h = make();
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    h.player.teardown();
    assert!(h.shared_ref().closed);
    assert_eq!(h.player.model().state, PlayerState::Idle);
    assert!(h.player.model().current.is_none());
    assert_eq!(h.player.model().position_seconds, 0.0);
    assert!(h.player.model().error.is_none());
    assert!(h.player.take_snapshot().is_none());
}

#[test]
fn player_external_clear_stops_playback() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.player.queue_mut().clear();
    h.player.on_queue_changed();
    assert_eq!(h.player.model().state, PlayerState::Idle);
    assert!(h.shared_ref().paused);
}

#[test]
fn player_external_remove_current_advances() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    h.player.queue_mut().remove_at(0);
    h.player.on_queue_changed();
    let titles: Vec<String> = h
        .player
        .queue()
        .items()
        .iter()
        .map(|i| i.title.clone())
        .collect();
    assert_eq!(titles, ["T2", "T3"]);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    h.player.on_primed();
    assert_eq!(h.player.model().state, PlayerState::Playing);
}

#[test]
fn player_external_remove_future_keeps_playing() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    let opens = h.shared_ref().opened.len();
    h.player.queue_mut().remove_at(2);
    h.player.on_queue_changed();
    assert_eq!(h.player.queue().items().len(), 2);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T1");
    assert_eq!(h.player.model().state, PlayerState::Playing);
    assert_eq!(h.shared_ref().opened.len(), opens);
}

#[test]
fn player_never_promotes_standby_for_removed_item() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    h.player.queue_mut().remove_at(1);
    h.player.on_queue_changed();
    h.shared.borrow_mut().opened.clear();
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T3");
    assert!(
        h.shared_ref()
            .opened
            .contains(&"/api/v1/tracks/3/audio".to_string())
    );
    assert!(
        !h.shared_ref()
            .opened
            .contains(&"/api/v1/tracks/2/audio".to_string())
    );
    assert!((h.player.model().current_track_duration_seconds - 10.0).abs() < 1e-5);
}

#[test]
fn player_ends_cleanly_when_wrapped_standby_but_repeat_off() {
    let mut h = make();
    h.player.set_repeat(RepeatMode::All);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert!(
        h.shared_ref()
            .prepared
            .iter()
            .filter(|u| *u == "/api/v1/tracks/1/audio")
            .count()
            >= 1
    );
    h.player.set_repeat(RepeatMode::Off);
    h.shared.borrow_mut().opened.clear();
    h.set_rendered(20 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert!(h.shared_ref().opened.is_empty());
    h.set_rendered(30 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.player.model().state, PlayerState::Ended);
}

#[test]
fn player_recovers_when_prepare_next_fails() {
    let mut h = make();
    h.shared.borrow_mut().fail_next_prepare = true;
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    assert!(h.shared_ref().standby_item.is_none());
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
    assert!(
        h.shared_ref()
            .opened
            .iter()
            .filter(|u| *u == "/api/v1/tracks/2/audio")
            .count()
            >= 1
    );
    assert_ne!(h.player.model().state, PlayerState::Ended);
    assert!(h.player.model().error.is_none());
}

// ---------------------------------------------------------------------------
// Crossfade (crossfade.test.ts)
// ---------------------------------------------------------------------------

fn prime_and_play(h: &mut Harness) {
    h.player.on_primed();
}

fn fade_ok() -> CrossfadeResult {
    CrossfadeResult {
        info: StreamInfo {
            rate: RATE,
            channels: 2,
            version: 0,
            length_samples: 30 * RATE as u64,
        },
        overlap_frames: 0,
    }
}

#[test]
fn crossfade_refuses_mismatched_standby_and_falls_back() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.shared.borrow_mut().standby_id_override = Some("t9".into());
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].0, "t2");
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t1");
    assert_eq!(h.player.model().state, PlayerState::Playing);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    assert_ne!(h.player.model().state, PlayerState::Error);
}

#[test]
fn crossfade_fires_once_and_advances() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].1, 8.0);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    h.set_rendered(57 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 2);
}

#[test]
fn crossfade_does_nothing_when_disabled() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_item(item(1));
    prime_and_play(&mut h);
    h.set_rendered(29 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
}

#[test]
fn crossfade_falls_back_when_engine_lacks_capability() {
    let mut h = make_with(musepack_caps(), FadeMode::Immediate, 30.0, None);
    h.shared.borrow_mut().caps.crossfade = false;
    h.player.play_item(item(1));
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(29 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
}

#[test]
fn crossfade_falls_back_when_declined() {
    let mut h = make_crossfade(FadeMode::Null, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(29 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t1");
    assert_ne!(h.player.model().state, PlayerState::Error);
}

#[test]
fn crossfade_never_on_last_track() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_item(item(9));
    h.player.set_crossfade(12.0);
    prime_and_play(&mut h);
    h.set_rendered(29 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
}

#[test]
fn crossfade_never_singleton_repeat_all() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_item(item(5));
    h.player.set_repeat(RepeatMode::All);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(29 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
}

#[test]
fn crossfade_never_repeat_one() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_repeat(RepeatMode::One);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
}

#[test]
fn crossfade_event_and_clamp() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    let events = h.player.set_crossfade(8.0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlayerEvent::Crossfade { seconds } if *seconds == 8.0))
    );
    let events = h.player.set_crossfade(6.0);
    assert!(matches!(events.last(), Some(PlayerEvent::Crossfade { seconds }) if *seconds == 0.0));
    assert_eq!(h.player.model().crossfade_seconds, 0.0);
}

#[test]
fn crossfade_shrinks_outgoing_by_overlap() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.shared.borrow_mut().overlap_frames = 8 * RATE as u64;
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    let m = h.player.model();
    assert_eq!(m.current.as_ref().unwrap().id, "t2");
    assert!((m.current_track_start_seconds - 22.0).abs() < 0.01);
    assert!((m.duration_seconds - 52.0).abs() < 0.01);
}

#[test]
fn boundary_drift_absent_when_position_matches() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.shared.borrow_mut().overlap_frames = 8 * RATE as u64;
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    h.set_rendered(22 * RATE as u64);
    let events = h.player.on_tick();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PlayerEvent::BoundaryDrift { .. }))
    );
}

#[test]
fn boundary_drift_reported_when_disagreeing() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.shared.borrow_mut().overlap_frames = 8 * RATE as u64;
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    h.set_rendered(20 * RATE as u64);
    let events = h.player.on_tick();
    let drift = events.iter().find_map(|e| match e {
        PlayerEvent::BoundaryDrift {
            expected_index,
            observed_index,
            position_samples,
        } => Some((*expected_index, *observed_index, *position_samples)),
        _ => None,
    });
    assert_eq!(drift, Some((1, 0, 20 * RATE as u64)));
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn crossfade_deferred_completes_after_pause() {
    let mut h = make_crossfade(FadeMode::Deferred, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(27 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    h.player.pause();
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    h.player.on_crossfade_complete(Some(fade_ok()));
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn crossfade_superseded_load_skips_bookkeeping() {
    let mut h = make_crossfade(FadeMode::Deferred, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(27 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    h.player.seek(5.0);
    h.player.on_crossfade_complete(Some(fade_ok()));
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t1");
    assert_ne!(h.player.model().state, PlayerState::Error);
}

#[test]
fn crossfade_gapless_plan_declines() {
    let mut h = make_crossfade(FadeMode::Immediate, Some(TransitionPlan::Gapless));
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
    assert_eq!(h.shared_ref().plan_calls.len(), 1);
}

#[test]
fn crossfade_uses_planned_overlap() {
    let mut h = make_crossfade(
        FadeMode::Immediate,
        Some(TransitionPlan::SweetFade {
            overlap_seconds: 2.0,
        }),
    );
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
    h.set_rendered((28.5 * RATE as f64) as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].1, 2.0);
}

#[test]
fn crossfade_eos_race_takes_fade() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].1, 8.0);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn crossfade_eos_race_declines_falls_back() {
    let mut h = make_crossfade(FadeMode::Null, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    assert_ne!(h.player.model().state, PlayerState::Error);
}

#[test]
fn crossfade_eos_race_repeat_one_no_fade() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_repeat(RepeatMode::One);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t1");
}

#[test]
fn crossfade_eos_race_uses_planned_overlap() {
    let mut h = make_crossfade(
        FadeMode::Immediate,
        Some(TransitionPlan::SweetFade {
            overlap_seconds: 3.0,
        }),
    );
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].1, 3.0);
}

#[test]
fn crossfade_eos_race_clamps_to_remaining() {
    let mut h = make_crossfade(
        FadeMode::Immediate,
        Some(TransitionPlan::SweetFade {
            overlap_seconds: 12.0,
        }),
    );
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(12.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_eos();
    let called = h.shared_ref().begin_crossfade_calls[0].1;
    assert!((called - 5.0).abs() < 0.1);
    assert!(called < 6.0);
}

#[test]
fn crossfade_eos_race_gapless_plan_declines() {
    let mut h = make_crossfade(FadeMode::Immediate, Some(TransitionPlan::Gapless));
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert!(h.shared_ref().begin_crossfade_calls.is_empty());
    assert_eq!(h.player.queue().index(), 1);
}

#[test]
fn crossfade_eos_fixed_cap_fallback() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.player.on_eos();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    assert_eq!(h.shared_ref().begin_crossfade_calls[0].1, 8.0);
}

#[test]
fn crossfade_short_tracks_progress() {
    let mut h = make_crossfade(FadeMode::Immediate, None);
    h.player
        .play_sequence(vec![mk(1, 48.0), mk(2, 1.0), mk(3, 1.0), mk(4, 48.0)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered((46.5 * RATE as f64) as u64);
    h.player.on_tick();
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t3");
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t4");
    h.set_rendered(99 * RATE as u64);
    h.player.on_eos();
    h.player.on_tick();
    assert_eq!(h.player.model().state, PlayerState::Ended);
}

#[test]
fn crossfade_eos_during_pending_fade_is_swallowed() {
    let mut h = make_crossfade(FadeMode::Deferred, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(27 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 0);
    h.player.on_crossfade_complete(Some(fade_ok()));
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
}

#[test]
fn crossfade_late_duplicate_eos_does_not_stack_advance() {
    let mut h = make_crossfade(FadeMode::Deferred, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(27 * RATE as u64);
    h.player.on_tick();
    h.player.on_eos();
    h.player.on_crossfade_complete(Some(fade_ok()));
    assert_eq!(h.player.queue().index(), 1);
    // The late eoses are absorbed by the (still-pending) follow-up fade.
    h.player.on_eos();
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
}

#[test]
fn crossfade_eos_race_declares_discarded_tail() {
    let mut h = make_crossfade(
        FadeMode::Immediate,
        Some(TransitionPlan::SweetFade {
            overlap_seconds: 1.0,
        }),
    );
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_crossfade(4.0);
    prime_and_play(&mut h);
    h.set_rendered(28 * RATE as u64);
    h.player.on_eos();
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t2");
    assert!((h.player.model().current_track_start_seconds - 28.0).abs() < 0.1);
}

#[test]
fn tick_catchup_advances_without_crossfade() {
    let mut h = make_with(musepack_caps(), FadeMode::Null, 30.0, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    prime_and_play(&mut h);
    assert_eq!(h.player.queue().index(), 0);
    h.set_rendered(35 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.player.queue().index(), 1);
}

#[test]
fn tick_catchup_capped_at_one_step() {
    let mut h = make_with(musepack_caps(), FadeMode::Null, 30.0, None);
    h.shared.borrow_mut().fail_next_prepare = true;
    h.player.play_sequence(
        vec![item(1), mk(2, 30.0), item(3), item(4), item(5), item(6)],
        0,
    );
    prime_and_play(&mut h);
    h.set_rendered(91 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t1");
}

#[test]
fn tick_catchup_stands_down_without_proven_length() {
    let mut h = make_with(musepack_caps(), FadeMode::Null, 30.0, None);
    h.shared.borrow_mut().fail_next_prepare = true;
    h.player.play_sequence(
        vec![item(1), item(2), item(3), item(4), item(5), item(6)],
        0,
    );
    prime_and_play(&mut h);
    for secs in [31.0, 61.0, 91.0, 181.0, 600.0] {
        h.set_rendered((secs * RATE as f64) as u64);
        h.player.on_tick();
        assert_eq!(h.player.queue().index(), 0);
    }
}

#[test]
fn tick_catchup_suppressed_during_crossfade() {
    let mut h = make_crossfade(FadeMode::Deferred, None);
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.set_crossfade(8.0);
    prime_and_play(&mut h);
    h.set_rendered(25 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.shared_ref().begin_crossfade_calls.len(), 1);
    h.set_rendered(35 * RATE as u64);
    h.player.on_tick();
    assert_eq!(h.player.queue().index(), 0);
}

#[test]
fn tick_previous_navigation_does_not_bounce_forward() {
    let mut h = make_with(musepack_caps(), FadeMode::Null, 30.0, None);
    h.player.play_sequence(
        vec![
            mk(1, 30.0),
            mk(2, 30.0),
            mk(3, 30.0),
            mk(4, 30.0),
            mk(5, 30.0),
            mk(6, 30.0),
        ],
        3,
    );
    prime_and_play(&mut h);
    assert_eq!(h.player.queue().index(), 3);
    h.set_rendered(121 * RATE as u64);
    h.player.previous();
    h.player.on_tick();
    assert_eq!(h.player.queue().index(), 2);
    assert_eq!(h.player.model().current.as_ref().unwrap().id, "t3");
}

#[test]
fn player_never_regresses_cursor_during_gapless_handoff() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.set_rendered((10 * RATE as u64) - 20000);
    h.player.on_eos();
    assert_eq!(h.player.queue().index(), 1);
    h.set_rendered((10 * RATE as u64) - 5000);
    h.player.on_tick();
    assert_eq!(h.player.queue().index(), 1);
    assert_eq!(h.player.model().current.as_ref().unwrap().title, "T2");
}

#[test]
fn player_shuffle_eos_follows_presentation_order() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2), item(3)], 0);
    h.player.on_primed();
    let prepared = h.shared_ref().prepared.clone();
    assert!(prepared.contains(&"/api/v1/tracks/2/audio".to_string()));
    h.player.queue_mut().set_shuffle(true);
    h.player
        .queue_mut()
        .set_presentation_order_for_test(vec![0, 2, 1]);
    assert_eq!(h.player.queue().presentation_order(), vec![0, 2, 1]);
    h.shared.borrow_mut().opened.clear();
    h.set_rendered(10 * RATE as u64);
    h.player.on_eos();
    let cur = h.player.model().current.clone().unwrap();
    if cur.id != "t2" {
        assert_eq!(h.shared_ref().opened.len(), 1);
        assert!((h.player.model().current_track_duration_seconds - 10.0).abs() < 1e-5);
    }
}

#[test]
fn player_events_policy_and_unsubscribe_shape() {
    let mut h = make();
    let mut events = Vec::new();
    events.extend(h.player.set_repeat(RepeatMode::All));
    events.extend(h.player.set_shuffle(true));
    events.extend(h.player.set_repeat(RepeatMode::Off));
    events.extend(h.player.set_shuffle(false));
    let policy: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::Policy { repeat, shuffle } => Some((*repeat, *shuffle)),
            _ => None,
        })
        .collect();
    assert_eq!(
        policy,
        vec![
            (RepeatMode::All, false),
            (RepeatMode::All, true),
            (RepeatMode::Off, true),
            (RepeatMode::Off, false),
        ]
    );
}

#[test]
fn player_error_state_and_event() {
    let mut h = make();
    let events = h.player.on_engine_error("This format is not supported.");
    assert_eq!(states(&events), vec![PlayerState::Error]);
    assert!(
        h.player
            .model()
            .error
            .as_deref()
            .unwrap()
            .contains("not supported")
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlayerEvent::Error { .. }))
    );
}

#[test]
fn player_track_events_fire_on_load_and_teardown() {
    let mut h = make();
    h.player.play_item(item(7));
    let mut tracks = Vec::new();
    h.player.teardown();
    // The load track event is observed through the queue current; teardown
    // reports a null track event.
    let events = h.player.teardown();
    let _ = events;
    tracks.push(h.player.model().current.as_ref().map(|i| i.id.clone()));
    assert!(tracks[0].is_none());
}

fn states(events: &[PlayerEvent]) -> Vec<PlayerState> {
    state_events(events)
}

#[test]
fn player_position_event_is_track_relative() {
    let mut h = make_with(musepack_caps(), FadeMode::Null, 10.0, None);
    h.player.play_sequence(vec![item(1)], 0);
    h.player.on_primed();
    h.set_rendered(3 * RATE as u64);
    let events = h.player.on_tick();
    let pos = events.iter().find_map(|e| match e {
        PlayerEvent::Position {
            position_seconds,
            track_start_seconds,
            track_duration_seconds,
        } => Some((
            *position_seconds,
            *track_start_seconds,
            *track_duration_seconds,
        )),
        _ => None,
    });
    let (p, s, d) = pos.unwrap();
    assert!((p - 3.0).abs() < 1e-5);
    assert_eq!(s, 0.0);
    assert!((d - 10.0).abs() < 1e-5);
}

#[test]
fn player_take_snapshot_none_without_current() {
    let h = make();
    assert!(h.player.take_snapshot().is_none());
}

#[test]
fn player_rng_returning_one_does_not_panic() {
    let mut q = QueueModel::new(Box::new(|| 1.0));
    q.play_sequence((1..=5).map(item).collect(), 0).unwrap();
    q.set_shuffle(true);
    let order = q.presentation_order();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    assert!(q.next().is_some());
}

#[test]
fn player_end_tolerance_constant() {
    assert_eq!(END_TOLERANCE_SAMPLES, 256);
}

#[test]
fn player_nan_duration_hint_is_safe() {
    let mut h = make();
    let mut it = item(1);
    it.duration_hint_seconds = Some(f64::NAN);
    h.player.play_sequence(vec![it], 0);
    // Length can't be computed; the load still completes and remains safe.
    assert_eq!(h.player.model().state, PlayerState::Buffering);
    h.player.on_tick();
    assert!(h.player.model().position_seconds.is_finite());
}

#[test]
fn player_identity_requires_id_and_track_id() {
    let a = item(1);
    let mut b = item(1);
    assert!(same_item_identity(&a, &b));
    b.track_id = 2;
    assert!(!same_item_identity(&a, &b));
    b.track_id = 1;
    b.id = "other".into();
    assert!(!same_item_identity(&a, &b));
}

#[test]
fn transition_plan_hardcut_query_does_not_require_items() {
    // `outgoing`/`incoming` are structurally required but never inspected by
    // the planner; this pins that the item fields do not leak into planning.
    let mut q = tq(8.0, false, None);
    q.outgoing.title = "x".into();
    q.incoming.title = "y".into();
    let out = profile(vec![0.9; 100], vec![], 200.0);
    let inc = profile(vec![], vec![0.85, 0.9], 200.0);
    assert_eq!(
        plan_transition(&q, Some(&out), Some(&inc), BPS),
        TransitionPlan::HardCut
    );
}

#[test]
fn player_duplicate_commands_are_safe() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    for _ in 0..5 {
        h.player.on_primed();
        h.player.pause();
        h.player.resume();
        h.player.on_tick();
    }
    assert!(h.player.model().position_seconds.is_finite());
}

#[test]
fn player_extreme_seek_is_clamped() {
    let mut h = make();
    h.player.play_sequence(vec![item(1), item(2)], 0);
    h.player.on_primed();
    h.player.seek(1e18);
    assert_eq!(h.player.queue().index(), 1);
    assert!(h.player.model().position_seconds.is_finite());
    h.player.seek(-1e18);
    assert!(h.player.model().position_seconds.is_finite());
}

#[test]
fn fake_engine_uses_length_overrides() {
    // Sanity: the harness knob works (used by the length tests above).
    let h = make();
    h.shared.borrow_mut().length_overrides.insert("x".into(), 5);
    assert_eq!(h.shared_ref().length_for("x"), 5);
    assert_eq!(h.shared_ref().length_for("y"), 10 * RATE as u64);
}
