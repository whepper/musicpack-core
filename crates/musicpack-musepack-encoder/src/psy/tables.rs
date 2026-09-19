//! Psychoacoustic constants, partition tables and frozen numerical tables.
//!
//! The partition geometry (`wl`, `wh`, `iw`, `wl_short`, `wh_short`,
//! `iw_short`) is literal integer/float source data from `psy_tab.c`. The
//! libm-derived tables (`SPRD`, `MinVal`, `Loudness`, `fftLtq`, `partLtq`,
//! `invLtq`, `O_*`, `FAC*`) and the FFT/fast-math kernels are **frozen** as
//! IEEE-754 bit patterns in [`super::frozen`]; see `tests/data/psy/`.

#![allow(dead_code)]

use super::frozen;

/// Number of long partitions (`PART_LONG`).
pub const PART_LONG: usize = 57;
/// Number of short partitions (`PART_SHORT = PART_LONG / 3`).
pub const PART_SHORT: usize = 19;
/// Maximum FFT lines fed to the noise shaper (`MAX_ANS_LINES`).
pub const MAX_ANS_LINES: usize = 512;
/// Maximum FFT index for CVD voice detection (`MAX_CVD_LINE`).
pub const MAX_CVD_LINE: usize = 300;
/// Short-FFT analysis offset (`SHORTFFT_OFFSET`).
pub const SHORTFFT_OFFSET: usize = 168;
/// Long-partition pre-echo factor (`PREFAC_LONG`).
pub const PREFAC_LONG: i32 = 10;
/// Unpredictability assigned to CVD-detected lines (`CVD_UNPRED`).
pub const CVD_UNPRED: f32 = 0.040;
/// `MIN_ANALYZED_IDX`.
pub const MIN_ANALYZED_IDX: usize = 12;
/// `MED_ANALYZED_IDX`.
pub const MED_ANALYZED_IDX: usize = 50;
/// `MAX_ANALYZED_IDX`.
pub const MAX_ANALYZED_IDX: usize = 900;
/// `MS2SPAT1`.
pub const MS2SPAT1: f32 = 0.5;
/// `MS2SPAT2`.
pub const MS2SPAT2: f32 = 0.25;
/// `MS2SPAT3`.
pub const MS2SPAT3: f32 = 0.125;
/// `MS2SPAT4`.
pub const MS2SPAT4: f32 = 0.0625;

/// The degenerate pre-echo/post-mask seed: `partLtq` computed while
/// `SampleFreq == 0` and the default (zero) ear model, i.e. `(float)pow(10,-2.3)`.
/// Frozen so the state seed is host-independent.
pub const INIT_PART_LTQ: f32 = f32::from_bits(0x3BA4_3AA2);

/// Antialiasing weights for the subband power (`Butfly[7]`).
pub const BUTFLY: [f32; 7] = [0.5, 0.2776, 0.1176, 0.0361, 0.0075, 0.000948, 0.0000598];
/// Antialiasing weights for the masking thresholds (`InvButfly[7]`).
pub const INVBUTFLY: [f32; 7] = [2.0, 3.6023, 8.5034, 27.701, 133.33, 1054.852, 16722.408];

/// `w_low` for long partitions.
pub const WL: [i32; PART_LONG] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31, 33, 35, 38, 41,
    44, 47, 50, 54, 58, 62, 67, 72, 78, 84, 91, 98, 106, 115, 124, 134, 145, 157, 170, 184, 199,
    216, 234, 254, 276, 301, 329, 360, 396, 437, 485,
];
/// `w_high` for long partitions.
pub const WH: [i32; PART_LONG] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 37, 40, 43,
    46, 49, 53, 57, 61, 66, 71, 77, 83, 90, 97, 105, 114, 123, 133, 144, 156, 169, 183, 198, 215,
    233, 253, 275, 300, 328, 359, 395, 436, 484, 511,
];
/// Inverse partition width for long partitions.
pub const IW: [f32; PART_LONG] = [
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 3.0,
    1.0 / 3.0,
    1.0 / 3.0,
    1.0 / 3.0,
    1.0 / 3.0,
    1.0 / 4.0,
    1.0 / 4.0,
    1.0 / 4.0,
    1.0 / 5.0,
    1.0 / 5.0,
    1.0 / 6.0,
    1.0 / 6.0,
    1.0 / 7.0,
    1.0 / 7.0,
    1.0 / 8.0,
    1.0 / 9.0,
    1.0 / 9.0,
    1.0 / 10.0,
    1.0 / 11.0,
    1.0 / 12.0,
    1.0 / 13.0,
    1.0 / 14.0,
    1.0 / 15.0,
    1.0 / 17.0,
    1.0 / 18.0,
    1.0 / 20.0,
    1.0 / 22.0,
    1.0 / 25.0,
    1.0 / 28.0,
    1.0 / 31.0,
    1.0 / 36.0,
    1.0 / 41.0,
    1.0 / 48.0,
    1.0 / 27.0,
];
/// `w_low` for short partitions.
pub const WL_SHORT: [i32; PART_SHORT] = [
    0, 1, 2, 3, 4, 5, 6, 8, 10, 12, 15, 18, 23, 29, 36, 46, 59, 75, 99,
];
/// `w_high` for short partitions.
pub const WH_SHORT: [i32; PART_SHORT] = [
    0, 1, 2, 3, 5, 6, 7, 9, 12, 14, 18, 23, 29, 36, 46, 58, 75, 99, 127,
];
/// Inverse partition width for short partitions.
pub const IW_SHORT: [f32; PART_SHORT] = [
    1.0,
    1.0,
    1.0,
    1.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0 / 3.0,
    1.0 / 3.0,
    1.0 / 4.0,
    1.0 / 6.0,
    1.0 / 7.0,
    1.0 / 8.0,
    1.0 / 11.0,
    1.0 / 13.0,
    1.0 / 17.0,
    1.0 / 25.0,
    1.0 / 29.0,
];

