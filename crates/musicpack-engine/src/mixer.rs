//! Equal-power crossfade mixing and swap accounting.
//!
//! Port of the deterministic core of `web/app/src/lib/playback/audio-worklet.ts`
//! (BSD-3-Clause): per-output-frame gains `out = cos(t·π/2)`,
//! `in = sin(t·π/2)` with `t = mixedFrames / fadeFrames`, missing-lane
//! samples rendered as zero, and the swap accounting that rebases the
//! promoted ring's reported playhead to the boundary where mixing began.

/// Facts reported when a fade's mixing window completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapFacts {
    /// Outgoing ring's own rendered position at the swap.
    pub outgoing_frames: u64,
    /// Incoming ring's own rendered position at the swap.
    pub incoming_frames: u64,
    /// Outgoing ring's position when mixing began (the boundary).
    pub swap_base_frames: u64,
    /// True frames of the outgoing tail actually blended.
    pub overlap_frames: u64,
}

/// Tracks one fade window's per-frame progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mixer {
    fade_frames: u64,
    mixed_frames: u64,
    start_frames: u64,
    started: bool,
}

impl Mixer {
    /// Creates a mixer for a fade window (frames), clamped to at least one.
    pub fn new(fade_frames: u64) -> Self {
        Self {
            fade_frames: fade_frames.max(1),
            mixed_frames: 0,
            start_frames: 0,
            started: false,
        }
    }

    /// The fade window in output frames.
    pub fn fade_frames(&self) -> u64 {
        self.fade_frames
    }

    /// Frames mixed so far.
    pub fn mixed_frames(&self) -> u64 {
        self.mixed_frames
    }

    /// Whether the mixer has begun (and not yet completed) its window.
    pub fn is_mixing(&self) -> bool {
        self.started && self.mixed_frames < self.fade_frames
    }

    /// Begins mixing; `outgoing_rendered` is the outgoing ring's rendered
    /// position at this instant (the swap rebase target).
    pub fn start(&mut self, outgoing_rendered: u64) {
        self.started = true;
        self.mixed_frames = 0;
        self.start_frames = outgoing_rendered;
    }

    /// Gains for the next output frame, or `None` once the window is done.
    pub fn next_gains(&mut self) -> Option<(f64, f64)> {
        if self.mixed_frames >= self.fade_frames {
            return None;
        }
        let t = self.mixed_frames as f64 / self.fade_frames as f64;
        self.mixed_frames += 1;
        Some(equal_power_gains(t))
    }

    /// Computes the swap facts for the moment the window completes.
    pub fn swap_facts(&self, outgoing_rendered: u64, incoming_rendered: u64) -> SwapFacts {
        SwapFacts {
            outgoing_frames: outgoing_rendered,
            incoming_frames: incoming_rendered,
            swap_base_frames: self.start_frames,
            overlap_frames: outgoing_rendered.saturating_sub(self.start_frames),
        }
    }
}

/// Equal-power gains for normalized position `t` in `[0, 1]`.
pub fn equal_power_gains(t: f64) -> (f64, f64) {
    let angle = t * std::f64::consts::PI / 2.0;
    (angle.cos(), angle.sin())
}

/// Mixes one interleaved frame, matching the reference arithmetic exactly:
/// `(current as f64 * out_gain + next as f64 * in_gain) as f32`, with a
/// missing lane contributing zero.
pub fn mix_interleaved(
    dst: &mut [f32],
    current: Option<&[f32]>,
    next: Option<&[f32]>,
    out_gain: f64,
    in_gain: f64,
) {
    for (i, out) in dst.iter_mut().enumerate() {
        let c = current.and_then(|s| s.get(i)).copied().unwrap_or(0.0) as f64;
        let n = next.and_then(|s| s.get(i)).copied().unwrap_or(0.0) as f64;
        *out = (c * out_gain + n * in_gain) as f32;
    }
}

/// Rebases the promoted (incoming) ring's reported playhead to the boundary
/// where mixing began. No-op when the incoming ring has already advanced past
/// it.
pub fn rebase_delta(facts: &SwapFacts) -> u64 {
    facts.swap_base_frames.saturating_sub(facts.incoming_frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_endpoints_and_energy() {
        assert_eq!(equal_power_gains(0.0), (1.0, 0.0));
        let (o, i) = equal_power_gains(0.5);
        assert!((o - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!((i - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!(o * o + i * i - 1.0 < 1e-12);
        assert!((equal_power_gains(1.0).0).abs() < 1e-12);
        assert!((equal_power_gains(1.0).1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mixer_advances_one_frame_per_call() {
        let mut m = Mixer::new(3);
        assert!(!m.is_mixing());
        m.start(100);
        assert!(m.is_mixing());
        assert!(m.next_gains().is_some());
        assert!(m.next_gains().is_some());
        assert!(m.next_gains().is_some());
        assert!(m.next_gains().is_none());
        assert!(!m.is_mixing());
        assert_eq!(m.mixed_frames(), 3);
    }

    #[test]
    fn missing_lane_contributes_zero() {
        let mut out = [9.0f32; 2];
        mix_interleaved(&mut out, None, Some(&[0.5, -0.5]), 1.0, 1.0);
        assert_eq!(out, [0.5, -0.5]);
        mix_interleaved(&mut out, Some(&[0.25]), None, 1.0, 1.0);
        // Missing channel in the "current" slice reads as zero.
        assert_eq!(out, [0.25, 0.0]);
    }

    #[test]
    fn swap_overlap_is_bounded_by_consumed_frames() {
        let mut m = Mixer::new(4);
        m.start(1000);
        // Outgoing ran dry before the window elapsed: only 2 frames consumed.
        let facts = m.swap_facts(1002, 4);
        assert_eq!(facts.outgoing_frames, 1002);
        assert_eq!(facts.incoming_frames, 4);
        assert_eq!(facts.swap_base_frames, 1000);
        assert_eq!(facts.overlap_frames, 2);
        assert_eq!(rebase_delta(&facts), 996);
    }
}
