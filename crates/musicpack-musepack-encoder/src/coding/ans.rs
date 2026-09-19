//! Musepack noise-shaping (ANS) analysis — the reference `NS_Analyse` /
//! `FindOptimalANS` and its Durbin recursion.
//!
//! This is a faithful scalar port from `codec/libmpcpsy/ans.c`. It consumes the
//! frozen psychoacoustic decisions (`SMR`, `ANSspec`, `Transient`, `MS_Flag`)
//! and the SCF output; it does **not** recompute any psychoacoustics.
//!
//! # Numerical compatibility
//!
//! The reference's operation order is preserved exactly:
//!
//! * the 16-tap autocorrelation accumulation is left-to-right;
//! * the specialised Durbin recursions for orders 1/2/3 (distinct code paths
//!   from the general recursion) are reproduced verbatim, including their
//!   explicit temporaries;
//! * `Cos_Tab`/`Sin_Tab`/`InvFourier` are frozen reference bit patterns because
//!   the reference derives them from libm `cos`/`sin`.
//!
//! The stage is **stateless**: `NS_Analyse` resets `NS_Order` and `FIR` each
//! frame and derives the scaled `SNR_comp` from the SCF output, so no
//! per-instance state is retained.

use super::ans_tables;
use crate::error::EncoderError;

/// `MAX_NS_ORDER`.
pub const MAX_NS_ORDER: usize = 6;
/// Subbands.
const SUBBANDS: usize = 32;
/// Masking-threshold lines per band.
const LINES: usize = 16;

/// `SMRTyp`: per-band signal-to-mask ratios for the four channels.
#[derive(Clone, Copy, Debug)]
pub struct Smr {
    /// Left.
    pub l: [f32; SUBBANDS],
    /// Right.
    pub r: [f32; SUBBANDS],
    /// Mid.
    pub m: [f32; SUBBANDS],
    /// Side.
    pub s: [f32; SUBBANDS],
}

/// The psychoacoustic `ANSspec_*` threshold spectra (512 lines each).
#[derive(Clone, Copy, Debug)]
pub struct Anspec {
    /// Left.
    pub l: [f32; 512],
    /// Right.
    pub r: [f32; 512],
    /// Mid.
    pub m: [f32; 512],
    /// Side.
    pub s: [f32; 512],
}

/// The outputs of one `NS_Analyse` call.
#[derive(Clone, Copy, Debug)]
pub struct NsOutput {
    /// `NS_Order_L[32]`.
    pub order_l: [u32; SUBBANDS],
    /// `NS_Order_R[32]`.
    pub order_r: [u32; SUBBANDS],
    /// `FIR_L[32][6]`.
    pub fir_l: [[f32; MAX_NS_ORDER]; SUBBANDS],
    /// `FIR_R[32][6]`.
    pub fir_r: [[f32; MAX_NS_ORDER]; SUBBANDS],
    /// `SNR_comp_L[32]` after the ANS gain.
    pub snr_comp_l: [f32; SUBBANDS],
    /// `SNR_comp_R[32]` after the ANS gain.
    pub snr_comp_r: [f32; SUBBANDS],
}

fn inv_fourier(k: usize, n: usize) -> f32 {
    f32::from_bits(ans_tables::INV_FOURIER_BITS[k * LINES + n])
}
fn cos_tab(n: usize, k: usize) -> f32 {
    f32::from_bits(ans_tables::COS_TAB_BITS[n * (MAX_NS_ORDER + 1) + k])
}
fn sin_tab(n: usize, k: usize) -> f32 {
    f32::from_bits(ans_tables::SIN_TAB_BITS[n * (MAX_NS_ORDER + 1) + k])
}

/// Runs the reference `NS_Analyse`.
///
/// `snr_comp_l`/`snr_comp_r` are the SCF-stage values (the reference scales
/// them in place); the scaled values are returned in [`NsOutput`].
#[allow(clippy::too_many_arguments)]
pub fn ns_analyse(
    max_band: usize,
    ms_flag: &[i32; SUBBANDS],
    smr: &Smr,
    anspec: &Anspec,
    scf_l: &[[i32; 3]; SUBBANDS],
    scf_r: &[[i32; 3]; SUBBANDS],
    transient: &[i32; SUBBANDS],
    snr_comp_l: &[f32; SUBBANDS],
    snr_comp_r: &[f32; SUBBANDS],
) -> Result<NsOutput, EncoderError> {
    if max_band >= SUBBANDS {
        return Err(EncoderError::InvalidMaxBand(max_band));
    }
    let mut out = NsOutput {
        order_l: [0; SUBBANDS],
        order_r: [0; SUBBANDS],
        fir_l: [[0.0; MAX_NS_ORDER]; SUBBANDS],
        fir_r: [[0.0; MAX_NS_ORDER]; SUBBANDS],
        snr_comp_l: *snr_comp_l,
        snr_comp_r: *snr_comp_r,
    };
    find_optimal_ans(
        max_band,
        ms_flag,
        &anspec.l,
        &anspec.m,
        &mut out.order_l,
        &mut out.snr_comp_l,
        &mut out.fir_l,
        &smr.l,
        &smr.m,
        scf_l,
        transient,
    );
    find_optimal_ans(
        max_band,
        ms_flag,
        &anspec.r,
        &anspec.s,
        &mut out.order_r,
        &mut out.snr_comp_r,
        &mut out.fir_r,
        &smr.r,
        &smr.s,
        scf_r,
        transient,
    );
    Ok(out)
}

