//! The psychoacoustic model (`psy.c`, `ans.c`).
//!
//! This is a faithful, scalar, source-derived migration of the reference
//! `Psychoakustisches_Modell`, `RaiseSMR`, `MS_LR_Entscheidung`,
//! `TransientenCalc` and their static helpers. All per-frame and cross-frame
//! state lives in [`PsychoacousticModel`]; no global mutable state is used.
//!
//! Numerical semantics follow the pinned reference exactly: `f32` arithmetic
//! stays `f32`, but double literals (`0.98`, `4.`, `0.025`, …) and the libm
//! calls retain their `f64` precision and narrowing points.

use super::cvd::cvd2048;
use super::fft::{new_fft_index, polar_spec_1024, pow_spec_256, pow_spec_2048};
use super::math::{fabs, maxf, maxi, minf, mini, my_cos, pow, pow10_d, sqrtf};
use super::profile::PsyParams;
use super::tables::{
    BUTFLY, CVD_UNPRED, INIT_PART_LTQ, INVBUTFLY, IW, IW_SHORT, MAX_CVD_LINE, MS2SPAT1, MS2SPAT2,
    MS2SPAT3, MS2SPAT4, PART_LONG, PART_SHORT, PREFAC_LONG, PsyTables, WH, WH_SHORT, WL, WL_SHORT,
    frozen_psy_tables,
};
use crate::error::EncoderError;

/// Number of PCM samples the analysis buffer holds (`ANABUFFER`).
pub const ANALYSE_BUFFER: usize = 1600;
/// FFT history length per save array (`3 * 512`).
const SAVE_LEN: usize = 1536;

/// A frame of PCM data as the reference `PCMDataTyp`.
#[derive(Clone, Copy)]
pub struct PcmFrame<'a> {
    /// Left channel, at least [`ANALYSE_BUFFER`] samples.
    pub l: &'a [f32],
    /// Right channel.
    pub r: &'a [f32],
    /// Mid channel `(L+R)/2`.
    pub m: &'a [f32],
    /// Side channel `(L-R)/2`.
    pub s: &'a [f32],
}

/// The four per-channel signal-to-mask arrays.
#[derive(Clone)]
pub struct Smr {
    /// Left/`L` SMR.
    pub l: [f32; 32],
    /// Right/`R` SMR.
    pub r: [f32; 32],
    /// Mid/`M` SMR.
    pub m: [f32; 32],
    /// Side/`S` SMR.
    pub s: [f32; 32],
}

impl Smr {
    fn zero() -> Self {
        Self {
            l: [0.0; 32],
            r: [0.0; 32],
            m: [0.0; 32],
            s: [0.0; 32],
        }
    }
}

/// The model outputs consumed downstream.
#[derive(Clone)]
pub struct ModelOutput {
    /// SMRs after `Psychoakustisches_Modell`.
    pub smr: Smr,
    /// Long-partition transient flags (left).
    pub transient_l: [i32; PART_SHORT],
    /// Long-partition transient flags (right).
    pub transient_r: [i32; PART_SHORT],
}

/// Persistent cross-frame and per-frame model state.
#[derive(Clone)]
struct PsyState {
    a: [f32; PART_LONG],
    b: [f32; PART_LONG],
    c: [f32; PART_LONG],
    d: [f32; PART_LONG],
    xsave_l: [f32; SAVE_LEN],
    xsave_r: [f32; SAVE_LEN],
    ysave_l: [f32; SAVE_LEN],
    ysave_r: [f32; SAVE_LEN],
    t_l: [f32; PART_LONG],
    t_r: [f32; PART_LONG],
    pre_erg_l: [[f32; PART_SHORT]; 2],
    pre_erg_r: [[f32; PART_SHORT]; 2],
    pre_thr_l: [f32; PART_LONG],
    pre_thr_r: [f32; PART_LONG],
    tmp_mask_l: [f32; PART_LONG],
    tmp_mask_r: [f32; PART_LONG],
    vocal_l: [i32; MAX_CVD_LINE + 4],
    vocal_r: [i32; MAX_CVD_LINE + 4],
    loud: f32,
    ans_spec_l: [f32; 512],
    ans_spec_r: [f32; 512],
    ans_spec_m: [f32; 512],
    ans_spec_s: [f32; 512],
}

impl PsyState {
    /// Mirrors `Init_Psychoakustik` with `SampleFreq == 0`: everything zero
    /// except the pre-echo/post-mask thresholds, seeded from the degenerate
    /// `partLtq`.
    fn new() -> Self {
        let mut s = Self {
            a: [0.0; PART_LONG],
            b: [0.0; PART_LONG],
            c: [0.0; PART_LONG],
            d: [0.0; PART_LONG],
            xsave_l: [0.0; SAVE_LEN],
            xsave_r: [0.0; SAVE_LEN],
            ysave_l: [0.0; SAVE_LEN],
            ysave_r: [0.0; SAVE_LEN],
            t_l: [0.0; PART_LONG],
            t_r: [0.0; PART_LONG],
            pre_erg_l: [[0.0; PART_SHORT]; 2],
            pre_erg_r: [[0.0; PART_SHORT]; 2],
            pre_thr_l: [0.0; PART_LONG],
            pre_thr_r: [0.0; PART_LONG],
            tmp_mask_l: [0.0; PART_LONG],
            tmp_mask_r: [0.0; PART_LONG],
            vocal_l: [0; MAX_CVD_LINE + 4],
            vocal_r: [0; MAX_CVD_LINE + 4],
            loud: 0.0,
            ans_spec_l: [0.0; 512],
            ans_spec_r: [0.0; 512],
            ans_spec_m: [0.0; 512],
            ans_spec_s: [0.0; 512],
        };
        for i in 0..PART_LONG {
            s.pre_thr_l[i] = INIT_PART_LTQ;
            s.pre_thr_r[i] = INIT_PART_LTQ;
            s.tmp_mask_l[i] = INIT_PART_LTQ;
            s.tmp_mask_r[i] = INIT_PART_LTQ;
            s.pre_erg_l[0][i / 3] = INIT_PART_LTQ;
            s.pre_erg_r[0][i / 3] = INIT_PART_LTQ;
            s.pre_erg_l[1][i / 3] = INIT_PART_LTQ;
            s.pre_erg_r[1][i / 3] = INIT_PART_LTQ;
        }
        s
    }
}

