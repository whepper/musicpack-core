//! Musepack allocation and PNS — the reference `Allocate`, `ISNR_Schaetzer`,
//! `ISNR_Schaetzer_Trans` and `PNS_SCF`.
//!
//! This is a faithful scalar port from `codec/mpcenc/mpcenc.c` and
//! `codec/libmpcenc/quant.c`. It consumes the frozen SCF/NS outputs and
//! produces `Res`, the post-allocation `SCF_Index` and the post-allocation
//! subband samples. It does **not** implement quantisation.
//!
//! # Numerical compatibility
//!
//! * `mpc_lrintf` (`mpc_nearbyintf`) is reproduced bit-for-bit;
//! * `PNS_SCF`'s `0.5`/`0.25`/`0.8` literals are `double`, so its comparisons
//!   and the `1.2005…` division are evaluated in `f64` exactly as the
//!   reference does;
//! * the per-band loop, the resolution search (`MNR`) and the scale-factor
//!   fine-adaptation preserve the reference order, including the `SCFfac`
//!   scaling of the saved samples.
//!
//! `PNS` is `0` for q5/q6/q7, so the PNS path is only reached for lower
//! qualities; it is implemented and covered by a dedicated fixture.

use super::alloc_tables::{A_TABLE, C_TABLE, NIC};
use super::{SCF_BIAS, SCF_SLOPE, Smr, ifloor, log10_f32};
use crate::error::EncoderError;
use crate::filterbank::Subband;

const SUBBANDS: usize = 32;
/// `LAST_HUFFMAN`: the highest resolution that still gets SCF fine-adaptation.
const LAST_HUFFMAN: i32 = 7;
/// `SCFfac = SCF[n-1]/SCF[n]`.
#[allow(clippy::excessive_precision)] // reference literal kept verbatim
const SCF_FAC: f32 = 0.832980664785;

#[allow(clippy::excessive_precision)]
const ONE_THIRD: f32 = 0.33333333333;
#[allow(clippy::excessive_precision)]
const PNS_SCALE: f64 = 1.2005080577484075047860806747022;

/// The reference `mpc_lrintf`/`mpc_nearbyintf` magic round-to-nearest.
///
/// `fVal + 0x00FF8000` adds the *integer* `16744448` (which is `0x4B7F8000` as
/// a float), then the bit pattern is read back and the bias subtracted.
pub(super) fn lrintf(x: f32) -> i32 {
    let shifted = (x + 16_744_448.0f32).to_bits() as i32;
    shifted.wrapping_sub(0x4B7F_8000u32 as i32)
}

fn ratio(signal: f32, error: f32, snr_comp: f32) -> f32 {
    if signal > error {
        error / (snr_comp * signal)
    } else {
        error / signal
    }
}