/// The reference `FindOptimalANS` for one channel pair.
// The loops are index-addressed to preserve the reference accumulation order.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn find_optimal_ans(
    max_band: usize,
    ms: &[i32; SUBBANDS],
    spec0: &[f32; 512],
    spec1: &[f32; 512],
    ns: &mut [u32; SUBBANDS],
    snr_comp: &mut [f32; SUBBANDS],
    fir: &mut [[f32; MAX_NS_ORDER]; SUBBANDS],
    smr0: &[f32; SUBBANDS],
    smr1: &[f32; SUBBANDS],
    scf: &[[i32; 3]; SUBBANDS],
    transient: &[i32; SUBBANDS],
) {
    for band in 0..=max_band {
        let max = ans_tables::MAX_ANS_ORDER[band] as usize;
        if max == 0 {
            break; // the reference loop stops at the first zero-order band
        }
        if scf[band][0] != scf[band][1] || scf[band][1] != scf[band][2] {
            continue;
        }
        if transient[band] != 0 {
            continue;
        }
        let (src, act_smr) = if ms[band] != 0 {
            (spec1, smr1[band])
        } else {
            (spec0, smr0[band])
        };
        if act_smr < 1.0 {
            continue;
        }

        let base = band << 4;
        let mut spec = [0.0f32; LINES];
        let mut norm = 1.0e-30f32;
        if band & 1 == 1 {
            for n in 0..LINES {
                spec[n] = src[base + (LINES - 1 - n)];
                norm += spec[n];
            }
        } else {
            for n in 0..LINES {
                spec[n] = src[base + n];
                norm += spec[n];
            }
        }

        norm = 16.0 / norm;
        let mut invspec = [0.0f32; LINES];
        let mut min_spec = 1.0e12f32;
        for n in 0..LINES {
            spec[n] *= norm;
            invspec[n] = 1.0 / spec[n];
            if spec[n] < min_spec {
                min_spec = spec[n];
            }
        }

        let mut akf = [0.0f32; MAX_NS_ORDER + 1];
        for k in 0..=max {
            akf[k] = inv_fourier(k, 0) * invspec[0]
                + inv_fourier(k, 1) * invspec[1]
                + inv_fourier(k, 2) * invspec[2]
                + inv_fourier(k, 3) * invspec[3]
                + inv_fourier(k, 4) * invspec[4]
                + inv_fourier(k, 5) * invspec[5]
                + inv_fourier(k, 6) * invspec[6]
                + inv_fourier(k, 7) * invspec[7]
                + inv_fourier(k, 8) * invspec[8]
                + inv_fourier(k, 9) * invspec[9]
                + inv_fourier(k, 10) * invspec[10]
                + inv_fourier(k, 11) * invspec[11]
                + inv_fourier(k, 12) * invspec[12]
                + inv_fourier(k, 13) * invspec[13]
                + inv_fourier(k, 14) * invspec[14]
                + inv_fourier(k, 15) * invspec[15];
        }

        let mut ns_gain = 1.0f32;
        for order in 1..=max {
            let mut h = [0.0f32; MAX_NS_ORDER];
            let mut reflex = [0.0f32; MAX_NS_ORDER];
            match order {
                1 => durbin1(&mut reflex, &mut h, &akf),
                2 => durbin2(&mut reflex, &mut h, &akf),
                3 => durbin3(&mut reflex, &mut h, &akf),
                _ => durbin_n(&mut reflex, &mut h, &akf, order),
            }

            let mut ns_loss = 1.0e-30f32;
            let mut min_diff = 1.0e12f32;
            for n in 0..LINES {
                let mut re = 1.0f32;
                let mut im = 0.0f32;
                for k in 0..order {
                    re -= h[k] * cos_tab(n, k);
                    im += h[k] * sin_tab(n, k);
                }
                let ns_energy = re * re + im * im;
                ns_loss += ns_energy;
                if spec[n] < min_diff * ns_energy {
                    min_diff = spec[n] / ns_energy;
                }
            }

            // `min_spec * ns_loss` is evaluated in f32 (both operands are
            // float) and only then promoted for the double `16.` division.
            let denom = f64::from(min_spec * ns_loss);
            let gain = (16.0 * f64::from(min_diff) / denom) as f32;
            if gain > ns_gain && ns_loss < act_smr {
                ns[band] = order as u32;
                ns_gain = gain;
                fir[band][..order].copy_from_slice(&h[..order]);
            }
        }

        if ns_gain > 1.0 {
            snr_comp[band] *= ns_gain;
        }
    }
}

/// `durbin_akf_to_kh1`.
fn durbin1(k: &mut [f32; MAX_NS_ORDER], h: &mut [f32; MAX_NS_ORDER], akf: &[f32; 7]) {
    h[0] = akf[1] / akf[0];
    k[0] = h[0];
}