/// The Psychoacoustic model for one encoder instance.
pub struct PsychoacousticModel {
    params: PsyParams,
    tables: PsyTables,
    state: PsyState,
    ip: [i32; 4096],
    scratch: [f32; 2048],
    last_erg_l: [f32; 512],
    last_erg_r: [f32; 512],
    last_ls_l: [f32; 57],
    last_ls_r: [f32; 57],
    last_phs_l: [f32; 512],
    last_cw_l: [f32; 512],
    last_erg2_l: [f32; 512],
    last_cls_l: [f32; 57],
}

impl PsychoacousticModel {
    /// Builds the model for a `(quality, sample rate)` pair.
    ///
    /// Only the frozen configurations are supported; an unsupported pair is an
    /// [`EncoderError::UnsupportedPsyConfig`].
    pub fn new(qual: f32, sample_rate: f32) -> Result<Self, EncoderError> {
        let bits = frozen_psy_tables(qual, sample_rate)
            .ok_or(EncoderError::UnsupportedPsyConfig { qual, sample_rate })?;
        Ok(Self {
            params: PsyParams::from_quality(qual),
            tables: PsyTables::from_bits(bits),
            state: PsyState::new(),
            ip: new_fft_index(),
            scratch: [0.0; 2048],
            last_erg_l: [0.0; 512],
            last_erg_r: [0.0; 512],
            last_ls_l: [0.0; 57],
            last_ls_r: [0.0; 57],
            last_phs_l: [0.0; 512],
            last_cw_l: [0.0; 512],
            last_erg2_l: [0.0; 512],
            last_cls_l: [0.0; 57],
        })
    }

    /// The resolved profile parameters.
    #[must_use]
    pub fn params(&self) -> &PsyParams {
        &self.params
    }

    /// The reference `Max_Band` for this configuration.
    #[must_use]
    pub fn max_band(&self) -> i32 {
        self.tables.max_band
    }

    /// The current ANS masking thresholds for the left channel.
    #[must_use]
    pub fn ans_spec_l(&self) -> &[f32; 512] {
        &self.state.ans_spec_l
    }

    /// The current ANS masking thresholds for the right channel.
    #[must_use]
    pub fn ans_spec_r(&self) -> &[f32; 512] {
        &self.state.ans_spec_r
    }

    /// The current ANS masking thresholds for the mid channel.
    #[must_use]
    pub fn ans_spec_m(&self) -> &[f32; 512] {
        &self.state.ans_spec_m
    }

    /// The current ANS masking thresholds for the side channel.
    #[must_use]
    pub fn ans_spec_s(&self) -> &[f32; 512] {
        &self.state.ans_spec_s
    }

    /// Tracked loudness (`state.loud`) — diagnostic accessor.
    #[must_use]
    pub fn loud(&self) -> f32 {
        self.state.loud
    }

    /// First eight `Xsave_L` values — diagnostic accessor.
    #[must_use]
    pub fn xsave_l8(&self) -> [f32; 8] {
        core::array::from_fn(|i| self.state.xsave_l[i])
    }

    /// First eight `PreThr_L` values — diagnostic accessor.
    #[must_use]
    pub fn pre_thr_l8(&self) -> [f32; 8] {
        core::array::from_fn(|i| self.state.pre_thr_l[i])
    }

    /// First eight `tmp_Mask_L` values — diagnostic accessor.
    #[must_use]
    pub fn tmp_mask_l8(&self) -> [f32; 8] {
        core::array::from_fn(|i| self.state.tmp_mask_l[i])
    }

    /// First four short-time integrators — diagnostic accessor.
    #[must_use]
    pub fn int_a4(&self) -> [f32; 4] {
        core::array::from_fn(|i| self.state.a[i])
    }

    /// First four long-time integrators — diagnostic accessor.
    #[must_use]
    pub fn int_b4(&self) -> [f32; 4] {
        core::array::from_fn(|i| self.state.b[i])
    }

    /// Window-1 FFT power spectrum (L) — diagnostic accessor.
    #[must_use]
    pub fn last_erg_l(&self) -> &[f32; 512] {
        &self.last_erg_l
    }

    /// Window-1 FFT power spectrum (R) — diagnostic accessor.
    #[must_use]
    pub fn last_erg_r(&self) -> &[f32; 512] {
        &self.last_erg_r
    }

    /// Window-1 partition energy (L) — diagnostic accessor.
    #[must_use]
    pub fn last_ls_l(&self) -> &[f32; 57] {
        &self.last_ls_l
    }

    /// Window-1 partition energy (R) — diagnostic accessor.
    #[must_use]
    pub fn last_ls_r(&self) -> &[f32; 57] {
        &self.last_ls_r
    }

    /// Window-1 FFT phase (L) — diagnostic accessor.
    #[must_use]
    pub fn last_phs_l(&self) -> &[f32; 512] {
        &self.last_phs_l
    }

    /// Window-1 unpredictability (L) — diagnostic accessor.
    #[must_use]
    pub fn last_cw_l(&self) -> &[f32; 512] {
        &self.last_cw_l
    }

    /// Window-2 FFT power spectrum (L) — diagnostic accessor.
    #[must_use]
    pub fn last_erg2_l(&self) -> &[f32; 512] {
        &self.last_erg2_l
    }

    /// Window-1 weighted partition energy (L) — diagnostic accessor.
    #[must_use]
    pub fn last_cls_l(&self) -> &[f32; 57] {
        &self.last_cls_l
    }

    /// First sixteen CVD voice-line values — diagnostic accessor.
    #[must_use]
    pub fn vocal_l16(&self) -> [i32; 16] {
        core::array::from_fn(|i| self.state.vocal_l[i])
    }

