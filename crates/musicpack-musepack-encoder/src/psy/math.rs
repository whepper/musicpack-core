//! Reference numerical primitives for the psychoacoustic model.
//!
//! The reference build defines `FAST_MATH` and `CVD_FASTLOG`. On the pinned
//! Apple/arm64 toolchain (`mpc/mpcmath.h`) that means:
//!
//! * `POW`, `POW10`, `LOG10`, `SQRT` are the **double** libm functions cast
//!   back to `float`;
//! * `COSF`, `ATAN2F` are the table-driven `my_cos` / `my_atan2` approximations
//!   (frozen tables in [`super::tables`]);
//! * `IFLOORF` is the bit-trick `my_ifloor`;
//! * `CVD_FASTLOG` selects the bit-manipulation `logfast` in `cvd.c`.
//!
//! The crate reproduces each one exactly. Crucially, `pow(10.0, x)` must call
//! the *generic* `pow`, not the `exp10` special case LLVM can emit for a
//! literal base ten, so [`pow10`] passes the base through `black_box`.

#![allow(dead_code)]

use std::hint::black_box;

use super::tables::{TABATAN2, TABCOS};

/// `TABSTEP` from `mpc/mpcmath.h`.
const TABSTEP: i32 = 64;

/// The reference `mpc_lrintf`: add the *integer* `0x00FF8000` (= 16744448) as
/// a float, reinterpret the sum's bits as `i32` and subtract `0x4B7F8000`.
#[inline]
pub fn lrintf(x: f32) -> i32 {
    let bits = (x + 16_744_448.0f32).to_bits() as i32;
    bits.wrapping_sub(0x4B7F8000u32 as i32)
}

/// The reference `my_ifloor` (`IFLOORF` under `FAST_MATH`).
#[inline]
pub fn my_ifloor(x: f32) -> i32 {
    let shifted = ((x as f64) + (0x00C0_0000u32 as f64 + 0.500_000_001)) as f32;
    (shifted.to_bits() as i32).wrapping_sub(1_262_485_505)
}

/// The reference `my_cos` (`COSF` under `FAST_MATH`), a table interpolation.
#[inline]
pub fn my_cos(x: f32) -> f32 {
    let t = TABSTEP as f32 * x;
    let i = lrintf(t);
    let idx = ((13 * TABSTEP + i) as usize) * 2;
    TABCOS[idx] + TABCOS[idx + 1] * (t - i as f32)
}

/// The reference `my_atan2` (`ATAN2F` under `FAST_MATH`).
#[inline]
pub fn my_atan2(x: f32, y: f32) -> f32 {
    let mx = x.to_bits();
    let my = y.to_bits();
    if (mx & 0x7FFF_FFFF) < (my & 0x7FFF_FFFF) {
        let t = TABSTEP as f32 * (x / y);
        let i = lrintf(t);
        let idx = ((TABSTEP + i) as usize) * 2;
        let mut ret = TABATAN2[idx] + TABATAN2[idx + 1] * (t - i as f32);
        if (my as i32) < 0 {
            ret = (f64::from(ret) - std::f64::consts::PI) as f32;
        }
        ret
    } else if (mx as i32) < 0 {
        let t = TABSTEP as f32 * (y / x);
        let i = lrintf(t);
        let idx = ((TABSTEP + i) as usize) * 2;
        let prod = TABATAN2[idx + 1] * (i as f32 - t);
        (-(std::f64::consts::PI / 2.0) - f64::from(TABATAN2[idx]) + f64::from(prod)) as f32
    } else if (mx as i32) > 0 {
        let t = TABSTEP as f32 * (y / x);
        let i = lrintf(t);
        let idx = ((TABSTEP + i) as usize) * 2;
        let prod = TABATAN2[idx + 1] * (i as f32 - t);
        (std::f64::consts::PI / 2.0 - f64::from(TABATAN2[idx]) + f64::from(prod)) as f32
    } else {
        0.0
    }
}

/// The reference `logfast` (`CVD_FASTLOG`): a bit-manipulation log of `x^8`.
#[inline]
pub fn logfast(x: f32) -> f32 {
    // `double d = x * x;` — the first product is an `f32` multiply.
    let mut d = f64::from(x * x);
    d *= d;
    d *= d;
    let hi = (d.to_bits() >> 32) as u32;
    let v = (f64::from(hi) + (45127.5 - 1_072_693_248.0)) * (std::f64::consts::LN_2 / 8_388_608.0);
    v as f32
}

