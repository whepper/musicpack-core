//! Deterministic fractional-quality psychoacoustic tables (slice J.2).
//!
//! Reproduces the reference `Init_Psychoakustiktabellen` outputs for **any
//! finite `f32` quality** at the four SV8 sample rates from the small frozen
//! primitive set the J.2 de-risking experiment validated:
//!
//! * profile selection/interpolation: the existing [`PsyParams::from_quality`];
//! * the quality-independent `Loudness`/`SPRD` sections: reused from the
//!   frozen `q5` config of the same rate (proven quality-independent);
//! * the frozen ATH base arrays (one per `(rate, EarModelFlag)`, captured
//!   from the C oracle after the flag roll-off and before the `Ltq_max`
//!   clamp);
//! * `math::{pow10_d, mind, minf}` — never substituted.
//!
//! Out-of-range qualities are clipped to `[0, 10]` inside
//! `PsyParams::from_quality`, exactly like `profile.c` (`clip(qual, 0, 10)`).
//! Non-finite qualities never reach this module: the callers reject them with
//! [`crate::error::EncoderError::NonFiniteQuality`] (C's `NaN` path is
//! undefined behaviour — an intentional compatibility boundary).
//!
//! The44 integer configurations keep using their frozen tables
//! unconditionally (`frozen_psy_tables` is consulted first); the unit tests
//! below prove the computed path independently reproduces those frozen
//! tables bit-for-bit, which is the permanent
//! `computed integer tables == frozen C oracle tables` invariant.

use super::frozen::{PSY_ATH_BASES, PsyTablesBits};
use super::math::{mind, minf, pow10_d};
use super::profile::PsyParams;
use super::tables::{PART_LONG, WH, WL, frozen_psy_tables};

/// The `Bass` `lfe` attenuation literal (`psy_tab.c`; exact integers, so the
/// `unsigned char -> float` promotion is exact).
const LFE: [f32; 11] = [
    120.0, 100.0, 80.0, 60.0, 50.0, 40.0, 30.0, 20.0, 15.0, 10.0, 5.0,
];

/// `POW10` for table generation: the existing `math::pow10_d`, guarded to
/// the `[-13, 13]` reference-equivalence domain proven by
/// `tests/data/psy/math.txt` (every argument observed by the J.2 experiment
/// and the oracle corpus lies strictly inside it).
#[inline]
fn pow10_table(x: f64) -> f32 {
    debug_assert!(
        (-13.0f64..=13.0).contains(&x),
        "pow10 argument {x} outside the proven reference domain [-13, 13]"
    );
    pow10_d(x)
}

/// Verbatim port of `psy_tab.c` `Bass`: the mixed `f32`/`f64` promotion of
/// each branch is preserved exactly (cases `0..=10` compute in `f32` and
/// widen on return; cases `19..=26` compute in `f64`).
fn bass_c(f: f32, tmn: f32, nmt: f32, bass: f32) -> f64 {
    let idx = (1024.0f64 / 44100.0 * f64::from(f) + 0.5) as i32;
    match idx {
        0..=10 => f64::from(tmn + bass * LFE[idx as usize]),
        11..=18 => f64::from(tmn),
        19..=22 => f64::from(tmn) * 0.75 + f64::from(nmt) * 0.25,
        23..=24 => f64::from(tmn) * 0.50 + f64::from(nmt) * 0.50,
        25..=26 => f64::from(tmn) * 0.25 + f64::from(nmt) * 0.75,
        _ => f64::from(nmt),
    }
}

