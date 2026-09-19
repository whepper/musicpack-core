//! Clear Voice Detection (`cvd.c`).
//!
//! CVD runs a cepstral analysis on the 2048-point power spectrum to locate
//! harmonic (voice-like) FFT lines, then caps their unpredictability. It uses
//! the bit-manipulation [`logfast`] and the frozen cepstral window/pulse
//! tables, and its second (finer) search is gated on `CVD_used >= 2`.

use super::cvd_tables::{COS_WIN, PULS};
use super::fft::cepstrum2048;
use super::math::logfast;
use super::tables::{MAX_ANALYZED_IDX, MAX_CVD_LINE, MED_ANALYZED_IDX, MIN_ANALYZED_IDX};

/// Results of `CEP_Analyse2048`.
struct CepResult {
    res1: f32,
    res2: f32,
}

/// `SetVoiceLines`: mark the harmonics of a detected base period.
fn set_voice_lines(vocal: &mut [i32], base: f32, val: i32) {
    let max = (MAX_CVD_LINE as f32 * base / 1024.0) as i32;
    let frq = 1024.0f32 / base;
    for n in 1..=max {
        let line = (n as f32 * frq) as usize;
        vocal[line] = val;
        vocal[line + 1] = val;
    }
}

/// `CEP_Analyse2048`.
fn cep_analyse2048(cvd_used: u8, cep: &mut [f32]) -> CepResult {
    let mut cc = [0.0f32; MAX_ANALYZED_IDX + 3];
    let mut res1 = 0.0f32;
    let mut res2 = 0.0f32;

    for n in (MIN_ANALYZED_IDX - 2)..=(MAX_ANALYZED_IDX + 2) {
        if cep[n] > 0.0 {
            let norm = cep[n - 4] * cep[n - 4]
                + cep[n - 3] * cep[n - 3]
                + cep[n - 2] * cep[n - 2]
                + cep[n - 1] * cep[n - 1]
                + cep[n] * cep[n]
                + cep[n + 1] * cep[n + 1]
                + cep[n + 2] * cep[n + 2]
                + cep[n + 3] * cep[n + 3]
                + cep[n + 4] * cep[n + 4];
            let kkf = cep[n - 4] * PULS[0]
                + cep[n - 3] * PULS[1]
                + cep[n - 2] * PULS[2]
                + cep[n - 1] * PULS[3]
                + cep[n] * PULS[4]
                + cep[n + 1] * PULS[5]
                + cep[n + 2] * PULS[6]
                + cep[n + 3] * PULS[7]
                + cep[n + 4] * PULS[8];
            cc[n] = kkf * kkf / norm;
        }
    }

    let mut reff = 0.0f32;
    let mut line = MED_ANALYZED_IDX;
    for n in (MED_ANALYZED_IDX..=MAX_ANALYZED_IDX).rev() {
        if cc[n] * cep[n] * cep[n] > reff
            && cc[n] > 0.40
            && cep[n] > 0.00
            && cc[n] >= cc[n + 1]
            && cc[n] >= cc[n - 1]
            && cc[n + 1] >= cc[n + 2]
            && cc[n - 1] >= cc[n - 2]
        {
            reff = cc[n] * cep[n] * cep[n];
            line = n;
        }
    }
    let sum = cep[line - 3]
        + cep[line - 2]
        + cep[line - 1]
        + cep[line]
        + cep[line + 1]
        + cep[line + 2]
        + cep[line + 3]
        + 1.0e-30;
    let line_sum = (cep[line + 1] - cep[line - 1])
        + 2.0 * (cep[line + 2] - cep[line - 2])
        + 3.0 * (cep[line + 3] - cep[line - 3])
        + sum * line as f32
        + 1.0e-30;
    let qual1 = cc[line] * cep[line] * cep[line]
        + cc[line - 1] * cep[line - 1] * cep[line - 1]
        + cc[line + 1] * cep[line + 1] * cep[line + 1];
    if qual1 > 0.015 {
        res1 = line_sum / sum;
    }

    if cvd_used < 2 {
        return CepResult { res1, res2 };
    }

    // finer search on the upsampled cepstrum
    let mut reff = 0.0f32;
    let mut line = MIN_ANALYZED_IDX;
    for n in (MIN_ANALYZED_IDX - 1..=MED_ANALYZED_IDX + 1).rev() {
        cc[2 * n] += 0.5 * cc[n];
        cc[2 * n + 1] += 0.5 * (cc[n] + cc[n + 1]);
        cep[2 * n] += 0.5 * cep[n];
        cep[2 * n + 1] += 0.5 * (cep[n] + cep[n + 1]);
    }
    for n in (2 * MIN_ANALYZED_IDX..=2 * MED_ANALYZED_IDX).rev() {
        if cc[n] * cep[n] * cep[n] > reff
            && cc[n] > 0.85
            && cep[n] > 0.00
            && cc[n] >= cc[n + 1]
            && cc[n] >= cc[n - 1]
            && cc[n + 1] >= cc[n + 2]
            && cc[n - 1] >= cc[n - 2]
        {
            reff = cc[n] * cep[n] * cep[n];
            line = n;
        }
    }
    let sum = cep[line - 3]
        + cep[line - 2]
        + cep[line - 1]
        + cep[line]
        + cep[line + 1]
        + cep[line + 2]
        + cep[line + 3]
        + 1.0e-30;
    let line_sum = (cep[line + 1] - cep[line - 1])
        + 2.0 * (cep[line + 2] - cep[line - 2])
        + 3.0 * (cep[line + 3] - cep[line - 3])
        + sum * line as f32
        + 1.0e-30;
    let qual2 = cc[line] * cep[line] * cep[line]
        + cc[line - 1] * cep[line - 1] * cep[line - 1]
        + cc[line + 1] * cep[line + 1] * cep[line + 1];
    if qual2 >= 0.1 {
        res2 = (0.5 * f64::from(line_sum) / f64::from(sum)) as f32;
    }

    CepResult { res1, res2 }
}