/// `POW(x, y)` = `(float) pow((double) x, (double) y)`.
#[inline]
pub fn pow(x: f32, y: f32) -> f32 {
    (f64::from(x).powf(f64::from(y))) as f32
}

/// `POW10(x)` = `(float) pow(10.0, (double) x)`.
///
/// `black_box` stops LLVM replacing the call with `exp10`, which is *not*
/// bit-identical to the reference `pow(10., x)`.
#[inline]
pub fn pow10(x: f32) -> f32 {
    (black_box(10.0f64).powf(f64::from(x))) as f32
}

/// `POW10` when the argument is already an `f64` expression, e.g.
/// `POW10(0.1 * m->minSMR)` where `0.1` is a double literal.
#[inline]
pub fn pow10_d(x: f64) -> f32 {
    (black_box(10.0f64).powf(x)) as f32
}

/// `LOG10(x)` = `(float) log10((double) x)`.
#[inline]
pub fn log10(x: f32) -> f32 {
    f64::from(x).log10() as f32
}

/// `SQRTF(x)` = `(float) sqrt((double) x)`.
#[inline]
pub fn sqrtf(x: f32) -> f32 {
    f64::from(x).sqrt() as f32
}

/// `FABS(x)` = `(float) fabs((double) x)`.
#[inline]
pub fn fabs(x: f32) -> f32 {
    f64::from(x).abs() as f32
}

/// `minf(A, B)`.
#[inline]
pub fn minf(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// `maxf(A, B)`.
#[inline]
pub fn maxf(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// `mini(A, B)`.
#[inline]
pub fn mini(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

/// `maxi(A, B)`.
#[inline]
pub fn maxi(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

/// `mind(A, B)`.
#[inline]
pub fn mind(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const MATH_TXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/psy/math.txt");

    /// Parses the sectioned `name count` / hex-line oracle format.
    fn parse_sections(text: &str) -> HashMap<String, Vec<u32>> {
        let mut out = HashMap::new();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut i = 0;
        while i < lines.len() {
            let parts: Vec<&str> = lines[i].split_whitespace().collect();
            assert_eq!(
                parts.len(),
                2,
                "expected a section header, got {:?}",
                lines[i]
            );
            let name = parts[0].to_string();
            let n: usize = parts[1].parse().unwrap();
            let mut vals = Vec::with_capacity(n);
            for j in 0..n {
                vals.push(u32::from_str_radix(lines[i + 1 + j].trim(), 16).unwrap());
            }
            out.insert(name, vals);
            i += 1 + n;
        }
        out
    }

    fn f32s(v: &[u32]) -> Vec<f32> {
        v.iter().map(|b| f32::from_bits(*b)).collect()
    }

    #[test]
    fn fastmath_primitives_match_the_reference() {
        let text = std::fs::read_to_string(MATH_TXT).expect("math oracle");
        let sec = parse_sections(&text);
        let input = f32s(&sec["in"]);
        let n = input.len();
        let y: Vec<f32> = (0..n).map(|i| input[(i * 7 + 13) % n]).collect();

        let check = |name: &str, f: &dyn Fn(usize) -> u32| {
            let expected = &sec[name];
            assert_eq!(expected.len(), n, "{name} length");
            for i in 0..n {
                assert_eq!(
                    f(i),
                    expected[i],
                    "{name}[{i}] input={:#010x}",
                    input[i].to_bits()
                );
            }
        };

        check("my_cos", &|i| my_cos(input[i]).to_bits());
        check("my_atan2", &|i| my_atan2(input[i], y[i]).to_bits());
        check("my_ifloor", &|i| my_ifloor(input[i]) as u32);
        check("lrintf", &|i| lrintf(input[i]) as u32);
        check("pow10", &|i| pow10(input[i]).to_bits());
        check("pow", &|i| pow(input[i].abs() + 0.0001, y[i]).to_bits());
        check("log10", &|i| log10(input[i].abs() + 0.0001).to_bits());
        check("sqrtf", &|i| sqrtf(input[i].abs()).to_bits());
    }
}
