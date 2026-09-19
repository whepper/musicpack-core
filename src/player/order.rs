//! Queue ordering policy (pure functions).
//!
//! Port of `web/player-core/src/order.ts` (BSD-3-Clause). Every function is
//! deterministic; shuffle takes an injected RNG so hosts/tests seed
//! randomness (purity law: no ambient globals).

use super::types::RepeatMode;

/// Shuffle RNG contract: a value in `[0, 1)`.
///
/// A host RNG that returns exactly `1.0` would select an out-of-range index
/// in the TypeScript implementation; this port clamps defensively so a
/// misbehaving host cannot panic or corrupt the permutation.
pub type Rng = dyn FnMut() -> f64;

/// Fisher–Yates shuffle over `upcoming`. Returns a new vector; the input is
/// untouched.
pub fn shuffle_order<T: Clone>(upcoming: &[T], rng: &mut Rng) -> Vec<T> {
    let mut out = upcoming.to_vec();
    if out.is_empty() {
        return out;
    }
    let mut i = out.len() - 1;
    while i > 0 {
        let raw = rng();
        let scaled = raw * (i as f64 + 1.0);
        let j = if scaled.is_finite() {
            (scaled.floor() as i64).clamp(0, i as i64) as usize
        } else {
            0
        };
        out.swap(i, j);
        i -= 1;
    }
    out
}

/// The index of the next item under the given repeat mode.
///
/// `off`/`one` return `None` at the end (`one` repeats via a player reload,
/// not here); `all` wraps to 0.
pub fn next_index_under_repeat(cursor: i64, length: usize, repeat: RepeatMode) -> Option<usize> {
    if length == 0 {
        return None;
    }
    if cursor + 1 < length as i64 {
        return Some((cursor + 1) as usize);
    }
    if repeat == RepeatMode::All {
        Some(0)
    } else {
        None
    }
}