    /// Runs `Psychoakustisches_Modell` for one frame.
    #[must_use]
    pub fn analyse_frame(&mut self, pcm: &PcmFrame<'_>) -> ModelOutput {
        let Self {
            params,
            tables,
            state,
            ip,
            scratch,
            last_erg_l,
            last_erg_r,
            last_ls_l,
            last_ls_r,
            last_phs_l,
            last_cw_l,
            last_erg2_l,
            last_cls_l,
        } = self;

        let max_band = 31usize;
        let max_line = (max_band + 1) * 16;

        let mut smr0 = Smr::zero();
        let mut smr1 = Smr::zero();
        let mut transient_l = [0i32; PART_SHORT];
        let mut transient_r = [0i32; PART_SHORT];

        let mut xerg = [0f32; 1024];
        let mut xerg2 = [0f32; 1024];
        let mut cep = [0f32; 4096];

        let mut isvoc_l = 0i32;
        let mut isvoc_r = 0i32;
        if params.cvd_used != 0 {
            state.vocal_l = [0; MAX_CVD_LINE + 4];
            state.vocal_r = [0; MAX_CVD_LINE + 4];
            pow_spec_2048(&pcm.l[..ANALYSE_BUFFER], &mut xerg, scratch, ip);
            pow_spec_2048(&pcm.r[..ANALYSE_BUFFER], &mut xerg2, scratch, ip);
            isvoc_l = cvd2048(params.cvd_used, &xerg, &mut state.vocal_l, &mut cep, ip);
            isvoc_r = cvd2048(params.cvd_used, &xerg2, &mut state.vocal_r, &mut cep, ip);
        }

        let mut factor_ltq = 1.0f32;

        // ---- window 1 (offset 0) --------------------------------------
        let mut erg0 = [0f32; 512];
        let mut erg1 = [0f32; 512];
        let mut phs0 = [0f32; 512];
        let mut phs1 = [0f32; 512];
        let mut xi_l = [0f32; 32];
        let mut xi_r = [0f32; 32];
        let mut xi_m = [0f32; 32];
        let mut xi_s = [0f32; 32];
        let mut ls_l = [0f32; PART_LONG];
        let mut ls_r = [0f32; PART_LONG];
        let mut ls_m = [0f32; PART_LONG];
        let mut ls_s = [0f32; PART_LONG];
        let mut cw_l = [0f32; 512];
        let mut cw_r = [0f32; 512];
        let mut cls_l = [0f32; PART_LONG];
        let mut cls_r = [0f32; PART_LONG];
        let mut sim_mask_l;
        let mut sim_mask_r;
        let mut clow_l;
        let mut clow_r;
        let mut part_thr_l = [0f32; PART_LONG];
        let mut part_thr_r = [0f32; PART_LONG];
        let mut part_thr_m = [0f32; PART_LONG];
        let mut part_thr_s = [0f32; PART_LONG];
        let mut thr_l = [0f32; 1024];
        let mut thr_r = [0f32; 1024];
        let mut thr_m = [0f32; 1024];
        let mut thr_s = [0f32; 1024];
        let mut short_thr_l = [0f32; PART_SHORT];
        let mut short_thr_r = [0f32; PART_SHORT];
        let mut f256 = [[0f32; 128]; 4];

        polar_spec_1024(&pcm.l[..1024], &mut erg0, &mut phs0, scratch, ip);
        polar_spec_1024(&pcm.r[..1024], &mut erg1, &mut phs1, scratch, ip);
        *last_erg_l = erg0;
        *last_erg_r = erg1;
        *last_phs_l = phs0;

        subband_energy(max_band, &mut xi_l, &mut xi_r, &erg0, &erg1);
        partition_energy(&mut ls_l, &mut ls_r, &erg0, &erg1);
        *last_ls_l = ls_l;
        *last_ls_r = ls_r;

        let vocal_l = if isvoc_l != 0 {
            Some(&state.vocal_l[..])
        } else {
            None
        };
        let vocal_r = if isvoc_r != 0 {
            Some(&state.vocal_r[..])
        } else {
            None
        };
        state.xsave_l.copy_within(0..1024, 512);
        state.ysave_l.copy_within(0..1024, 512);
        calc_unpred(
            params.cvd_used,
            max_line,
            &erg0,
            &phs0,
            vocal_l,
            &mut state.xsave_l,
            &mut state.ysave_l,
            &mut cw_l,
        );
        *last_cw_l = cw_l;
        state.xsave_r.copy_within(0..1024, 512);
        state.ysave_r.copy_within(0..1024, 512);
        calc_unpred(
            params.cvd_used,
            max_line,
            &erg1,
            &phs1,
            vocal_r,
            &mut state.xsave_r,
            &mut state.ysave_r,
            &mut cw_r,
        );

        weighted_partition_energy(&mut cls_l, &mut cls_r, &erg0, &erg1, &cw_l, &cw_r);
        *last_cls_l = cls_l;

        sim_mask_l = [0.0; PART_LONG];
        clow_l = [0.0; PART_LONG];
        spreading_signal(tables, &ls_l, &cls_l, &mut sim_mask_l, &mut clow_l);
        sim_mask_r = [0.0; PART_LONG];
        clow_r = [0.0; PART_LONG];
        spreading_signal(tables, &ls_r, &cls_r, &mut sim_mask_r, &mut clow_r);

        apply_tonality_offset(tables, &mut sim_mask_l, &mut sim_mask_r, &clow_l, &clow_r);

        pow_spec_256(&pcm.l[168..168 + 256], &mut f256[0], scratch, ip);
        pow_spec_256(&pcm.l[312..312 + 256], &mut f256[1], scratch, ip);
        pow_spec_256(&pcm.l[456..456 + 256], &mut f256[2], scratch, ip);
        pow_spec_256(&pcm.l[600..600 + 256], &mut f256[3], scratch, ip);
        calc_short_threshold(
            params,
            &f256,
            &mut short_thr_l,
            &mut state.pre_erg_l,
            &mut transient_l,
        );

        pow_spec_256(&pcm.r[168..168 + 256], &mut f256[0], scratch, ip);
        pow_spec_256(&pcm.r[312..312 + 256], &mut f256[1], scratch, ip);
        pow_spec_256(&pcm.r[456..456 + 256], &mut f256[2], scratch, ip);
        pow_spec_256(&pcm.r[600..600 + 256], &mut f256[3], scratch, ip);
        calc_short_threshold(
            params,
            &f256,
            &mut short_thr_r,
            &mut state.pre_erg_r,
            &mut transient_r,
        );

        if params.var_ltq > 0.0 {
            factor_ltq = adapt_ltq(params, &mut state.loud, tables, &ls_l, &ls_r);
        }

        if params.tmp_mask_used != 0 {
            calc_temporal_threshold(
                tables,
                &mut state.a,
                &mut state.b,
                &mut state.t_l,
                &mut sim_mask_l,
                &mut state.tmp_mask_l,
            );
            calc_temporal_threshold(
                tables,
                &mut state.c,
                &mut state.d,
                &mut state.t_r,
                &mut sim_mask_r,
                &mut state.tmp_mask_r,
            );
            sim_mask_l.copy_from_slice(&state.tmp_mask_l);
            sim_mask_r.copy_from_slice(&state.tmp_mask_r);
        }

        for n in 0..PART_SHORT {
            if transient_l[n] != 0 {
                sim_mask_l[3 * n] = minf(sim_mask_l[3 * n], short_thr_l[n]);
                sim_mask_l[3 * n + 1] = minf(sim_mask_l[3 * n + 1], short_thr_l[n]);
                sim_mask_l[3 * n + 2] = minf(sim_mask_l[3 * n + 2], short_thr_l[n]);
            }
            if transient_r[n] != 0 {
                sim_mask_r[3 * n] = minf(sim_mask_r[3 * n], short_thr_r[n]);
                sim_mask_r[3 * n + 1] = minf(sim_mask_r[3 * n + 1], short_thr_r[n]);
                sim_mask_r[3 * n + 2] = minf(sim_mask_r[3 * n + 2], short_thr_r[n]);
            }
        }

        preecho_control(
            &mut part_thr_l,
            &mut state.pre_thr_l,
            &sim_mask_l,
            &mut part_thr_r,
            &mut state.pre_thr_r,
            &sim_mask_r,
        );

        apply_ltq(
            tables,
            &mut thr_l,
            &mut thr_r,
            &part_thr_l,
            &part_thr_r,
            factor_ltq,
            false,
        );
        adapt_thresholds(max_line, &mut thr_l);
        adapt_thresholds(max_line, &mut thr_r);
        thr_l.copy_within(512..1024, 0);
        thr_r.copy_within(512..1024, 0);

        calculate_smr(
            max_band,
            &xi_l,
            &xi_r,
            &thr_l,
            &thr_r,
            &mut smr0.l,
            &mut smr0.r,
        );

        if params.ms_channelmode > 0 {
            polar_spec_1024(&pcm.m[..1024], &mut erg0, &mut phs0, scratch, ip);
            polar_spec_1024(&pcm.s[..1024], &mut erg1, &mut phs1, scratch, ip);
            subband_energy(max_band, &mut xi_m, &mut xi_s, &erg0, &erg1);
            partition_energy(&mut ls_m, &mut ls_s, &erg0, &erg1);
            calc_ms_threshold(
                params.ms_channelmode,
                &ls_l,
                &ls_r,
                &ls_m,
                &ls_s,
                &mut part_thr_l,
                &mut part_thr_r,
                &mut part_thr_m,
                &mut part_thr_s,
            );
            apply_ltq(
                tables,
                &mut thr_m,
                &mut thr_s,
                &part_thr_m,
                &part_thr_s,
                factor_ltq,
                true,
            );
            adapt_thresholds(max_line, &mut thr_m);
            adapt_thresholds(max_line, &mut thr_s);
            thr_m.copy_within(512..1024, 0);
            thr_s.copy_within(512..1024, 0);
            calculate_smr(
                max_band,
                &xi_m,
                &xi_s,
                &thr_m,
                &thr_s,
                &mut smr0.m,
                &mut smr0.s,
            );
        }

        if params.ns_order > 0 {
            state.ans_spec_l.copy_from_slice(&thr_l[..512]);
            state.ans_spec_r.copy_from_slice(&thr_r[..512]);
            state.ans_spec_m.copy_from_slice(&thr_m[..512]);
            state.ans_spec_s.copy_from_slice(&thr_s[..512]);
        }

        // ---- window 2 (offset 576) ------------------------------------
        part_thr_l = [0.0; PART_LONG];
        part_thr_r = [0.0; PART_LONG];
        polar_spec_1024(&pcm.l[576..576 + 1024], &mut erg0, &mut phs0, scratch, ip);
        polar_spec_1024(&pcm.r[576..576 + 1024], &mut erg1, &mut phs1, scratch, ip);
        *last_erg2_l = erg0;
        subband_energy(max_band, &mut xi_l, &mut xi_r, &erg0, &erg1);
        partition_energy(&mut ls_l, &mut ls_r, &erg0, &erg1);

        state.xsave_l.copy_within(0..1024, 512);
        state.ysave_l.copy_within(0..1024, 512);
        calc_unpred(
            params.cvd_used,
            max_line,
            &erg0,
            &phs0,
            vocal_l,
            &mut state.xsave_l,
            &mut state.ysave_l,
            &mut cw_l,
        );
        state.xsave_r.copy_within(0..1024, 512);
        state.ysave_r.copy_within(0..1024, 512);
        calc_unpred(
            params.cvd_used,
            max_line,
            &erg1,
            &phs1,
            vocal_r,
            &mut state.xsave_r,
            &mut state.ysave_r,
            &mut cw_r,
        );

        weighted_partition_energy(&mut cls_l, &mut cls_r, &erg0, &erg1, &cw_l, &cw_r);
        sim_mask_l = [0.0; PART_LONG];
        clow_l = [0.0; PART_LONG];
        spreading_signal(tables, &ls_l, &cls_l, &mut sim_mask_l, &mut clow_l);
        sim_mask_r = [0.0; PART_LONG];
        clow_r = [0.0; PART_LONG];
        spreading_signal(tables, &ls_r, &cls_r, &mut sim_mask_r, &mut clow_r);
        apply_tonality_offset(tables, &mut sim_mask_l, &mut sim_mask_r, &clow_l, &clow_r);

        pow_spec_256(
            &pcm.l[576 + 168..576 + 168 + 256],
            &mut f256[0],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.l[576 + 312..576 + 312 + 256],
            &mut f256[1],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.l[576 + 456..576 + 456 + 256],
            &mut f256[2],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.l[576 + 600..576 + 600 + 256],
            &mut f256[3],
            scratch,
            ip,
        );
        calc_short_threshold(
            params,
            &f256,
            &mut short_thr_l,
            &mut state.pre_erg_l,
            &mut transient_l,
        );

        pow_spec_256(
            &pcm.r[576 + 168..576 + 168 + 256],
            &mut f256[0],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.r[576 + 312..576 + 312 + 256],
            &mut f256[1],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.r[576 + 456..576 + 456 + 256],
            &mut f256[2],
            scratch,
            ip,
        );
        pow_spec_256(
            &pcm.r[576 + 600..576 + 600 + 256],
            &mut f256[3],
            scratch,
            ip,
        );
        calc_short_threshold(
            params,
            &f256,
            &mut short_thr_r,
            &mut state.pre_erg_r,
            &mut transient_r,
        );

        if params.var_ltq > 0.0 {
            factor_ltq = adapt_ltq(params, &mut state.loud, tables, &ls_l, &ls_r);
        }
        if params.tmp_mask_used != 0 {
            calc_temporal_threshold(
                tables,
                &mut state.a,
                &mut state.b,
                &mut state.t_l,
                &mut sim_mask_l,
                &mut state.tmp_mask_l,
            );
            calc_temporal_threshold(
                tables,
                &mut state.c,
                &mut state.d,
                &mut state.t_r,
                &mut sim_mask_r,
                &mut state.tmp_mask_r,
            );
            sim_mask_l.copy_from_slice(&state.tmp_mask_l);
            sim_mask_r.copy_from_slice(&state.tmp_mask_r);
        }
        for n in 0..PART_SHORT {
            if transient_l[n] != 0 {
                sim_mask_l[3 * n] = minf(sim_mask_l[3 * n], short_thr_l[n]);
                sim_mask_l[3 * n + 1] = minf(sim_mask_l[3 * n + 1], short_thr_l[n]);
                sim_mask_l[3 * n + 2] = minf(sim_mask_l[3 * n + 2], short_thr_l[n]);
            }
            if transient_r[n] != 0 {
                sim_mask_r[3 * n] = minf(sim_mask_r[3 * n], short_thr_r[n]);
                sim_mask_r[3 * n + 1] = minf(sim_mask_r[3 * n + 1], short_thr_r[n]);
                sim_mask_r[3 * n + 2] = minf(sim_mask_r[3 * n + 2], short_thr_r[n]);
            }
        }
        preecho_control(
            &mut part_thr_l,
            &mut state.pre_thr_l,
            &sim_mask_l,
            &mut part_thr_r,
            &mut state.pre_thr_r,
            &sim_mask_r,
        );
        apply_ltq(
            tables,
            &mut thr_l,
            &mut thr_r,
            &part_thr_l,
            &part_thr_r,
            factor_ltq,
            false,
        );
        adapt_thresholds(max_line, &mut thr_l);
        adapt_thresholds(max_line, &mut thr_r);
        thr_l.copy_within(512..1024, 0);
        thr_r.copy_within(512..1024, 0);
        calculate_smr(
            max_band,
            &xi_l,
            &xi_r,
            &thr_l,
            &thr_r,
            &mut smr1.l,
            &mut smr1.r,
        );

        if params.ms_channelmode > 0 {
            polar_spec_1024(&pcm.m[576..576 + 1024], &mut erg0, &mut phs0, scratch, ip);
            polar_spec_1024(&pcm.s[576..576 + 1024], &mut erg1, &mut phs1, scratch, ip);
            subband_energy(max_band, &mut xi_m, &mut xi_s, &erg0, &erg1);
            partition_energy(&mut ls_m, &mut ls_s, &erg0, &erg1);
            calc_ms_threshold(
                params.ms_channelmode,
                &ls_l,
                &ls_r,
                &ls_m,
                &ls_s,
                &mut part_thr_l,
                &mut part_thr_r,
                &mut part_thr_m,
                &mut part_thr_s,
            );
            apply_ltq(
                tables,
                &mut thr_m,
                &mut thr_s,
                &part_thr_m,
                &part_thr_s,
                factor_ltq,
                true,
            );
            adapt_thresholds(max_line, &mut thr_m);
            adapt_thresholds(max_line, &mut thr_s);
            thr_m.copy_within(512..1024, 0);
            thr_s.copy_within(512..1024, 0);
            calculate_smr(
                max_band,
                &xi_m,
                &xi_s,
                &thr_m,
                &thr_s,
                &mut smr1.m,
                &mut smr1.s,
            );
        }

        if params.ns_order > 0 {
            for n in 0..512 {
                state.ans_spec_l[n] = minf(state.ans_spec_l[n], thr_l[n]);
                state.ans_spec_r[n] = minf(state.ans_spec_r[n], thr_r[n]);
                state.ans_spec_m[n] = minf(state.ans_spec_m[n], thr_m[n]);
                state.ans_spec_s[n] = minf(state.ans_spec_s[n], thr_s[n]);
            }
        }

        for n in 0..=max_band {
            smr0.l[n] = maxf(smr0.l[n], smr1.l[n]);
            smr0.r[n] = maxf(smr0.r[n], smr1.r[n]);
            smr0.m[n] = maxf(smr0.m[n], smr1.m[n]);
            smr0.s[n] = maxf(smr0.s[n], smr1.s[n]);
        }

        ModelOutput {
            smr: smr0,
            transient_l,
            transient_r,
        }
    }
}

