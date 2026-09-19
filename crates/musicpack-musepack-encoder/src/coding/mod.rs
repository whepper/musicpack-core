//! Musepack SV8 coding layers (downstream of the psychoacoustic model).
//!
//! The psychoacoustic model is a **frozen compatibility boundary** and is not
//! implemented here. This module consumes the model's recorded decisions and
//! implements the coding stages after it:
//!
//! ```text
//! Analyse_Filter → [frozen psychoacoustics] → SCF_Extraktion → NS_Analyse
//!   → Allocate/PNS → Quantisierung → Huffman → AP payload
//! ```
//!
//! Phase 15E implements **SCF extraction**; the later stages are tracked as
//! incomplete and will land in follow-up phases against the same frozen
//! `tests/data/coding/` oracle.
//!
//! # Numerical compatibility
//!
//! The reference operation order is preserved exactly: three 12-sample
//! subframes per band, left-to-right accumulation, the `IFLOORF`/`LOG10`
//! helpers reproduced bit-for-bit, and the reference's stateful behaviour for
//! zero subframes (the scale-factor index is only overwritten when the peak is
//! non-zero) modelled explicitly.

mod alloc_tables;
mod ans_tables;
mod huffman_tables;
mod tables;

pub mod allocate;
pub mod ans;
pub mod frame;
pub mod huffman;
pub mod quant;

pub use allocate::{AllocationOutput, allocate};
pub use ans::{Anspec, NsOutput, Smr, ns_analyse};
pub use frame::FrameEncoder;
pub use huffman::{encode_enum, encode_log};
pub use quant::Quantizer;

use crate::error::EncoderError;
use crate::filterbank::Subband;

/// Subbands produced by the analysis filterbank.
const SUBBANDS: usize = 32;
/// Subframes per band.
const SUBFRAMES: usize = 3;
/// Samples per subframe.
const SUBFRAME_SAMPLES: usize = 12;

/// The reference `-12.6f * log10(x) + 57.8945021823f` constants.
const SCF_SLOPE: f32 = -12.6;
#[allow(clippy::excessive_precision)] // keep the reference literal verbatim
const SCF_BIAS: f32 = 57.8945021823;

/// The reference `IFLOORF` (`my_ifloor`) magic-constant floor.
///
/// `(int32) bits((float)(x + (0x0C00000L + 0.500000001))) - 1262485505`.
fn ifloor(x: f32) -> i32 {
    let shifted = (f64::from(x) + (12_582_912.0_f64 + 0.500_000_001_f64)) as f32;
    (shifted.to_bits() as i32) - 1_262_485_505
}

/// The reference `LOG10` on Apple platforms: `(float) log10((double) x)`.
fn log10_f32(x: f32) -> f32 {
    f64::from(x).log10() as f32
}

/// `SCF[index]` for `index in -6..=127` (frozen reference values).
pub fn scf(index: i32) -> f32 {
    f32::from_bits(tables::SCF_BITS[(index + 6) as usize])
}

/// `invSCF[index]` for `index in -6..=127` (frozen reference values).
pub fn inv_scf(index: i32) -> f32 {
    f32::from_bits(tables::INV_SCF_BITS[(index + 6) as usize])
}

/// `P(new, old) = Penalty[128 + (old) - (new)]`.
fn penalty(new: i32, old: i32) -> u8 {
    tables::PENALTY[(128 + old - new) as usize]
}

/// Persistent scale-factor state carried between frames.
///
/// The reference only overwrites an index when the corresponding subframe peak
/// is non-zero, so zero subframes retain the previous frame's value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScfState {
    /// `SCF_Index_L[32][3]`.
    pub scf_l: [[i32; SUBFRAMES]; SUBBANDS],
    /// `SCF_Index_R[32][3]`.
    pub scf_r: [[i32; SUBFRAMES]; SUBBANDS],
}

/// The outputs of one `SCF_Extraktion` call.
#[derive(Clone, Debug)]
pub struct ScfOutput {
    /// Per-band, per-subframe peak power (left).
    pub power_l: [[f32; SUBFRAMES]; SUBBANDS],
    /// Per-band, per-subframe peak power (right).
    pub power_r: [[f32; SUBFRAMES]; SUBBANDS],
    /// SNR compensation (left).
    pub snr_comp_l: [f32; SUBBANDS],
    /// SNR compensation (right).
    pub snr_comp_r: [f32; SUBBANDS],
    /// Number of samples clamped to ±32767.
    pub overflows: u32,
    /// Largest magnitude clamped.
    pub max_overflow: f32,
}

