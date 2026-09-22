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

/// The complete integer SV8 quality matrix: `(quality, sample rate)` pairs
/// accepted by the reference `mpcenc` for which tables are frozen.
///
/// Integer quality `0..=10` × the four SV8 sample rates = 44 configurations.
/// Each entry maps the exact `f32` bit patterns of the pair onto the const
/// transcribed from the C oracle dump `tests/data/psy/psy_<q>-<rate>.txt`.
const MATRIX: [(u32, u32, &frozen::PsyTablesBits); 44] = [
    (
        0.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q0_44100,
    ),
    (
        0.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q0_48000,
    ),
    (
        0.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q0_37800,
    ),
    (
        0.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q0_32000,
    ),
    (
        1.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q1_44100,
    ),
    (
        1.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q1_48000,
    ),
    (
        1.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q1_37800,
    ),
    (
        1.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q1_32000,
    ),
    (
        2.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q2_44100,
    ),
    (
        2.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q2_48000,
    ),
    (
        2.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q2_37800,
    ),
    (
        2.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q2_32000,
    ),
    (
        3.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q3_44100,
    ),
    (
        3.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q3_48000,
    ),
    (
        3.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q3_37800,
    ),
    (
        3.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q3_32000,
    ),
    (
        4.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q4_44100,
    ),
    (
        4.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q4_48000,
    ),
    (
        4.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q4_37800,
    ),
    (
        4.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q4_32000,
    ),
    (
        5.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q5_44100,
    ),
    (
        5.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q5_48000,
    ),
    (
        5.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q5_37800,
    ),
    (
        5.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q5_32000,
    ),
    (
        6.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q6_44100,
    ),
    (
        6.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q6_48000,
    ),
    (
        6.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q6_37800,
    ),
    (
        6.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q6_32000,
    ),
    (
        7.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q7_44100,
    ),
    (
        7.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q7_48000,
    ),
    (
        7.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q7_37800,
    ),
    (
        7.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q7_32000,
    ),
    (
        8.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q8_44100,
    ),
    (
        8.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q8_48000,
    ),
    (
        8.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q8_37800,
    ),
    (
        8.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q8_32000,
    ),
    (
        9.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q9_44100,
    ),
    (
        9.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q9_48000,
    ),
    (
        9.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q9_37800,
    ),
    (
        9.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q9_32000,
    ),
    (
        10.0f32.to_bits(),
        44100.0f32.to_bits(),
        &frozen::PSY_Q10_44100,
    ),
    (
        10.0f32.to_bits(),
        48000.0f32.to_bits(),
        &frozen::PSY_Q10_48000,
    ),
    (
        10.0f32.to_bits(),
        37800.0f32.to_bits(),
        &frozen::PSY_Q10_37800,
    ),
    (
        10.0f32.to_bits(),
        32000.0f32.to_bits(),
        &frozen::PSY_Q10_32000,
    ),
];