// ---------------------------------------------------------------------------
// static helpers from psy.c
// ---------------------------------------------------------------------------

fn subband_energy(
    max_band: usize,
    erg0: &mut [f32; 32],
    erg1: &mut [f32; 32],
    spec0: &[f32],
    spec1: &[f32],
) {
    let mut idx = 0usize;
    for k in 0..=max_band {
        let mut tmp0 = 0.0f32;
        let mut tmp1 = 0.0f32;
        for n in 0..16 {
            tmp0 += spec0[idx];
            tmp1 += spec1[idx];
            if n < 7 && k != 0 {
                let alias = idx as isize - 1 - (n as isize) * 2;
                tmp0 += BUTFLY[n] * (spec0[alias as usize] - spec0[idx]);
                tmp1 += BUTFLY[n] * (spec1[alias as usize] - spec1[idx]);
            } else if n > 8 && k != 31 {
                let alias = idx as isize + 31 - (n as isize) * 2;
                tmp0 += BUTFLY[15 - n] * (spec0[alias as usize] - spec0[idx]);
                tmp1 += BUTFLY[15 - n] * (spec1[alias as usize] - spec1[idx]);
            }
            idx += 1;
        }
        erg0[k] = tmp0;
        erg1[k] = tmp1;
    }
}

fn partition_energy(
    erg0: &mut [f32; PART_LONG],
    erg1: &mut [f32; PART_LONG],
    spec0: &[f32],
    spec1: &[f32],
) {
    let mut idx = 0usize;
    for n in 0..23 {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = spec0[idx];
        let mut e1 = spec1[idx];
        idx += 1;
        for _ in 0..k {
            e0 += spec0[idx];
            e1 += spec1[idx];
            idx += 1;
        }
        erg0[n] = e0;
        erg1[n] = e1;
    }
    for n in 23..48 {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = sqrtf(spec0[idx]);
        let mut e1 = sqrtf(spec1[idx]);
        idx += 1;
        for _ in 0..k {
            // The reference adds the double `sqrt` result to the float
            // accumulator and narrows once: `e0 += sqrt(x)`.
            e0 = (f64::from(e0) + f64::from(spec0[idx]).sqrt()) as f32;
            e1 = (f64::from(e1) + f64::from(spec1[idx]).sqrt()) as f32;
            idx += 1;
        }
        erg0[n] = e0 * e0 * IW[n];
        erg1[n] = e1 * e1 * IW[n];
    }
    for n in 48..PART_LONG {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = spec0[idx];
        let mut e1 = spec1[idx];
        idx += 1;
        for _ in 0..k {
            e0 += spec0[idx];
            e1 += spec1[idx];
            idx += 1;
        }
        erg0[n] = e0;
        erg1[n] = e1;
    }
}