fn maxf(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// The reference `ISNR_Schaetzer`.
#[allow(clippy::needless_range_loop)]
pub fn isnr_schaetzer(input: &[f32; 36], snr_comp: f32, res: i32) -> f32 {
    let nic = NIC[res as usize];
    let fac = A_TABLE[res as usize] * nic;
    let invfac = C_TABLE[res as usize] / nic;
    let mut signal = 1.0e-30f32;
    let mut error = 1.0e-30f32;
    for n in 0..36 {
        let x = input[n];
        let err = (lrintf(x * fac) as f32) * invfac - x;
        error += err * err;
        signal += x * x;
    }
    let nic2 = nic * nic;
    error *= nic2;
    signal *= nic2;
    ratio(signal, error, snr_comp)
}

/// The reference `ISNR_Schaetzer_Trans` (three 12-sample subframes, maximum).
#[allow(clippy::needless_range_loop)]
pub fn isnr_schaetzer_trans(input: &[f32; 36], snr_comp: f32, res: i32) -> f32 {
    let nic = NIC[res as usize];
    let fac = A_TABLE[res as usize];
    let invfac = C_TABLE[res as usize];

    let run = |range: core::ops::Range<usize>| {
        let mut signal = 1.0e-30f32;
        let mut error = 1.0e-30f32;
        for k in range {
            let sig = input[k] * nic;
            let err = (lrintf(sig * fac) as f32) * invfac - sig;
            error += err * err;
            signal += sig * sig;
        }
        ratio(signal, error, snr_comp)
    };

    let first = run(0..12);
    let second = run(12..24);
    let third = run(24..36);
    maxf(maxf(first, second), third)
}

/// The reference `PNS_SCF`. Returns whether PNS was selected (and mutates `scf`).
pub fn pns_scf(scf: &mut [i32; 3], s0: f32, s1: f32, s2: f32) -> bool {
    let (mut s0, mut s1, mut s2) = (s0, s1, s2);

    if f64::from(s0) < 0.5 * f64::from(s1)
        || f64::from(s1) < 0.5 * f64::from(s2)
        || f64::from(s0) < 0.5 * f64::from(s2)
    {
        return false;
    }
    if f64::from(s1) < 0.25 * f64::from(s0)
        || f64::from(s2) < 0.25 * f64::from(s1)
        || f64::from(s2) < 0.25 * f64::from(s0)
    {
        return false;
    }

    if f64::from(s0) >= 0.8 * f64::from(s1) {
        if f64::from(s0) >= 0.8 * f64::from(s2) && f64::from(s1) > 0.8 * f64::from(s2) {
            let mean = ONE_THIRD * (s0 + s1 + s2);
            s0 = mean;
            s1 = mean;
            s2 = mean;
        } else {
            let mean = 0.5f32 * (s0 + s1);
            s0 = mean;
            s1 = mean;
        }
    } else if f64::from(s1) >= 0.8 * f64::from(s2) {
        let mean = 0.5f32 * (s1 + s2);
        s1 = mean;
        s2 = mean;
    }

    scf[0] = 63;
    scf[1] = 63;
    scf[2] = 63;

    // `sqrt(S/12 * 4 / 1.2005080577...)`: the product is f32, the division and
    // sqrt are f64, then the result is narrowed back to f32.
    let scale = |s: f32| -> f32 {
        let product = (s / 12.0f32) * 4.0f32;
        (f64::from(product) / PNS_SCALE).sqrt() as f32
    };
    s0 = scale(s0);
    s1 = scale(s1);
    s2 = scale(s2);

    if s0 > 0.0 {
        scf[0] = ifloor(SCF_SLOPE * log10_f32(s0) + SCF_BIAS);
    }
    if s1 > 0.0 {
        scf[1] = ifloor(SCF_SLOPE * log10_f32(s1) + SCF_BIAS);
    }
    if s2 > 0.0 {
        scf[2] = ifloor(SCF_SLOPE * log10_f32(s2) + SCF_BIAS);
    }

    for value in scf.iter_mut() {
        if *value & !63 != 0 {
            *value = if *value > 63 { 63 } else { 0 };
        }
    }
    true
}

/// The outputs of [`allocate`] (the SCF/X mutations happen in place).
#[derive(Clone, Copy, Debug)]
pub struct AllocationOutput {
    /// `Res_L[32]`.
    pub res_l: [i32; SUBBANDS],
    /// `Res_R[32]`.
    pub res_r: [i32; SUBBANDS],
}

/// Runs the reference `Allocate` for both channels.
///
/// `scf_l`/`scf_r` (post-SCF indices) and `x` (post-SCF samples) are mutated in
/// place, matching the reference. `comp_l`/`comp_r` are the `SNR_comp` values
/// after noise shaping.
#[allow(clippy::too_many_arguments)]
pub fn allocate(
    max_band: usize,
    pns: f32,
    transient: &[i32; SUBBANDS],
    smr: &Smr,
    power_l: &[[f32; 3]; SUBBANDS],
    power_r: &[[f32; 3]; SUBBANDS],
    comp_l: &[f32; SUBBANDS],
    comp_r: &[f32; SUBBANDS],
    scf_l: &mut [[i32; 3]; SUBBANDS],
    scf_r: &mut [[i32; 3]; SUBBANDS],
    x: &mut [Subband; SUBBANDS],
) -> Result<AllocationOutput, EncoderError> {
    if max_band >= SUBBANDS {
        return Err(EncoderError::InvalidMaxBand(max_band));
    }
    let mut out = AllocationOutput {
        res_l: [0; SUBBANDS],
        res_r: [0; SUBBANDS],
    };
    allocate_channel(
        max_band,
        &mut out.res_l,
        x,
        true,
        scf_l,
        comp_l,
        &smr.l,
        power_l,
        transient,
        pns,
    );
    allocate_channel(
        max_band,
        &mut out.res_r,
        x,
        false,
        scf_r,
        comp_r,
        &smr.r,
        power_r,
        transient,
        pns,
    );
    Ok(out)
}

// The band loop is index-addressed because it mutates band-indexed state.
#[allow(clippy::too_many_arguments)]
fn allocate_channel(
    max_band: usize,
    res: &mut [i32; SUBBANDS],
    x: &mut [Subband; SUBBANDS],
    left: bool,
    scf: &mut [[i32; 3]; SUBBANDS],
    comp: &[f32; SUBBANDS],
    smr: &[f32; SUBBANDS],
    power: &[[f32; 3]; SUBBANDS],
    transient: &[i32; SUBBANDS],
    pns: f32,
) {
    for band in 0..=max_band {
        let is_transient = transient[band] != 0;
        let pns_selected = band > 0
            && res[band - 1] < 3
            && smr[band] >= 1.0
            && smr[band] < (band as f32) * pns
            && pns_scf(
                &mut scf[band],
                power[band][0],
                power[band][1],
                power[band][2],
            );

        let mut mnr: f32;
        if pns_selected {
            res[band] = -1;
            mnr = 0.0; // the reference leaves MNR stale, but the fine-adapt is skipped
        } else {
            mnr = smr[band]; // `*smr * 1.` rounded back to f32
            while mnr > 1.0 && res[band] != 15 {
                res[band] += 1;
                let r = res[band];
                let est = if is_transient {
                    isnr_schaetzer_trans(x[band].channel(left), comp[band], r)
                } else {
                    isnr_schaetzer(x[band].channel(left), comp[band], r)
                };
                mnr = smr[band] * est;
            }
        }

        if res[band] > 0 && res[band] <= LAST_HUFFMAN && mnr < 1.0 && mnr > 0.0 && !is_transient {
            while scf[band][0] > 0 && scf[band][1] > 0 && scf[band][2] > 0 {
                scf[band][2] -= 1;
                scf[band][1] -= 1;
                scf[band][0] -= 1;

                let samples = x[band].channel_mut(left);
                let save = *samples;
                for value in samples.iter_mut() {
                    *value *= SCF_FAC;
                }
                let r = res[band];
                let tmp = smr[band]
                    * if is_transient {
                        isnr_schaetzer_trans(samples, comp[band], r)
                    } else {
                        isnr_schaetzer(samples, comp[band], r)
                    };
                if tmp > 1.0 {
                    scf[band][0] += 1;
                    scf[band][1] += 1;
                    scf[band][2] += 1;
                    *samples = save;
                    break;
                }
            }
        }
    }
}

/// Small helper to select a channel without unsafe pointer arithmetic.
trait SubbandChannel {
    fn channel(&self, left: bool) -> &[f32; 36];
    fn channel_mut(&mut self, left: bool) -> &mut [f32; 36];
}

impl SubbandChannel for Subband {
    fn channel(&self, left: bool) -> &[f32; 36] {
        if left { &self.left } else { &self.right }
    }
    fn channel_mut(&mut self, left: bool) -> &mut [f32; 36] {
        if left {
            &mut self.left
        } else {
            &mut self.right
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrintf_matches_the_magic_helper() {
        assert_eq!(lrintf(0.0), 0);
        assert_eq!(lrintf(0.4), 0);
        assert_eq!(lrintf(0.6), 1);
        assert_eq!(lrintf(-0.6), -1);
        assert_eq!(lrintf(123.5), 124);
    }

    #[test]
    fn isnr_is_positive_and_finite_for_a_simple_signal() {
        let input = [100.0f32; 36];
        let v = isnr_schaetzer(&input, 1.0, 3);
        assert!(v.is_finite() && v > 0.0, "{v}");
    }

    #[test]
    fn pns_rejects_unbalanced_subframes() {
        let mut scf = [0i32; 3];
        // s0 far below s1 -> reject and leave scf untouched.
        assert!(!pns_scf(&mut scf, 1.0, 100.0, 100.0));
        assert_eq!(scf, [0, 0, 0]);
    }

    #[test]
    fn pns_accepts_balanced_subframes_and_clamps_indices() {
        let mut scf = [0i32; 3];
        assert!(pns_scf(&mut scf, 100.0, 100.0, 100.0));
        for value in scf {
            assert!((0..=63).contains(&value), "{value}");
        }
    }
}
