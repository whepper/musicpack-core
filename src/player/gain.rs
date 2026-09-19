//! BS.1770 playback normalization policy.
//!
//! Port of `web/player-core/src/gain.ts` (BSD-3-Clause). `.mpack` values are
//! never modified: this derives a playback gain only.
//!
//! Phase 9's [`crate::audio::gain_db`] is the low-level measurement
//! primitive (`target − measured`); this module is the player's
//! normalization *policy* layered on top of it (mode selection, the true-peak
//! ceiling, and the linear combination with user volume).

use super::types::{AlbumLoudness, TrackLoudness};

/// Initial client playback target (documented playback policy).
pub const PLAYBACK_TARGET_LUFS: f64 = -16.0;

/// Ceiling applied to the normalized true peak (headroom for downstream).
pub const TRUE_PEAK_CAP_DB: f64 = -1.0;

/// Normalization gain in dB for a track under the given mode.
///
/// Missing loudness (or mode `off`) yields `0`. The gain is
/// `target − measured`, reduced when necessary so the output true peak stays
/// at or below [`TRUE_PEAK_CAP_DB`].
pub fn normalization_gain(
    mode: super::types::NormalizationMode,
    track: Option<&TrackLoudness>,
    album: Option<&AlbumLoudness>,
) -> f64 {
    use super::types::NormalizationMode;
    if mode == NormalizationMode::Off {
        return 0.0;
    }
    let measured = match mode {
        NormalizationMode::Album => album.map(|a| a.album_lufs),
        NormalizationMode::Track => track.map(|t| t.lufs),
        NormalizationMode::Off => None,
    };
    let peak = match mode {
        NormalizationMode::Album => album.map(|a| a.album_true_peak_db),
        NormalizationMode::Track => track.map(|t| t.true_peak_db),
        NormalizationMode::Off => None,
    };
    let Some(measured) = measured else {
        return 0.0;
    };
    let mut gain = PLAYBACK_TARGET_LUFS - measured;
    if let Some(peak) = peak {
        let max_gain = TRUE_PEAK_CAP_DB - peak;
        if gain > max_gain {
            gain = max_gain;
        }
    }
    gain
}

/// Converts dB to a linear gain factor (`10^(db/20)`).
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Combined linear gain = user volume (0..1) × normalization gain.
pub fn combined_gain(user_volume: f64, norm_db: f64) -> f64 {
    user_volume * db_to_linear(norm_db)
}