fn weighted_partition_energy(
    erg0: &mut [f32; PART_LONG],
    erg1: &mut [f32; PART_LONG],
    spec0: &[f32],
    spec1: &[f32],
    cw0: &[f32],
    cw1: &[f32],
) {
    let mut idx = 0usize;
    for n in 0..23 {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = spec0[idx] * cw0[idx];
        let mut e1 = spec1[idx] * cw1[idx];
        idx += 1;
        for _ in 0..k {
            e0 += spec0[idx] * cw0[idx];
            e1 += spec1[idx] * cw1[idx];
            idx += 1;
        }
        erg0[n] = e0;
        erg1[n] = e1;
    }
    for n in 23..48 {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = sqrtf(spec0[idx] * cw0[idx]);
        let mut e1 = sqrtf(spec1[idx] * cw1[idx]);
        idx += 1;
        for _ in 0..k {
            // `e0 += sqrt(spec * cw)`, accumulated in double and narrowed once.
            e0 = (f64::from(e0) + f64::from(spec0[idx] * cw0[idx]).sqrt()) as f32;
            e1 = (f64::from(e1) + f64::from(spec1[idx] * cw1[idx]).sqrt()) as f32;
            idx += 1;
        }
        erg0[n] = e0 * e0 * IW[n];
        erg1[n] = e1 * e1 * IW[n];
    }
    for n in 48..PART_LONG {
        let k = (WH[n] - WL[n]) as usize;
        let mut e0 = spec0[idx] * cw0[idx];
        let mut e1 = spec1[idx] * cw1[idx];
        idx += 1;
        for _ in 0..k {
            e0 += spec0[idx] * cw0[idx];
            e1 += spec1[idx] * cw1[idx];
            idx += 1;
        }
        erg0[n] = e0;
        erg1[n] = e1;
    }
}