/// `CVD2048_prepare`: logarithmate and window the lower 256 lines, zero the
/// rest of the first half.
fn cvd2048_prepare(spec: &[f32], cep: &mut [f32]) {
    for n in 0..256 {
        cep[n] = logfast(spec[n]);
    }
    for n in 256..512 {
        cep[n] = logfast(spec[n]) * COS_WIN[n - 256];
    }
    for slot in cep[512..1025].iter_mut() {
        *slot = 0.0;
    }
}

/// `CVD2048`: returns 1 when any harmonic was found (setting `vocal`).
#[must_use]
pub fn cvd2048(
    cvd_used: u8,
    spec: &[f32],
    vocal: &mut [i32],
    cep: &mut [f32],
    ip: &mut [i32; 4096],
) -> i32 {
    cvd2048_prepare(spec, cep);
    cepstrum2048(cep, MAX_ANALYZED_IDX, ip);
    let r = cep_analyse2048(cvd_used, cep);
    if r.res1 > 0.0 || r.res2 > 0.0 {
        if r.res1 > 0.0 {
            set_voice_lines(vocal, r.res1, 100);
        }
        if r.res2 > 0.0 {
            set_voice_lines(vocal, r.res2, 20);
        }
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::super::fft::new_fft_index;
    use super::super::tables::MAX_CVD_LINE;
    use super::{COS_WIN, PULS, cvd2048};

    /// Regression test for Phase 15G.1: the frozen CVD tables must hold the
    /// exact reference constants. The table generator once emitted the
    /// little-endian byte string of each `f32` as a hex integer, i.e. every
    /// entry byte-swapped (`CosWin[0] = 1.0` became a denormal ~4.6e-41).
    /// With swapped tables the cepstral window collapses, voice is never
    /// detected, and delay-frame SMR/Res/SCF/Q diverge from the reference.
    #[test]
    fn frozen_cvd_tables_match_reference_constants() {
        assert_eq!(PULS.len(), 9);
        assert_eq!(COS_WIN.len(), 256);
        // Exact reference constants (`cvd.c`).
        assert_eq!(PULS[4].to_bits(), 0x3F24_5AEF);
        assert_eq!(COS_WIN[0].to_bits(), 0x3F80_0000);
        assert_eq!(COS_WIN[128].to_bits(), 0x3F00_0000);
        // Structural properties a byte-swap breaks: the pulse is symmetric
        // and the rolloff window strictly decreases from 1 towards 0.
        for i in 0..9 {
            assert_eq!(PULS[i].to_bits(), PULS[8 - i].to_bits(), "PULS[{i}]");
        }
        let mut prev = f32::INFINITY;
        for (i, w) in COS_WIN.iter().enumerate() {
            assert!(*w > 0.0 && *w <= 1.0, "COS_WIN[{i}] = {w}");
            assert!(*w < prev, "COS_WIN[{i}] = {w} not decreasing");
            prev = *w;
        }
    }

    /// End-to-end CVD regression: a deterministic harmonic power spectrum must
    /// detect voice exactly as the reference does (base period 512 samples:
    /// even lines 2..=301 set to 100, everything else 0). With byte-swapped
    /// tables no voice is found and the vocal array stays zero.
    #[test]
    fn harmonic_spectrum_detects_voice_like_reference() {
        let mut spec = [0.5f32; 1024];
        for n in (0..512).step_by(20) {
            spec[n] = 80.5;
        }
        let mut vocal = [0i32; MAX_CVD_LINE + 4];
        let mut cep = [0f32; 4096];
        let mut ip = new_fft_index();
        assert_eq!(cvd2048(2, &spec, &mut vocal, &mut cep, &mut ip), 1);
        assert_eq!(vocal[0], 0);
        assert_eq!(vocal[1], 0);
        assert_eq!(vocal[302], 0);
        assert_eq!(vocal[303], 0);
        let marked = vocal.iter().filter(|v| **v == 100).count();
        assert_eq!(marked, 300, "expected 300 voice lines, got {marked}");
        for (n, v) in vocal.iter().enumerate().take(302).skip(2) {
            assert_eq!(*v, 100, "vocal[{n}]");
        }
    }
}