/// Computes the complete table set for `(qual, rate)` in the reference
/// sequence and with the reference numeric types.
///
/// Returns `None` when the rate has no frozen quality-independent sibling
/// (i.e. not one of the four SV8 rates), preserving the fail-closed
/// `UnsupportedPsyConfig` path.
pub(crate) fn psy_tables(qual: f32, rate: f32) -> Option<PsyTablesBits> {
    // Non-finite qualities are rejected by the callers; belt-and-braces so
    // this module can never silently propagate a `NaN` into table bits.
    if !qual.is_finite() {
        return None;
    }
    // Quality-independent sections: reuse this rate's frozen q5 data
    // (`Loudness`/`SPRD` depend on the rate only; proven by the J.2
    // investigation and re-checked against every committed dump in tests).
    // `None` for non-SV8 rates keeps the fail-closed gate intact.
    let q5 = frozen_psy_tables(5.0, rate)?;
    let p = PsyParams::from_quality(qual); // includes the C clip to [0, 10]

    // Init_Psychoakustiktabellen: Max_Band first —
    // (int)(BandWidth *64. / SampleFreq), then clamped to 1..=31.
    let max_band = if rate != 0.0 {
        (f64::from(p.band_width) * 64.0 / f64::from(rate)) as i32
    } else {
        0
    };
    let max_band = max_band.clamp(1, 31);

    // Tonalitaetskoeffizienten: `bass` clamps (MinValChoice is discrete per
    // integer profile row), then MinVal over the long partitions.
    let mut bass = (0.1f64 / 8.0 * f64::from(p.nmt)) as f32;
    if p.min_val_choice <= 2 && f64::from(bass) > 0.1 {
        bass = 0.1f32;
    }
    if p.min_val_choice <= 1 {
        bass = 0.0f32;
    }
    let mut min_val = [0u32; PART_LONG];
    for n in 0..PART_LONG {
        // C: `Bass((wl[n] + wh[n]) /2048. * SampleFreq, ...)` — f64 division
        // and multiplication, then the `float f` parameter conversion
        // truncates to f32 at the call boundary.
        let fc = ((WL[n] as f64 + WH[n] as f64) / 2048.0 * f64::from(rate)) as f32;
        let tmp = bass_c(fc, p.tmn, p.nmt, bass);
        min_val[n] = pow10_table(-0.1 * tmp).to_bits();
    }

    // Tonality-offset constants. FAC1 keeps the C evaluation order:
    // f32 `(TMN - NMT)`, then f64 `* 0.229`, then the f64 expression into
    // POW10. FAC2 multiplies the f64 constant product and stores to f32.
    let o_max = pow10_table(-0.1 * f64::from(p.tmn));
    let o_min = pow10_table(-0.1 * f64::from(p.nmt));
    let d = p.tmn - p.nmt; // C: (m->TMN - m->NMT) stays f32
    let fac1 = pow10_table(-0.1 * (f64::from(p.nmt) - f64::from(d) * 0.229));
    let fac2 = (f64::from(d) * (0.99011159f64 * 0.1)) as f32;

    // Ruhehoerschwelle tail from the frozen base for this rate's discrete
    // EarModelFlag: `(int)` truncations at the C parameter conversion, f64
    // `mind`, `+= Ltq_offset -23`, then POW10(0.1 * tmp).
    let rate_bits = rate.to_bits();
    let (_, _, words) = PSY_ATH_BASES
        .iter()
        .find(|(r, f, _)| *r == rate_bits && *f == p.ear_model_flag)?;
    let off_i = p.ltq_offset as i32; // C: float -> int parameter conversion
    let lmax_i = p.ltq_max as i32;
    let mut fft_ltq = [0u32; 512];
    for n in 0..512 {
        let tmp = mind(f64::from_bits(words[n]), lmax_i as f64);
        let tmp = tmp + (off_i as f64 - 23.0);
        fft_ltq[n] = pow10_table(0.1 * tmp).to_bits();
    }

    // Threshold in quiet per long partition (f32 min over the FFT values)
    // and its f32 reciprocal.
    let mut part_ltq = [0u32; PART_LONG];
    let mut inv_ltq = [0u32; PART_LONG];
    for n in 0..PART_LONG {
        let mut erg = 1.0e20f32;
        for k in WL[n]..=WH[n] {
            erg = minf(erg, f32::from_bits(fft_ltq[k as usize]));
        }
        part_ltq[n] = erg.to_bits();
        inv_ltq[n] = (1.0f32 / f32::from_bits(part_ltq[n])).to_bits();
    }

    Some(PsyTablesBits {
        max_band,
        fft_ltq,
        part_ltq,
        inv_ltq,
        min_val,
        loudness: q5.loudness,
        sprd: q5.sprd,
        scalars: [
            o_max.to_bits(),
            o_min.to_bits(),
            fac1.to_bits(),
            fac2.to_bits(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fractional oracle pairs: `(quality string, rate)` exactly as the
    /// committed `psy_q<qual>-<rate>.txt` files are named.
    const FRAC_PAIRS: &[(&str, f32)] = &[
        ("4.25", 44100.0),
        ("4.25", 48000.0),
        ("4.25", 37800.0),
        ("4.25", 32000.0),
        ("5.5", 44100.0),
        ("5.5", 48000.0),
        ("5.5", 37800.0),
        ("5.5", 32000.0),
        ("6.5", 44100.0),
        ("6.5", 48000.0),
        ("6.5", 37800.0),
        ("6.5", 32000.0),
        ("8.5", 44100.0),
        ("8.5", 48000.0),
        ("8.5", 37800.0),
        ("8.5", 32000.0),
        ("4.9999999", 44100.0),
        ("5.0000001", 44100.0),
        ("5.9999999", 44100.0),
        ("6.0000001", 44100.0),
        ("4.2501", 44100.0),
        ("6.0000005", 44100.0),
        ("9.9999", 44100.0),
    ];

    const RATES: [f32; 4] = [44100.0, 48000.0, 37800.0, 32000.0];

    /// Parses a committed C-oracle dump (`psy_q*-*.txt`) into the same
    /// `PsyTablesBits` representation the computed path produces, so a
    /// comparison reports the first differing value, not just a hash.
    fn load_dump(text: &str) -> PsyTablesBits {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let first = lines.next().expect("dump header");
        let mut head = first.split_whitespace();
        assert_eq!(head.next(), Some("max_band"));
        let max_band: i32 = head.next().expect("max_band value").parse().unwrap();
        let mut fft_ltq = [0u32; 512];
        let mut part_ltq = [0u32; PART_LONG];
        let mut inv_ltq = [0u32; PART_LONG];
        let mut min_val = [0u32; PART_LONG];
        let mut loudness = [0u32; PART_LONG];
        let mut sprd = [0u32; PART_LONG * PART_LONG];
        let mut scalars = [0u32; 4];
        while let Some(name) = lines.next() {
            let mut parts = name.split_whitespace();
            let key = parts.next().unwrap();
            let count: usize = parts.next().expect("section count").parse().unwrap();
            let mut read = |dst: &mut [u32]| {
                assert_eq!(dst.len(), count, "section {key} length mismatch");
                for slot in dst.iter_mut() {
                    let line = lines.next().unwrap_or_else(|| panic!("short dump {key}"));
                    *slot = u32::from_str_radix(line.trim(), 16).expect("hex bits");
                }
            };
            match key {
                "fftLtq" => read(&mut fft_ltq),
                "partLtq" => read(&mut part_ltq),
                "invLtq" => read(&mut inv_ltq),
                "MinVal" => read(&mut min_val),
                "Loudness" => read(&mut loudness),
                "SPRD" => read(&mut sprd),
                "scalars" => read(&mut scalars),
                other => panic!("unknown dump section {other}"),
            }
        }
        PsyTablesBits {
            max_band,
            fft_ltq,
            part_ltq,
            inv_ltq,
            min_val,
            loudness,
            sprd,
            scalars,
        }
    }

    fn section_first_diff(name: &str, a: &[u32], b: &[u32]) -> Option<String> {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .enumerate()
            .find(|(_i, (x, y))| x != y)
            .map(|(i, (x, y))| format!("{name}[{i}]: expected {x:08x}, computed {y:08x}"))
    }

    /// Reports the first differing value between two table sets (or `None`
    /// when bit-identical), with the exact section/index/expected/computed.
    fn first_diff(a: &PsyTablesBits, b: &PsyTablesBits) -> Option<String> {
        if a.max_band != b.max_band {
            return Some(format!(
                "max_band: expected {}, computed {}",
                a.max_band, b.max_band
            ));
        }
        let checks: [(&str, &[u32], &[u32]); 7] = [
            ("fft_ltq", &a.fft_ltq, &b.fft_ltq),
            ("part_ltq", &a.part_ltq, &b.part_ltq),
            ("inv_ltq", &a.inv_ltq, &b.inv_ltq),
            ("min_val", &a.min_val, &b.min_val),
            ("loudness", &a.loudness, &b.loudness),
            ("sprd", &a.sprd, &b.sprd),
            ("scalars", &a.scalars, &b.scalars),
        ];
        checks
            .into_iter()
            .find_map(|(name, x, y)| section_first_diff(name, x, y))
    }

    /// The permanent J.2 invariant: for every one of the44 integer
    /// configurations the deterministic computed path reproduces the frozen
    /// C-oracle tables bit-for-bit (the frozen tables themselves stay the
    /// production source of truth for those pairs).
    #[test]
    fn computed_tables_match_the44_frozen_integer_oracles() {
        for &qual in &[0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0] {
            for rate in RATES {
                let computed = psy_tables(qual, rate).expect("SV8 rate must have computed tables");
                let frozen = frozen_psy_tables(qual, rate)
                    .unwrap_or_else(|| panic!("q{qual} @{rate} must keep its frozen tables"));
                if let Some(d) = first_diff(&computed, frozen) {
                    panic!("computed vs frozen mismatch q{qual} @{rate}: {d}");
                }
            }
        }
    }

    /// Fractional table parity: the computed tables match the committed
    /// C-oracle dumps bit-for-bit for every pair of the J.2 fractional
    /// corpus (interiors at four rates, integer-boundary values, arbitrary
    /// precision, and the q9.9999 near-clip row).
    #[test]
    fn computed_tables_match_the_fractional_c_oracle_dumps() {
        for &(qual_str, rate) in FRAC_PAIRS {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/psy")
                .join(format!("psy_q{qual_str}-{rate}.txt"));
            let text =
                std::fs::read_to_string(&dir).unwrap_or_else(|e| panic!("missing {dir:?}: {e}"));
            let expected = load_dump(&text);
            let qual: f32 = qual_str.parse().expect("quality parse");
            let computed = psy_tables(qual, rate).expect("SV8 rate");
            if let Some(d) = first_diff(&expected, &computed) {
                panic!("fractional mismatch q{qual_str} @{rate}: {d}");
            }
        }
    }

    /// No `.01` quantization or lookup grid: qualities that differ below
    /// the f32 parse-merge boundary resolve exactly like the C oracle
    /// (`4.9999999 -> 5.0f`, `6.0000001 -> 6.0f`) while a genuinely
    /// distinct bit pattern (`6.0000005`) computes genuinely distinct
    /// tables — all three pinned against the committed dumps above.
    #[test]
    fn f32_parse_merges_match_the_c_quality_semantics() {
        assert_eq!(4.9999999f32.to_bits(), 5.0f32.to_bits());
        assert_eq!(6.0000001f32.to_bits(), 6.0f32.to_bits());
        assert_ne!(6.0000005f32.to_bits(), 6.0f32.to_bits());
        let merged = psy_tables(4.9999999, 44100.0).unwrap();
        let five = psy_tables(5.0, 44100.0).unwrap();
        if let Some(d) = first_diff(&merged, &five) {
            panic!("4.9999999 must be exactly5.0 after f32 parse: {d}");
        }
    }
}