fn adapt_thresholds(max_line: usize, thr: &mut [f32; 1024]) {
    for n in 0..max_line {
        let m = n & 15;
        let mut tmp0 = thr[n];
        if m < 7 && n > 12 {
            let alias = -1 - (m as isize) * 2;
            let t = thr[(n as isize + alias) as usize] * INVBUTFLY[m];
            if t < tmp0 {
                tmp0 = t;
            }
        } else if m > 8 && n < 499 {
            let alias = 31 - (m as isize) * 2;
            let t = thr[(n as isize + alias) as usize] * INVBUTFLY[15 - m];
            if t < tmp0 {
                tmp0 = t;
            }
        }
        thr[512 + n] = tmp0;
    }
}

#[allow(clippy::too_many_arguments)]
fn calc_unpred(
    cvd_used: u8,
    max_line: usize,
    spec: &[f32],
    phase: &[f32],
    vocal: Option<&[i32]>,
    xsave: &mut [f32; SAVE_LEN],
    ysave: &mut [f32; SAVE_LEN],
    cw: &mut [f32; 512],
) {
    for n in 0..max_line {
        let ph = phase[n];
        ysave[n] = ph;
        let arg = ph - 2.0 * ysave[512 + n] + ysave[1024 + n];
        let tmp = my_cos(arg);
        xsave[n] = sqrtf(spec[n]);
        let amp = 2.0 * xsave[512 + n] - xsave[1024 + n];
        cw[n] = sqrtf(spec[n] + amp * (amp - 2.0 * xsave[n] * tmp)) / (xsave[n] + fabs(amp));
    }
    if cvd_used != 0 {
        if let Some(vocal) = vocal {
            for n in 0..MAX_CVD_LINE {
                if vocal[n] != 0 {
                    let thr = f64::from(CVD_UNPRED) * 0.01 * f64::from(vocal[n]);
                    if f64::from(cw[n]) > thr {
                        cw[n] = thr as f32;
                    }
                }
            }
        }
    }
}

fn spreading_signal(
    tables: &PsyTables,
    erg: &[f32; PART_LONG],
    werg: &[f32; PART_LONG],
    res: &mut [f32; PART_LONG],
    wres: &mut [f32; PART_LONG],
) {
    for k in 0..PART_LONG {
        let start = maxi(k as i32 - 5, 0) as usize;
        let stop = mini(k as i32 + 7, PART_LONG as i32 - 1) as usize;
        let e = erg[k];
        let ew = werg[k];
        for n in start..=stop {
            let s = tables.sprd_at(k, n);
            res[n] += s * e;
            wres[n] += s * ew;
        }
    }
}

fn apply_tonality_offset(
    tables: &PsyTables,
    erg0: &mut [f32; PART_LONG],
    erg1: &mut [f32; PART_LONG],
    werg0: &[f32; PART_LONG],
    werg1: &[f32; PART_LONG],
) {
    for n in 0..PART_LONG {
        let quot = werg0[n] / erg0[n];
        let offset = if quot <= 0.05737540597 {
            tables.o_max
        } else if quot < 0.5871011603 {
            tables.fac1 * pow(quot, tables.fac2)
        } else {
            tables.o_min
        };
        erg0[n] *= IW[n] * minf(tables.min_val[n], offset);

        let quot = werg1[n] / erg1[n];
        let offset = if quot <= 0.05737540597 {
            tables.o_max
        } else if quot < 0.5871011603 {
            tables.fac1 * pow(quot, tables.fac2)
        } else {
            tables.o_min
        };
        erg1[n] *= IW[n] * minf(tables.min_val[n], offset);
    }
}