/// Returns the frozen tables for a `(quality, sample rate)` pair.
///
/// The frozen set covers the complete **integer SV8 quality matrix** of the
/// reference `mpcenc`: integer qualities `0..=10` at 44100, 48000, 37800 and
/// 32000 Hz — 44 configurations total. Quality and rate are matched by exact
/// `f32` bit equality, so any other pair returns `None`; the caller then
/// falls back to the deterministic computed path
/// ([`super::computed`], J.2) for finite qualities at an SV8 rate, or fails
/// closed with [`crate::error::EncoderError::UnsupportedPsyConfig`] for
/// non-SV8 rates.
///
/// The lookup itself must stay exact-bit: these frozen tables are the
/// permanent C-oracle regression oracles the computed path is checked
/// against (see `psy::computed`'s `computed_tables_match_the44_frozen_...`),
/// and there is no quantization, rounding or fractional grid anywhere.
#[must_use]
pub fn frozen_psy_tables(qual: f32, sample_rate: f32) -> Option<&'static frozen::PsyTablesBits> {
    let q = qual.to_bits();
    let r = sample_rate.to_bits();
    MATRIX
        .iter()
        .find(|(mq, mr, _)| *mq == q && *mr == r)
        .map(|(_, _, tables)| *tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// Integer qualities `0..=10` (independent of `MATRIX`).
    const QUALITIES: [f32; 11] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    /// The four SV8 sample rates.
    const RATES: [f32; 4] = [44100.0, 48000.0, 37800.0, 32000.0];

    fn dump_path(qual: f32, rate: f32) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/psy")
            .join(format!("psy_q{qual}-{rate}.txt"))
    }

    /// Parses one committed C-oracle dump `psy_<q>-<rate>.txt`, returning
    /// `(max_band, [(field name, bits)])` in file order.
    fn parse_dump(qual: f32, rate: f32) -> (i32, Vec<(&'static str, Vec<u32>)>) {
        let text = std::fs::read_to_string(dump_path(qual, rate))
            .unwrap_or_else(|e| panic!("missing oracle dump for q{qual} @{rate}: {e}"));
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let first = lines.next().expect("dump header");
        let mut header = first.split_whitespace();
        assert_eq!(header.next(), Some("max_band"));
        let max_band: i32 = header.next().expect("max_band value").parse().unwrap();
        let mut sections = Vec::new();
        while let Some(name) = lines.next() {
            let mut parts = name.split_whitespace();
            let key = parts.next().unwrap();
            let count: usize = parts.next().expect("section count").parse().unwrap();
            let bits: Vec<u32> = (0..count)
                .map(|_| {
                    let line = lines.next().unwrap_or_else(|| panic!("short dump {key}"));
                    u32::from_str_radix(line.trim(), 16).expect("hex bits")
                })
                .collect();
            let named = match key {
                "fftLtq" => "fft_ltq",
                "partLtq" => "part_ltq",
                "invLtq" => "inv_ltq",
                "MinVal" => "min_val",
                "Loudness" => "loudness",
                "SPRD" => "sprd",
                "scalars" => "scalars",
                other => panic!("unknown dump section {other}"),
            };
            sections.push((named, bits));
        }
        (max_band, sections)
    }

    fn table_bits(t: &frozen::PsyTablesBits) -> Vec<(&'static str, &[u32])> {
        vec![
            ("fft_ltq", &t.fft_ltq),
            ("part_ltq", &t.part_ltq),
            ("inv_ltq", &t.inv_ltq),
            ("min_val", &t.min_val),
            ("loudness", &t.loudness),
            ("sprd", &t.sprd),
            ("scalars", &t.scalars),
        ]
    }

    /// Every integer `(quality, rate)` pair resolves to exactly one frozen
    /// table, each pair points at a distinct const, and every bit matches the
    /// committed dump the C oracle produced for that pair. This is the
    /// psy-layer acceptance test for the 44-configuration matrix: lookup
    /// succeeds, the expected table identity is selected (a mis-wired pair
    /// would compare against the wrong dump and fail), no fallback occurs,
    /// and the values equal the C oracle data.
    #[test]
    fn integer_matrix_resolves_to_distinct_tables_matching_the_c_oracle_dumps() {
        let mut seen_keys = BTreeSet::new();
        let mut seen_ptrs = BTreeSet::new();
        for &qual in &QUALITIES {
            for &rate in &RATES {
                let tables = frozen_psy_tables(qual, rate)
                    .unwrap_or_else(|| panic!("q{qual} @{rate} Hz must resolve"));
                seen_keys.insert((qual.to_bits(), rate.to_bits()));
                seen_ptrs.insert(std::ptr::from_ref(tables) as usize);

                let (max_band, sections) = parse_dump(qual, rate);
                assert_eq!(
                    tables.max_band, max_band,
                    "q{qual} @{rate}: max_band differs from the C oracle dump"
                );
                let own = table_bits(tables);
                assert_eq!(own.len(), sections.len());
                for ((field, bits), (dump_field, dump_bits)) in own.iter().zip(&sections) {
                    assert_eq!(field, dump_field, "q{qual} @{rate}: section order");
                    assert_eq!(
                        *bits,
                        dump_bits.as_slice(),
                        "q{qual} @{rate}: `{field}` differs from the C oracle dump"
                    );
                }
            }
        }
        assert_eq!(
            seen_keys.len(),
            44,
            "the matrix must cover 44 distinct pairs"
        );
        assert_eq!(
            seen_ptrs.len(),
            44,
            "each pair must resolve to its own frozen table (no aliasing/fallback)"
        );
    }

    /// Anything outside the integer matrix keeps missing the *frozen*
    /// lookup — exact `f32` bit equality, no quantization, no fractional
    /// grid (J.1/J.2 boundary pin): fractional/out-of-range qualities take
    /// the computed path (`psy::computed`) instead, and non-SV8 rates stay
    /// rejected end to end.
    #[test]
    fn frozen_lookup_requires_exact_integer_matrix_pairs() {
        let rejected: &[(f32, f32)] = &[
            (5.5, 44100.0),       // fractional -> computed path
            (4.25, 44100.0),      // the C "centesimal" example -> computed path
            (6.0000005, 44100.0), // not the exact integer bit pattern
            (11.0, 44100.0),      // C clips to q10 -> computed path
            (-1.0, 44100.0),      // C clips to q0 -> computed path
            (5.0, 96000.0),       // not an SV8 rate (fails closed everywhere)
            (5.0, 22050.0),       // not an SV8 rate
            (8.0, 44101.0),       // near-miss rate
        ];
        for &(qual, rate) in rejected {
            assert!(
                frozen_psy_tables(qual, rate).is_none(),
                "q{qual} @{rate} must stay rejected"
            );
        }
    }
}