/// Runs the reference `SCF_Extraktion`.
///
/// `x` is the post-M/S subband matrix; it is normalised in place and clamped on
/// overflow, exactly as the reference does.
// The loop is band-addressed because it updates band-indexed history state.
#[allow(clippy::needless_range_loop)]
pub fn scf_extraktion(
    state: &mut ScfState,
    max_band: usize,
    comb_penalties: i32,
    x: &mut [Subband; SUBBANDS],
) -> Result<ScfOutput, EncoderError> {
    if max_band >= SUBBANDS {
        return Err(EncoderError::InvalidMaxBand(max_band));
    }

    let mut out = ScfOutput {
        power_l: [[0.0; SUBFRAMES]; SUBBANDS],
        power_r: [[0.0; SUBFRAMES]; SUBBANDS],
        snr_comp_l: [0.0; SUBBANDS],
        snr_comp_r: [0.0; SUBBANDS],
        overflows: 0,
        max_overflow: 0.0,
    };

    for band in 0..=max_band {
        let mut peak_l = [0.0f32; SUBFRAMES];
        let mut peak_r = [0.0f32; SUBFRAMES];

        for sub in 0..SUBFRAMES {
            let base = sub * SUBFRAME_SAMPLES;
            let mut l = x[band].left[base].abs();
            let mut r = x[band].right[base].abs();
            let mut sl = x[band].left[base] * x[band].left[base];
            let mut sr = x[band].right[base] * x[band].right[base];
            for n in base + 1..base + SUBFRAME_SAMPLES {
                let al = x[band].left[n].abs();
                let ar = x[band].right[n].abs();
                if l < al {
                    l = al;
                }
                if r < ar {
                    r = ar;
                }
                sl += x[band].left[n] * x[band].left[n];
                sr += x[band].right[n] * x[band].right[n];
            }
            out.power_l[band][sub] = sl;
            out.power_r[band][sub] = sr;
            peak_l[sub] = l;
            peak_r[sub] = r;
        }

        // Scale-factor indices (stateful: only overwritten for non-zero peaks).
        for sub in 0..SUBFRAMES {
            if peak_l[sub] > 0.0 {
                state.scf_l[band][sub] = ifloor(SCF_SLOPE * log10_f32(peak_l[sub]) + SCF_BIAS);
            }
            if peak_r[sub] > 0.0 {
                state.scf_r[band][sub] = ifloor(SCF_SLOPE * log10_f32(peak_r[sub]) + SCF_BIAS);
            }
        }

        let warn_l = clamp_scf(&mut state.scf_l[band]);
        let warn_r = clamp_scf(&mut state.scf_r[band]);

        // Save the pre-combination values for the compensation calculation.
        let comp_l = state.scf_l[band];
        let comp_r = state.scf_r[band];

        combine_scf(&mut state.scf_l[band], comb_penalties);
        combine_scf(&mut state.scf_r[band], comb_penalties);

        out.snr_comp_l[band] = snr_comp(&comp_l, &state.scf_l[band]);
        out.snr_comp_r[band] = snr_comp(&comp_r, &state.scf_r[band]);

        // Normalise the subband samples.
        for sub in 0..SUBFRAMES {
            let fac_l = inv_scf(state.scf_l[band][sub]);
            let fac_r = inv_scf(state.scf_r[band][sub]);
            let base = sub * SUBFRAME_SAMPLES;
            for n in base..base + SUBFRAME_SAMPLES {
                x[band].left[n] *= fac_l;
                x[band].right[n] *= fac_r;
            }
        }

        if warn_l {
            for n in 0..36 {
                let v = x[band].left[n];
                if v > 32_767.0 {
                    out.overflows += 1;
                    out.max_overflow = out.max_overflow.max(v);
                    x[band].left[n] = 32_767.0;
                } else if v < -32_767.0 {
                    out.overflows += 1;
                    out.max_overflow = out.max_overflow.max(-v);
                    x[band].left[n] = -32_767.0;
                }
            }
        }
        if warn_r {
            for n in 0..36 {
                let v = x[band].right[n];
                if v > 32_767.0 {
                    out.overflows += 1;
                    out.max_overflow = out.max_overflow.max(v);
                    x[band].right[n] = 32_767.0;
                } else if v < -32_767.0 {
                    out.overflows += 1;
                    out.max_overflow = out.max_overflow.max(-v);
                    x[band].right[n] = -32_767.0;
                }
            }
        }
    }

    Ok(out)
}