fn adapt_ltq(
    params: &PsyParams,
    loud: &mut f32,
    tables: &PsyTables,
    erg0: &[f32; PART_LONG],
    erg1: &[f32; PART_LONG],
) -> f32 {
    let mut sum = 0.0f32;
    for n in 0..PART_LONG {
        sum += (erg0[n] + erg1[n]) * tables.loudness[n];
    }
    *loud = (0.98 * f64::from(*loud) + 0.02 * (0.5 * f64::from(sum))) as f32;
    1.0 + params.var_ltq * *loud * 5.023772e-08
}

fn calc_temporal_threshold(
    tables: &PsyTables,
    a: &mut [f32; PART_LONG],
    b: &mut [f32; PART_LONG],
    tau: &mut [f32; PART_LONG],
    frqthr: &mut [f32; PART_LONG],
    tmpthr: &mut [f32; PART_LONG],
) {
    for n in 0..PART_LONG {
        frqthr[n] *= tables.inv_ltq[n];
        tmpthr[n] *= tables.inv_ltq[n];
        let tmp = if tmpthr[n] > 1.0 {
            pow(tmpthr[n], tau[n])
        } else {
            1.0
        };
        a[n] += 0.5 * (frqthr[n] - a[n]);
        b[n] += 0.15 * (frqthr[n] - b[n]);
        if tmp < frqthr[n] {
            tau[n] = if a[n] <= b[n] {
                0.8
            } else {
                0.2 + b[n] / a[n] * 0.6
            };
        }
        tmpthr[n] = maxf(frqthr[n], tmp) * tables.part_ltq[n];
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::collapsible_match)]
fn calc_ms_threshold(
    ms_channelmode: u8,
    erg_l: &[f32; PART_LONG],
    erg_r: &[f32; PART_LONG],
    erg_m: &[f32; PART_LONG],
    erg_s: &[f32; PART_LONG],
    thr_l: &mut [f32; PART_LONG],
    thr_r: &mut [f32; PART_LONG],
    thr_m: &mut [f32; PART_LONG],
    thr_s: &mut [f32; PART_LONG],
) {
    for n in 0..PART_LONG {
        thr_s[n] = maxf(erg_m[n], erg_s[n]) / maxf(erg_l[n], erg_r[n]) * minf(thr_l[n], thr_r[n]);
        thr_m[n] = thr_s[n];
        match ms_channelmode {
            3 => {
                if n > 0 {
                    let ratio_ms = (if erg_m[n] > erg_s[n] {
                        erg_s[n] / erg_m[n]
                    } else {
                        erg_m[n] / erg_s[n]
                    }) as f64;
                    let ratio_lr = (if erg_l[n] > erg_r[n] {
                        erg_r[n] / erg_l[n]
                    } else {
                        erg_l[n] / erg_r[n]
                    }) as f64;
                    if ratio_ms < ratio_lr {
                        if erg_m[n] > erg_s[n] {
                            thr_s[n] = 1.0e18;
                            thr_l[n] = 1.0e18;
                            thr_r[n] = 1.0e18;
                        } else {
                            thr_m[n] = 1.0e18;
                            thr_l[n] = 1.0e18;
                            thr_r[n] = 1.0e18;
                        }
                    } else if erg_l[n] > erg_r[n] {
                        thr_r[n] = 1.0e18;
                        thr_m[n] = 1.0e18;
                        thr_s[n] = 1.0e18;
                    } else {
                        thr_l[n] = 1.0e18;
                        thr_m[n] = 1.0e18;
                        thr_s[n] = 1.0e18;
                    }
                }
            }
            4 => {
                if n > 0 {
                    let ratio_ms = (if erg_m[n] > erg_s[n] {
                        erg_s[n] / erg_m[n]
                    } else {
                        erg_m[n] / erg_s[n]
                    }) as f64;
                    let ratio_lr = (if erg_l[n] > erg_r[n] {
                        erg_r[n] / erg_l[n]
                    } else {
                        erg_l[n] / erg_r[n]
                    }) as f64;
                    if ratio_ms < ratio_lr {
                        if erg_m[n] > erg_s[n] {
                            thr_s[n] = 1.0e18;
                        } else {
                            thr_m[n] = 1.0e18;
                        }
                    } else if erg_l[n] > erg_r[n] {
                        thr_r[n] = 1.0e18;
                    } else {
                        thr_l[n] = 1.0e18;
                    }
                }
            }
            5 => {
                thr_s[n] *= 2.0;
            }
            6 => {}
            10 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.70794578 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] = MS2SPAT1 * thr_s[n] + (1.0 - MS2SPAT1) * tmp;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] = MS2SPAT1 * thr_m[n] + (1.0 - MS2SPAT1) * tmp;
                        }
                    }
                }
            }
            11 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.63095734 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] = MS2SPAT2 * thr_s[n] + (1.0 - MS2SPAT2) * tmp;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] = MS2SPAT2 * thr_m[n] + (1.0 - MS2SPAT2) * tmp;
                        }
                    }
                }
            }
            12 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.56234133 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] = MS2SPAT3 * thr_s[n] + (1.0 - MS2SPAT3) * tmp;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] = MS2SPAT3 * thr_m[n] + (1.0 - MS2SPAT3) * tmp;
                        }
                    }
                }
            }
            13 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.50118723 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] = MS2SPAT4 * thr_s[n] + (1.0 - MS2SPAT4) * tmp;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] = MS2SPAT4 * thr_m[n] + (1.0 - MS2SPAT4) * tmp;
                        }
                    }
                }
            }
            15 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.50118723 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] = tmp;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] = tmp;
                        }
                    }
                }
            }
            22 => {
                if 4.0 * f64::from(erg_l[n]) > f64::from(erg_r[n])
                    && f64::from(erg_l[n]) < 4.0 * f64::from(erg_r[n])
                {
                    let norm = 0.56234133 * IW[n];
                    if erg_m[n] > erg_s[n] {
                        let tmp = erg_s[n] * norm;
                        if thr_s[n] > tmp {
                            thr_s[n] =
                                max_d(f64::from(tmp), f64::from(erg_m[n] * IW[n]) * 0.025) as f32;
                        }
                    } else if erg_s[n] > erg_m[n] {
                        let tmp = erg_m[n] * norm;
                        if thr_m[n] > tmp {
                            thr_m[n] =
                                max_d(f64::from(tmp), f64::from(erg_s[n] * IW[n]) * 0.025) as f32;
                        }
                    }
                }
            }
            // The reference `default:` arm falls through to `case 10`, but no
            // profile selects a mode outside the explicit arms above.
            _ => {}
        }
    }
}