/// `durbin_akf_to_kh2`.
///
/// The reference's `1.` literals are `double`, so `1. - tk*tk` and the `h[0]`
/// update are evaluated in `f64` before being rounded back to `f32`.
fn durbin2(k: &mut [f32; MAX_NS_ORDER], h: &mut [f32; MAX_NS_ORDER], akf: &[f32; 7]) {
    let tk = akf[1] / akf[0];
    let e = (f64::from(akf[0]) * (1.0 - f64::from(tk * tk))) as f32;
    h[0] = tk;
    k[0] = tk;
    let h0 = h[0];
    let tk2 = (akf[2] - h0 * akf[1]) / e;
    h[1] = tk2;
    k[1] = tk2;
    h[0] = (f64::from(h[0]) * (1.0 - f64::from(tk2))) as f32;
}

/// `durbin_akf_to_kh3`, with the reference's `double` intermediates preserved.
fn durbin3(k: &mut [f32; MAX_NS_ORDER], h: &mut [f32; MAX_NS_ORDER], akf: &[f32; 7]) {
    let tk1 = akf[1] / akf[0];
    let mut e = (f64::from(akf[0]) * (1.0 - f64::from(tk1 * tk1))) as f32;
    h[0] = tk1;
    k[0] = tk1;
    let tk2 = (akf[2] - h[0] * akf[1]) / e;
    e = (f64::from(e) * (1.0 - f64::from(tk2 * tk2))) as f32;
    let h0 = (f64::from(h[0]) * (1.0 - f64::from(tk2))) as f32;
    h[0] = h0;
    h[1] = tk2;
    k[1] = tk2;
    let tk3 = (akf[3] - h0 * akf[2] - h[1] * akf[1]) / e;
    h[2] = tk3;
    k[2] = tk3;
    let a = h0;
    let b = h[1];
    h[0] = a - b * tk3;
    h[1] = b - a * tk3;
}

/// `durbin_akf_to_kh` (general order).
fn durbin_n(k: &mut [f32; MAX_NS_ORDER], h: &mut [f32; MAX_NS_ORDER], akf: &[f32; 7], n: usize) {
    let mut e = akf[0];
    for i in 0..n {
        let mut s = 0.0f32;
        let mut p = 0usize;
        let mut q = i;
        let mut j = i;
        while j > 0 {
            s += h[p] * akf[q];
            p += 1;
            q -= 1;
            j -= 1;
        }
        let tk = (akf[i + 1] - s) / e;
        e = (f64::from(e) * (1.0 - f64::from(tk * tk))) as f32;
        h[i] = tk;
        k[i] = tk;

        let mut p = 0usize;
        let mut q = i as isize - 1;
        while (p as isize) < q {
            let a = h[p];
            let b = h[q as usize];
            h[p] = a - b * tk;
            h[q as usize] = b - a * tk;
            p += 1;
            q -= 1;
        }
        if p as isize == q {
            h[p] = (f64::from(h[p]) * (1.0 - f64::from(tk))) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durbin_order_one_is_the_normalised_autocorrelation() {
        let akf = [2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut h = [0.0; MAX_NS_ORDER];
        let mut k = [0.0; MAX_NS_ORDER];
        durbin1(&mut k, &mut h, &akf);
        assert_eq!(h[0], 0.5);
        assert_eq!(k[0], 0.5);
    }

    #[test]
    fn durbin_specialisations_produce_finite_coefficients() {
        // The reference uses distinct specialised recursions for orders 1..3
        // (and the general one for 4..6); they are not bit-identical to each
        // other, so this only sanity-checks the order-3 path.
        let akf = [1.0, 0.4, 0.2, 0.1, 0.0, 0.0, 0.0];
        let mut h = [0.0; MAX_NS_ORDER];
        let mut k = [0.0; MAX_NS_ORDER];
        durbin3(&mut k, &mut h, &akf);
        for i in 0..3 {
            assert!(h[i].is_finite(), "h[{i}]");
            assert!(k[i].is_finite(), "k[{i}]");
        }
        // Only the last reflection coefficient is also copied into h.
        assert_eq!(k[2].to_bits(), h[2].to_bits());
    }

    #[test]
    fn silence_leaves_order_zero_without_gain() {
        let ms = [0i32; SUBBANDS];
        let smr = Smr {
            l: [2.0; SUBBANDS],
            r: [2.0; SUBBANDS],
            m: [0.0; SUBBANDS],
            s: [0.0; SUBBANDS],
        };
        let anspec = Anspec {
            l: [0.0; 512],
            r: [0.0; 512],
            m: [0.0; 512],
            s: [0.0; 512],
        };
        let scf = [[0i32; 3]; SUBBANDS];
        let transient = [0i32; SUBBANDS];
        let snr = [1.0f32; SUBBANDS];
        let out = ns_analyse(15, &ms, &smr, &anspec, &scf, &scf, &transient, &snr, &snr).unwrap();
        assert_eq!(out.order_l, [0u32; SUBBANDS]);
        assert_eq!(out.snr_comp_l, snr);
    }
}