/// Clamps a scale-factor triple to `-6..=121`; returns whether anything moved.
fn clamp_scf(scf: &mut [i32; SUBFRAMES]) -> bool {
    let mut warn = false;
    for value in scf.iter_mut() {
        if *value < -6 {
            *value = -6;
            warn = true;
        }
        if *value > 121 {
            *value = 121;
            warn = true;
        }
    }
    warn
}

/// The reference `invSCF[comp - scf]` SNR compensation.
fn snr_comp(comp: &[i32; SUBFRAMES], scf: &[i32; SUBFRAMES]) -> f32 {
    let t = [
        inv_scf(comp[0] - scf[0]),
        inv_scf(comp[1] - scf[1]),
        inv_scf(comp[2] - scf[2]),
    ];
    (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]) * 0.333_333_33
}

/// The reference scale-factor combination (`CombPenalities >= 0` path or the
/// heuristic `d01`/`d12`/`d02` path).
fn combine_scf(scf: &mut [i32; SUBFRAMES], comb: i32) {
    if comb >= 0 {
        if penalty(scf[0], scf[1]) as i32 + penalty(scf[0], scf[2]) as i32 <= comb {
            scf[2] = scf[0];
            scf[1] = scf[0];
        } else if penalty(scf[1], scf[0]) as i32 + penalty(scf[1], scf[2]) as i32 <= comb {
            scf[0] = scf[1];
            scf[2] = scf[1];
        } else if penalty(scf[2], scf[0]) as i32 + penalty(scf[2], scf[1]) as i32 <= comb {
            scf[0] = scf[2];
            scf[1] = scf[2];
        } else if penalty(scf[0], scf[1]) as i32 <= comb {
            scf[1] = scf[0];
        } else if penalty(scf[1], scf[0]) as i32 <= comb {
            scf[0] = scf[1];
        } else if penalty(scf[1], scf[2]) as i32 <= comb {
            scf[2] = scf[1];
        } else if penalty(scf[2], scf[1]) as i32 <= comb {
            scf[1] = scf[2];
        }
    } else {
        let d12 = scf[2] - scf[1];
        let d01 = scf[1] - scf[0];
        let d02 = scf[2] - scf[0];
        if 0 < d12 && d12 < 5 {
            scf[2] = scf[1];
        } else if -3 < d12 && d12 < 0 {
            scf[1] = scf[2];
        } else if 0 < d01 && d01 < 5 {
            scf[1] = scf[0];
        } else if -3 < d01 && d01 < 0 {
            scf[0] = scf[1];
        } else if 0 < d02 && d02 < 4 {
            scf[2] = scf[0];
        } else if -2 < d02 && d02 < 0 {
            scf[0] = scf[2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ifloor_matches_the_expected_floor() {
        assert_eq!(ifloor(1.5), 1);
        assert_eq!(ifloor(-1.5), -2);
        assert_eq!(ifloor(0.0), 0);
        assert_eq!(ifloor(123.999), 123);
        assert_eq!(ifloor(-0.0001), -1);
    }

    #[test]
    fn frozen_scf_tables_are_self_consistent() {
        // SCF and invSCF are inverses by construction.
        for n in -6..=127 {
            let product = scf(n) * inv_scf(n);
            assert!((product - 1.0).abs() < 1e-5, "n={n} product={product}");
        }
    }

    #[test]
    fn single_band_constant_signal_is_normalised() {
        let mut state = ScfState::default();
        let mut x = [Subband::ZERO; SUBBANDS];
        x[0].left = [1000.0; 36];
        x[0].right = [1000.0; 36];
        let out = scf_extraktion(&mut state, 0, 9, &mut x).unwrap();
        assert_eq!(out.power_l[0][0], 12_000_000.0);
        assert_eq!(state.scf_l[0][0], state.scf_l[0][1]);
        assert_eq!(state.scf_l[0][1], state.scf_l[0][2]);
        // Equal subframes combine to a zero scale-factor difference, so the
        // compensation is invSCF[0]^2 (about 0.694), not 1.
        assert!(out.snr_comp_l[0] > 0.0 && out.snr_comp_l[0] < 1.0);
    }
}