/// Converts a frozen `[u32; N]` bit table to `[f32; N]` at compile time.
const fn to_f32<const N: usize>(bits: [u32; N]) -> [f32; N] {
    let mut out = [0.0f32; N];
    let mut i = 0;
    while i < N {
        out[i] = f32::from_bits(bits[i]);
        i += 1;
    }
    out
}

/// Frozen 2048-point FFT twiddle table `w`.
pub(crate) const W: [f32; 4096] = to_f32(frozen::W_BITS);
/// Frozen 256-point analysis window.
pub(crate) const HANN_256: [f32; 256] = to_f32(frozen::HANN_256_BITS);
/// Frozen 1024-point analysis window.
pub(crate) const HANN_1024: [f32; 1024] = to_f32(frozen::HANN_1024_BITS);
/// Frozen 1600-point (centered in 2048) analysis window.
pub(crate) const HANN_1600: [f32; 1600] = to_f32(frozen::HANN_1600_BITS);
/// Frozen `my_cos` interpolation table (`tabcos`).
pub(crate) const TABCOS: [f32; 3330] = to_f32(frozen::TABCOS_BITS);
/// Frozen `my_atan2` interpolation table (`tabatan2`).
pub(crate) const TABATAN2: [f32; 258] = to_f32(frozen::TABATAN2_BITS);

/// Resolved floating-point psychoacoustic tables for one configuration.
#[derive(Clone)]
pub struct PsyTables {
    /// `Max_Band` selected by `Init_Psychoakustiktabellen`.
    pub max_band: i32,
    /// Threshold in quiet in FFT resolution.
    pub fft_ltq: [f32; 512],
    /// Threshold in quiet per long partition.
    pub part_ltq: [f32; PART_LONG],
    /// Inverse threshold in quiet per long partition.
    pub inv_ltq: [f32; PART_LONG],
    /// Minimum tonality offsets.
    pub min_val: [f32; PART_LONG],
    /// Loudness weighting factors.
    pub loudness: [f32; PART_LONG],
    /// Tabulated spreading function, source-major.
    pub sprd: [f32; PART_LONG * PART_LONG],
    /// Tonality-offset maximum (`O_MAX`).
    pub o_max: f32,
    /// Tonality-offset minimum (`O_MIN`).
    pub o_min: f32,
    /// `FAC1`.
    pub fac1: f32,
    /// `FAC2`.
    pub fac2: f32,
}

impl PsyTables {
    /// Expands the frozen bit tables for the given configuration.
    #[must_use]
    pub fn from_bits(b: &frozen::PsyTablesBits) -> Self {
        Self {
            max_band: b.max_band,
            fft_ltq: to_f32(b.fft_ltq),
            part_ltq: to_f32(b.part_ltq),
            inv_ltq: to_f32(b.inv_ltq),
            min_val: to_f32(b.min_val),
            loudness: to_f32(b.loudness),
            sprd: to_f32(b.sprd),
            o_max: f32::from_bits(b.scalars[0]),
            o_min: f32::from_bits(b.scalars[1]),
            fac1: f32::from_bits(b.scalars[2]),
            fac2: f32::from_bits(b.scalars[3]),
        }
    }

    /// `SPRD[source][target]`.
    #[inline]
    pub fn sprd_at(&self, source: usize, target: usize) -> f32 {
        self.sprd[source * PART_LONG + target]
    }
}

/// Returns the frozen tables for a `(quality, sample rate)` pair.
///
/// Only the combinations exercised by the oracle are frozen; other pairs
/// return `None`. Extending coverage means re-running the oracle tool.
#[must_use]
pub fn frozen_psy_tables(qual: f32, sample_rate: f32) -> Option<&'static frozen::PsyTablesBits> {
    let q = qual.to_bits();
    let r = sample_rate.to_bits();
    let q4 = 4.0f32.to_bits();
    let q5 = 5.0f32.to_bits();
    let q6 = 6.0f32.to_bits();
    let q7 = 7.0f32.to_bits();
    let s44 = 44100.0f32.to_bits();
    let s48 = 48000.0f32.to_bits();
    let s37 = 37800.0f32.to_bits();
    let s32 = 32000.0f32.to_bits();
    match (q, r) {
        _ if q == q4 && r == s44 => Some(&frozen::PSY_Q4_44100),
        _ if q == q5 && r == s44 => Some(&frozen::PSY_Q5_44100),
        _ if q == q6 && r == s44 => Some(&frozen::PSY_Q6_44100),
        _ if q == q7 && r == s44 => Some(&frozen::PSY_Q7_44100),
        _ if q == q5 && r == s48 => Some(&frozen::PSY_Q5_48000),
        _ if q == q5 && r == s37 => Some(&frozen::PSY_Q5_37800),
        _ if q == q5 && r == s32 => Some(&frozen::PSY_Q5_32000),
        _ => None,
    }
}