fn max_d(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

fn apply_ltq(
    tables: &PsyTables,
    thr0: &mut [f32; 1024],
    thr1: &mut [f32; 1024],
    part0: &[f32; PART_LONG],
    part1: &[f32; PART_LONG],
    adapted_ltq: f32,
    ms_flag: bool,
) {
    let ms = adapted_ltq * if ms_flag { 0.125 } else { 0.25 };
    let mut k = 0usize;
    for n in 0..PART_LONG {
        let tmp0 = sqrtf(part0[n]);
        let tmp1 = sqrtf(part1[n]);
        for _ in WL[n]..=WH[n] {
            let ltq = sqrtf(ms * tables.fft_ltq[k]);
            let t = tmp0 + ltq;
            thr0[k] = t * t;
            let t = tmp1 + ltq;
            thr1[k] = t * t;
            k += 1;
        }
    }
}

fn calculate_smr(
    max_band: usize,
    erg0: &[f32; 32],
    erg1: &[f32; 32],
    thr0: &[f32; 1024],
    thr1: &[f32; 1024],
    smr0: &mut [f32; 32],
    smr1: &mut [f32; 32],
) {
    for n in 0..=max_band {
        let base = n * 16;
        let mut tmp0 = thr0[base];
        let mut tmp1 = thr1[base];
        for k in 1..16 {
            if thr0[base + k] < tmp0 {
                tmp0 = thr0[base + k];
            }
            if thr1[base + k] < tmp1 {
                tmp1 = thr1[base + k];
            }
        }
        smr0[n] = 0.0625 * erg0[n] / tmp0;
        smr1[n] = 0.0625 * erg1[n] / tmp1;
    }
}

#[allow(clippy::needless_range_loop)]
fn calc_short_threshold(
    params: &PsyParams,
    erg: &[[f32; 128]; 4],
    thr: &mut [f32; PART_SHORT],
    old_erg: &mut [[f32; PART_SHORT]; 2],
    transient: &mut [i32; PART_SHORT],
) {
    for k in 0..PART_SHORT {
        transient[k] = 0;
        let mut th = old_erg[0][k];
        for n in 0..4 {
            let lo = WL_SHORT[k] as usize;
            let hi = WH_SHORT[k] as usize;
            let mut new_erg = erg[n][lo];
            for i in lo + 1..=hi {
                new_erg += erg[n][i];
            }
            if new_erg > old_erg[0][k] {
                if new_erg > old_erg[0][k] * params.trans_detect
                    || new_erg > old_erg[1][k] * params.trans_detect * 2.0
                {
                    transient[k] = 1;
                }
            } else {
                th = minf(th, new_erg);
            }
            old_erg[1][k] = old_erg[0][k];
            old_erg[0][k] = new_erg;
        }
        thr[k] = th * params.short_thr * IW_SHORT[k];
    }
}

fn preecho_control(
    part0: &mut [f32; PART_LONG],
    pre0: &mut [f32; PART_LONG],
    sim0: &[f32; PART_LONG],
    part1: &mut [f32; PART_LONG],
    pre1: &mut [f32; PART_LONG],
    sim1: &[f32; PART_LONG],
) {
    for n in 0..PART_LONG {
        part0[n] = minf(sim0[n], pre0[n] * PREFAC_LONG as f32);
        part1[n] = minf(sim1[n], pre1[n] * PREFAC_LONG as f32);
        pre0[n] = sim0[n];
        pre1[n] = sim1[n];
    }
}

/// `RaiseSMR_Signal`.
fn raise_smr_signal(max_band: i32, signal: &mut [f32; 32], tmp: f32) {
    let mut z = 0.0f32;
    for band in (0..=max_band).rev() {
        let b = band as usize;
        if z < signal[b] {
            z = signal[b];
        }
        if z > tmp {
            z = tmp;
        }
        if signal[b] < z {
            signal[b] = z;
        }
    }
}

/// `RaiseSMR`: raises every SMR to a minimum and monotonically toward the
/// highest band's value.
pub fn raise_smr(max_band: i32, min_smr: f32, smr: &mut Smr) {
    let tmp = pow10_d(0.1 * f64::from(min_smr));
    raise_smr_signal(max_band, &mut smr.l, tmp);
    raise_smr_signal(max_band, &mut smr.r, tmp);
    raise_smr_signal(max_band, &mut smr.m, tmp);
    raise_smr_signal(max_band, &mut smr.s, (0.5 * f64::from(tmp)) as f32);
}

/// `MS_LR_Entscheidung`: chooses M/S or L/R per band by lowest perceptual
/// entropy and, when M/S wins, rewrites `smr` and the subband samples.
pub fn ms_lr_entscheidung(
    max_band: i32,
    ms: &mut [u8; 32],
    smr: &mut Smr,
    x_l: &mut [[f32; 36]; 32],
    x_r: &mut [[f32; 36]; 32],
) {
    for band in 0..=max_band as usize {
        let mut pe_lr = 1.0f32;
        let mut pe_ms = 1.0f32;
        if f64::from(smr.l[band]) > 1.0 {
            pe_lr *= smr.l[band];
        }
        if f64::from(smr.r[band]) > 1.0 {
            pe_lr *= smr.r[band];
        }
        if f64::from(smr.m[band]) > 1.0 {
            pe_ms *= smr.m[band];
        }
        if f64::from(smr.s[band]) > 1.0 {
            pe_ms *= smr.s[band];
        }
        if pe_ms < pe_lr {
            ms[band] = 1;
            for n in 0..36 {
                let l = x_l[band][n];
                let r = x_r[band][n];
                x_l[band][n] = (l + r) * 0.5;
                x_r[band][n] = (l - r) * 0.5;
            }
            smr.l[band] = smr.m[band];
            smr.r[band] = smr.s[band];
        } else {
            ms[band] = 0;
        }
    }
}

/// `TransientenCalc`: expands the short-partition transient flags to FFT
/// partitions (`wl_short[i] >> 2 ..= wh_short[i] >> 2`).
pub fn transienten_calc(t_l: &[i32; PART_SHORT], t_r: &[i32; PART_SHORT]) -> [i32; 32] {
    let mut t = [0i32; 32];
    for i in 0..PART_SHORT {
        if t_l[i] != 0 || t_r[i] != 0 {
            let mut x1 = WL_SHORT[i] >> 2;
            let x2 = WH_SHORT[i] >> 2;
            while x1 <= x2 {
                t[x1 as usize] = 1;
                x1 += 1;
            }
        }
    }
    t
}
