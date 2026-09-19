//! Waveform envelope v1: constants and the quantization kernel.
//!
//! `specs/musicpack-waveform-v1.md` is normative. A waveform envelope is
//! optional, per-track, derived data: a flat sequence of 100 ms buckets,
//! two bytes each (`peak_u8, rms_u8`), no per-file header; the manifest
//! reference (`version`, `path`, `sha256`, `intervalMs`, `encoding`,
//! `floorDb`, `points`) carries the interpretation.
//!
//! The streaming accumulator lives in
//! [`crate::audio::waveform_acc::WaveformAccumulator`] (phase 9); what lives
//! here is the frozen quantization kernel it calls, ported so its numeric
//! behaviour is pinned by tests independently of the accumulator.
//!
//! Reference implementation: `core/libmusicpack/src/waveform.c`.

/// Waveform manifest `version`: exactly `1` (closed).
pub const VERSION: u32 = 1;

/// Waveform manifest `intervalMs`: exactly `100` (closed).
pub const INTERVAL_MS: u32 = 100;

/// Waveform manifest `encoding`: exactly `"peak-rms-u8"` (closed).
pub const ENCODING: &str = "peak-rms-u8";

/// Waveform manifest `floorDb`: exactly `-60` (closed; a different floor
/// is a v2).
pub const FLOOR_DB: i32 = -60;

/// Maximum `points` per track (24 h × 10 buckets/s), bounding the payload
/// and preventing allocation amplification.
pub const MAX_POINTS: u64 = 864_000;

/// Maximum payload bytes per track (`points * 2`, ≈ 1.5 MiB).
pub const MAX_PAYLOAD_BYTES: u64 = MAX_POINTS * 2;

/// Maximum channel count the accumulator accepts (matches the
/// `musicpack_audio_*` decoder contract).
pub const MAX_CHANNELS: u32 = 8;

/// The v1 quantization kernel (`musicpack-waveform-v1.md` §6).
///
/// Maps a linear amplitude in `[0, ∞)` to the 1-byte logarithmic scale:
///
/// - `a == 0` → `0` (silent; the log mapping is bypassed entirely);
/// - `0 < a <= 1.0`: `dB = 20·log10(a)`, `normalized = (dB − floor)/(0 − floor)`,
///   `raw = round(normalized · 254) + 1`, clamped to `[1, 255]`;
/// - `a > 1.0` → `255` (defensive clamp for decoded float overflow);
/// - `a` exactly `1.0` → `255`;
/// - `a` at the floor (`10⁻³` for the fixed −60 dB floor) → `1`;
/// - non-finite input → `0` (defensive; cannot arise from real PCM).
///
/// Both peak and RMS values use this same mapping. `round` semantics are
/// C `round()`: half-away-from-zero, identical to Rust's `f64::round`.
pub fn quantize_amplitude(a: f64) -> u8 {
    if a.is_nan() {
        return 0;
    }
    if a <= 0.0 {
        return 0;
    }
    if a >= 1.0 {
        return 255;
    }
    let floor = FLOOR_DB as f64; // -60.0
    let db = 20.0 * a.log10();
    let normalized = (db - floor) / (0.0 - floor);
    let raw = (normalized * 254.0).round() + 1.0;
    raw.clamp(1.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_special_cases() {
        assert_eq!(quantize_amplitude(0.0), 0); // silent
        assert_eq!(quantize_amplitude(1.0), 255); // exactly full scale
        assert_eq!(quantize_amplitude(1e-3), 1); // exactly the floor
        assert_eq!(quantize_amplitude(2.0), 255); // > 1.0 clamps
        assert_eq!(quantize_amplitude(1.5), 255);
        assert_eq!(quantize_amplitude(f64::NAN), 0); // defensive
        assert_eq!(quantize_amplitude(-0.5), 0); // defensive (|sample| never negative)
    }

    #[test]
    fn quantization_is_monotonic() {
        let mut prev = 0u8;
        let mut a = 1e-4;
        while a <= 1.0 {
            let q = quantize_amplitude(a);
            assert!(q >= prev, "not monotonic at a={a}");
            prev = q;
            a *= 1.05;
        }
        // The last sampled amplitude below 1.0 legitimately maps to 254;
        // exactly 1.0 is the value that reaches 255 (covered above).
        assert!(prev >= 254);
        assert_eq!(quantize_amplitude(1.0), 255);
    }

    #[test]
    fn known_values() {
        // -6 dB ≈ 0.5012 → normalized = 54/60 = 0.9 → raw = round(228.6)+1 = 230
        assert_eq!(quantize_amplitude(10f64.powf(-6.0 / 20.0)), 230);
        // -30 dB → normalized = 30/60 = 0.5 → raw = round(127)+1 = 128
        assert_eq!(quantize_amplitude(10f64.powf(-30.0 / 20.0)), 128);
        // -60+ε dB just above the floor → 1
        assert_eq!(quantize_amplitude(1.0001e-3), 1);
        // far below the floor → clamps to the minimum non-silent code
        assert_eq!(quantize_amplitude(1e-6), 1);
    }

    #[test]
    fn constants_are_frozen() {
        assert_eq!(VERSION, 1);
        assert_eq!(INTERVAL_MS, 100);
        assert_eq!(ENCODING, "peak-rms-u8");
        assert_eq!(FLOOR_DB, -60);
        assert_eq!(MAX_POINTS, 864_000);
        assert_eq!(MAX_PAYLOAD_BYTES, 1_728_000);
        assert_eq!(MAX_CHANNELS, 8);
    }
}
