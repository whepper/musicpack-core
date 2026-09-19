//! Transition planning ("Sweet Fades" + repair).
//!
//! Port of `web/player-core/src/transition.ts` (BSD-3-Clause). A PURE
//! function from content profiles + policy context to a transition decision:
//! no DOM, no fetching, no ambient state. Hosts compute [`BoundaryProfile`]s
//! from whatever data they have and inject the plan through
//! [`super::player::PlayerPorts`].

use super::types::PlaybackItem;

/// A transition decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionPlan {
    /// Sample-exact sequential handoff (the normal EOS path).
    Gapless,
    /// Natural stop/start; overlapping would mush two full-energy signals.
    HardCut,
    /// Equal-power overlap of exactly this many seconds.
    SweetFade {
        /// Overlap length in seconds.
        overlap_seconds: f64,
    },
}

/// Linear RMS energy contour around one track boundary.
///
/// Values are normalized to the segment's own peak (0..1). `None` when the
/// host has no data (yet).
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryProfile {
    /// Track length in seconds.
    pub length_seconds: f64,
    /// Last `TAIL` seconds of the track, oldest first, one value per bucket.
    pub tail: Option<Vec<f64>>,
    /// First `HEAD` seconds of the following track, one value per bucket.
    pub head: Option<Vec<f64>>,
}

/// What the host asks the planner.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionQuery {
    /// Outgoing item.
    pub outgoing: PlaybackItem,
    /// Incoming item.
    pub incoming: PlaybackItem,
    /// User cap in seconds (0 disables the feature entirely).
    pub max_fade_seconds: f64,
    /// Repeat-one reloads instead of fading.
    pub repeat_one: bool,
    /// Both items belong to one release (album flow keeps gapless intent).
    pub same_release: Option<bool>,
}

/// Linear RMS below which a bucket counts as silence (~-34 dBFS relative).
pub const SILENCE_THRESHOLD: f64 = 0.02;
/// Trailing silence at least this long → the recording already separates.
pub const MIN_TRAILING_SILENCE_SECONDS: f64 = 1.2;
/// Fraction of a segment's median that still counts as "loud".
pub const LOUD_FRACTION: f64 = 0.25;
/// Shortest overlap worth scheduling when a fade is warranted.
pub const FADE_MIN_SECONDS: f64 = 1.0;
/// Longest overlap the planner will ever choose on its own.
pub const FADE_BASE_SECONDS: f64 = 6.0;
/// Slack added after the last loud bucket so the decay keeps its tail.
pub const OUTRO_GRACE_SECONDS: f64 = 0.5;
/// Extra lead before the overlap span in which arming is allowed.
pub const PRIME_LEAD_SECONDS: f64 = 1.0;

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() >> 1;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

fn energy_floor(contour: &[f64]) -> f64 {
    SILENCE_THRESHOLD.max(LOUD_FRACTION * median(contour))
}

/// Seconds of near-silence at the very end of the contour.
pub fn trailing_silence_seconds(tail: &[f64], seconds_per_bucket: f64) -> f64 {
    let mut n = 0usize;
    for &v in tail.iter().rev() {
        if v >= SILENCE_THRESHOLD {
            break;
        }
        n += 1;
    }
    n as f64 * seconds_per_bucket
}

/// True when the track ends loud and stays loud: no sustained decay, no drop
/// into silence.
pub fn is_clean_loud_ending(tail: &[f64], seconds_per_bucket: f64) -> bool {
    if tail.is_empty() {
        return false;
    }
    let floor = energy_floor(tail);
    let last = *tail.last().unwrap_or(&0.0);
    if last < floor {
        return false;
    }
    let check = check_count(2.0, seconds_per_bucket, tail.len());
    for v in tail.iter().skip(tail.len() - check) {
        if *v < LOUD_FRACTION * median(tail) {
            return false;
        }
    }
    true
}

/// True when the next track reaches loud energy within ~0.3 s of starting.
pub fn is_fast_attack(head: &[f64], seconds_per_bucket: f64) -> bool {
    if head.is_empty() {
        return false;
    }
    let floor = energy_floor(head);
    let check = check_count(0.3, seconds_per_bucket, head.len());
    head.iter().take(check).any(|&v| v >= floor)
}

/// Seconds since the last loud bucket in the tail (the audible outro start).
pub fn decay_seconds(tail: &[f64], seconds_per_bucket: f64) -> f64 {
    let floor = energy_floor(tail);
    for (i, &v) in tail.iter().enumerate().rev() {
        if v >= floor {
            return (tail.len() - 1 - i) as f64 * seconds_per_bucket;
        }
    }
    tail.len() as f64 * seconds_per_bucket
}

/// `min(len, max(1, round(seconds/spb)))`, robust against a zero/NaN bucket
/// duration (the TypeScript arithmetic would produce `Infinity`).
fn check_count(seconds: f64, seconds_per_bucket: f64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if seconds_per_bucket <= 0.0 || !seconds_per_bucket.is_finite() {
        return len;
    }
    let n = (seconds / seconds_per_bucket).round();
    if !n.is_finite() {
        return len;
    }
    (n.max(1.0) as usize).min(len)
}

/// The Sweet-Fade policy. Deterministic and side-effect free.
pub fn plan_transition(
    query: &TransitionQuery,
    outgoing: Option<&BoundaryProfile>,
    incoming: Option<&BoundaryProfile>,
    seconds_per_bucket: f64,
) -> TransitionPlan {
    let max_fade = query.max_fade_seconds.max(0.0);
    if max_fade == 0.0 || query.repeat_one {
        return TransitionPlan::Gapless;
    }

    let out_tail = outgoing.and_then(|p| p.tail.as_deref());
    let in_head = incoming.and_then(|p| p.head.as_deref());
    if outgoing.is_none() || incoming.is_none() || out_tail.is_none() || in_head.is_none() {
        return TransitionPlan::SweetFade {
            overlap_seconds: max_fade,
        };
    }
    let out_tail = out_tail.unwrap();
    let in_head = in_head.unwrap();
    let outgoing = outgoing.unwrap();
    let incoming = incoming.unwrap();

    if trailing_silence_seconds(out_tail, seconds_per_bucket) >= MIN_TRAILING_SILENCE_SECONDS {
        return TransitionPlan::Gapless;
    }
    if query.same_release == Some(true) && is_clean_loud_ending(out_tail, seconds_per_bucket) {
        return TransitionPlan::Gapless;
    }
    if is_clean_loud_ending(out_tail, seconds_per_bucket)
        && is_fast_attack(in_head, seconds_per_bucket)
    {
        return TransitionPlan::HardCut;
    }

    let mut overlap = max_fade.min(FADE_BASE_SECONDS);
    let decay = decay_seconds(out_tail, seconds_per_bucket);
    overlap = overlap.min(decay + OUTRO_GRACE_SECONDS);
    overlap = (FADE_MIN_SECONDS.min(max_fade)).max(overlap);
    overlap = overlap.min(max_fade);
    let out_limit = if outgoing.length_seconds > 0.0 {
        outgoing.length_seconds / 2.0
    } else {
        overlap
    };
    let in_limit = if incoming.length_seconds > 0.0 {
        incoming.length_seconds / 2.0
    } else {
        overlap
    };
    let limit = out_limit.min(in_limit);
    overlap = 0.0f64.max(overlap.min(limit));
    TransitionPlan::SweetFade {
        overlap_seconds: (overlap * 100.0).round() / 100.0,
    }
}
