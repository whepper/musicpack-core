//! Forward real FFT and the windowed spectrum kernels (`fft4g.c`,
//! `fft_routines.c`).
//!
//! Only the forward `rdft` exists in the reference (the inverse was removed),
//! and it is driven by the frozen twiddle table [`super::tables::W`]. The
//! bit-reversal permutation `ip` is integer state recomputed per call, so it
//! lives in the per-instance model rather than in a global.

#![allow(dead_code)]

use super::math::my_atan2;
use super::tables::{HANN_256, HANN_1024, HANN_1600, W};

/// The reference `bitrv2`; `ip` is the `ip+2` sub-array of the table.
fn bitrv2(n: usize, ip: &mut [i32], a: &mut [f32]) {
    ip[0] = 0;
    let mut l = n;
    let mut m = 1usize;
    while (m << 3) < l {
        l >>= 1;
        for j in 0..m {
            ip[m + j] = ip[j] + l as i32;
        }
        m <<= 1;
    }
    let m2 = 2 * m;
    let swap = |a: &mut [f32], j1: usize, k1: usize| {
        let xr = a[j1];
        let xi = a[j1 + 1];
        let yr = a[k1];
        let yi = a[k1 + 1];
        a[j1] = yr;
        a[j1 + 1] = yi;
        a[k1] = xr;
        a[k1 + 1] = xi;
    };
    if (m << 3) == l {
        for k in 0..m {
            for j in 0..k {
                let mut j1 = 2 * j + ip[k] as usize;
                let mut k1 = 2 * k + ip[j] as usize;
                swap(a, j1, k1);
                j1 += m2;
                k1 += 2 * m2;
                swap(a, j1, k1);
                j1 += m2;
                k1 -= m2;
                swap(a, j1, k1);
                j1 += m2;
                k1 += 2 * m2;
                swap(a, j1, k1);
            }
            let j1 = 2 * k + m2 + ip[k] as usize;
            let k1 = j1 + m2;
            swap(a, j1, k1);
        }
    } else {
        for k in 1..m {
            for j in 0..k {
                let mut j1 = 2 * j + ip[k] as usize;
                let mut k1 = 2 * k + ip[j] as usize;
                swap(a, j1, k1);
                j1 += m2;
                k1 += m2;
                swap(a, j1, k1);
            }
        }
    }
}

fn cft1st(n: usize, a: &mut [f32], w: &[f32]) {
    let (mut x0r, mut x0i, mut x1r, mut x1i, mut x2r, mut x2i, mut x3r, mut x3i);
    x0r = a[0] + a[2];
    x0i = a[1] + a[3];
    x1r = a[0] - a[2];
    x1i = a[1] - a[3];
    x2r = a[4] + a[6];
    x2i = a[5] + a[7];
    x3r = a[4] - a[6];
    x3i = a[5] - a[7];
    a[0] = x0r + x2r;
    a[1] = x0i + x2i;
    a[4] = x0r - x2r;
    a[5] = x0i - x2i;
    a[2] = x1r - x3i;
    a[3] = x1i + x3r;
    a[6] = x1r + x3i;
    a[7] = x1i - x3r;
    let wk1r = w[2];
    x0r = a[8] + a[10];
    x0i = a[9] + a[11];
    x1r = a[8] - a[10];
    x1i = a[9] - a[11];
    x2r = a[12] + a[14];
    x2i = a[13] + a[15];
    x3r = a[12] - a[14];
    x3i = a[13] - a[15];
    a[8] = x0r + x2r;
    a[9] = x0i + x2i;
    a[12] = x2i - x0i;
    a[13] = x0r - x2r;
    x0r = x1r - x3i;
    x0i = x1i + x3r;
    a[10] = wk1r * (x0r - x0i);
    a[11] = wk1r * (x0r + x0i);
    x0r = x3i + x1r;
    x0i = x3r - x1i;
    a[14] = wk1r * (x0i - x0r);
    a[15] = wk1r * (x0i + x0r);

    let mut k1 = 0usize;
    let mut j = 16usize;
    loop {
        k1 += 2;
        let wk2r = w[k1];
        let wk2i = w[k1 + 1];
        let wk1r = w[2 * k1];
        let wk1i = w[2 * k1 + 1];
        let wk3r = wk1r - 2.0 * wk2i * wk1i;
        let wk3i = 2.0 * wk2i * wk1r - wk1i;
        let mut x0r = a[j] + a[j + 2];
        let mut x0i = a[j + 1] + a[j + 3];
        let x1r = a[j] - a[j + 2];
        let x1i = a[j + 1] - a[j + 3];
        let x2r = a[j + 4] + a[j + 6];
        let x2i = a[j + 5] + a[j + 7];
        let x3r = a[j + 4] - a[j + 6];
        let x3i = a[j + 5] - a[j + 7];
        a[j] = x0r + x2r;
        a[j + 1] = x0i + x2i;
        x0r -= x2r;
        x0i -= x2i;
        a[j + 4] = wk2r * x0r - wk2i * x0i;
        a[j + 5] = wk2r * x0i + wk2i * x0r;
        let x0r = x1r - x3i;
        let x0i = x1i + x3r;
        a[j + 2] = wk1r * x0r - wk1i * x0i;
        a[j + 3] = wk1r * x0i + wk1i * x0r;
        let mut x0r = x1r + x3i;
        let mut x0i = x1i - x3r;
        a[j + 6] = wk3r * x0r - wk3i * x0i;
        a[j + 7] = wk3r * x0i + wk3i * x0r;

        let wk1r = w[2 * k1 + 2];
        let wk1i = w[2 * k1 + 3];
        let wk3r = wk1r - 2.0 * wk2r * wk1i;
        let wk3i = 2.0 * wk2r * wk1r - wk1i;
        x0r = a[j + 8] + a[j + 10];
        x0i = a[j + 9] + a[j + 11];
        let x1r = a[j + 8] - a[j + 10];
        let x1i = a[j + 9] - a[j + 11];
        let x2r = a[j + 12] + a[j + 14];
        let x2i = a[j + 13] + a[j + 15];
        let x3r = a[j + 12] - a[j + 14];
        let x3i = a[j + 13] - a[j + 15];
        a[j + 8] = x0r + x2r;
        a[j + 9] = x0i + x2i;
        x0r -= x2r;
        x0i -= x2i;
        a[j + 12] = -wk2i * x0r - wk2r * x0i;
        a[j + 13] = -wk2i * x0i + wk2r * x0r;
        let x0r = x1r - x3i;
        let x0i = x1i + x3r;
        a[j + 10] = wk1r * x0r - wk1i * x0i;
        a[j + 11] = wk1r * x0i + wk1i * x0r;
        let x0r = x1r + x3i;
        let x0i = x1i - x3r;
        a[j + 14] = wk3r * x0r - wk3i * x0i;
        a[j + 15] = wk3r * x0i + wk3i * x0r;
        j += 16;
        if j >= n {
            break;
        }
    }
}

fn cftmdl(n: usize, l: usize, a: &mut [f32], w: &[f32]) {
    let m = l << 2;
    for j in (0..l).step_by(2) {
        let j1 = j + l;
        let j2 = j1 + l;
        let j3 = j2 + l;
        let x0r = a[j] + a[j1];
        let x0i = a[j + 1] + a[j1 + 1];
        let x1r = a[j] - a[j1];
        let x1i = a[j + 1] - a[j1 + 1];
        let x2r = a[j2] + a[j3];
        let x2i = a[j2 + 1] + a[j3 + 1];
        let x3r = a[j2] - a[j3];
        let x3i = a[j2 + 1] - a[j3 + 1];
        a[j] = x0r + x2r;
        a[j + 1] = x0i + x2i;
        a[j2] = x0r - x2r;
        a[j2 + 1] = x0i - x2i;
        a[j1] = x1r - x3i;
        a[j1 + 1] = x1i + x3r;
        a[j3] = x1r + x3i;
        a[j3 + 1] = x1i - x3r;
    }
    let wk1r = w[2];
    let mut j = m;
    while j < l + m {
        let j1 = j + l;
        let j2 = j1 + l;
        let j3 = j2 + l;
        let x0r = a[j] + a[j1];
        let x0i = a[j + 1] + a[j1 + 1];
        let x1r = a[j] - a[j1];
        let x1i = a[j + 1] - a[j1 + 1];
        let x2r = a[j2] + a[j3];
        let x2i = a[j2 + 1] + a[j3 + 1];
        let x3r = a[j2] - a[j3];
        let x3i = a[j2 + 1] - a[j3 + 1];
        a[j] = x0r + x2r;
        a[j + 1] = x0i + x2i;
        a[j2] = x2i - x0i;
        a[j2 + 1] = x0r - x2r;
        let mut x0r = x1r - x3i;
        let mut x0i = x1i + x3r;
        a[j1] = wk1r * (x0r - x0i);
        a[j1 + 1] = wk1r * (x0r + x0i);
        x0r = x3i + x1r;
        x0i = x3r - x1i;
        a[j3] = wk1r * (x0i - x0r);
        a[j3 + 1] = wk1r * (x0i + x0r);
        j += 2;
    }
    let mut k1 = 0usize;
    let m2 = 2 * m;
    let mut k = m2;
    while k < n {
        k1 += 2;
        let wk2r = w[k1];
        let wk2i = w[k1 + 1];
        let mut wk1r = w[2 * k1];
        let wk1i = w[2 * k1 + 1];
        let wk3r = wk1r - 2.0 * wk2i * wk1i;
        let wk3i = 2.0 * wk2i * wk1r - wk1i;
        let mut j = k;
        loop {
            let j1 = j + l;
            let j2 = j1 + l;
            let j3 = j2 + l;
            let x0r = a[j] + a[j1];
            let x0i = a[j + 1] + a[j1 + 1];
            let x1r = a[j] - a[j1];
            let x1i = a[j + 1] - a[j1 + 1];
            let x2r = a[j2] + a[j3];
            let x2i = a[j2 + 1] + a[j3 + 1];
            let x3r = a[j2] - a[j3];
            let x3i = a[j2 + 1] - a[j3 + 1];
            a[j] = x0r + x2r;
            a[j + 1] = x0i + x2i;
            let mut x0r = x0r - x2r;
            let mut x0i = x0i - x2i;
            a[j2] = wk2r * x0r - wk2i * x0i;
            a[j2 + 1] = wk2r * x0i + wk2i * x0r;
            x0r = x1r - x3i;
            x0i = x1i + x3r;
            a[j1] = wk1r * x0r - wk1i * x0i;
            a[j1 + 1] = wk1r * x0i + wk1i * x0r;
            x0r = x1r + x3i;
            x0i = x1i - x3r;
            a[j3] = wk3r * x0r - wk3i * x0i;
            a[j3 + 1] = wk3r * x0i + wk3i * x0r;
            j += 2;
            if j >= l + k {
                break;
            }
        }
        wk1r = w[2 * k1 + 2];
        let wk1i = w[2 * k1 + 3];
        let wk3r = wk1r - 2.0 * wk2r * wk1i;
        let wk3i = 2.0 * wk2r * wk1r - wk1i;
        let mut j = k + m;
        loop {
            let j1 = j + l;
            let j2 = j1 + l;
            let j3 = j2 + l;
            let x0r = a[j] + a[j1];
            let x0i = a[j + 1] + a[j1 + 1];
            let x1r = a[j] - a[j1];
            let x1i = a[j + 1] - a[j1 + 1];
            let x2r = a[j2] + a[j3];
            let x2i = a[j2 + 1] + a[j3 + 1];
            let x3r = a[j2] - a[j3];
            let x3i = a[j2 + 1] - a[j3 + 1];
            a[j] = x0r + x2r;
            a[j + 1] = x0i + x2i;
            let mut x0r = x0r - x2r;
            let mut x0i = x0i - x2i;
            a[j2] = -wk2i * x0r - wk2r * x0i;
            a[j2 + 1] = -wk2i * x0i + wk2r * x0r;
            x0r = x1r - x3i;
            x0i = x1i + x3r;
            a[j1] = wk1r * x0r - wk1i * x0i;
            a[j1 + 1] = wk1r * x0i + wk1i * x0r;
            x0r = x1r + x3i;
            x0i = x1i - x3r;
            a[j3] = wk3r * x0r - wk3i * x0i;
            a[j3 + 1] = wk3r * x0i + wk3i * x0r;
            j += 2;
            if j >= l + k + m {
                break;
            }
        }
        k += m2;
    }
}

fn cftfsub(n: usize, a: &mut [f32], w: &[f32]) {
    let mut l = 2usize;
    if n > 8 {
        cft1st(n, a, w);
        l = 8;
        while (l << 2) < n {
            cftmdl(n, l, a, w);
            l <<= 2;
        }
    }
    if (l << 2) == n {
        let mut j = 0usize;
        loop {
            let j1 = j + l;
            let j2 = j1 + l;
            let j3 = j2 + l;
            let x0r = a[j] + a[j1];
            let x0i = a[j + 1] + a[j1 + 1];
            let x1r = a[j] - a[j1];
            let x1i = a[j + 1] - a[j1 + 1];
            let x2r = a[j2] + a[j3];
            let x2i = a[j2 + 1] + a[j3 + 1];
            let x3r = a[j2] - a[j3];
            let x3i = a[j2 + 1] - a[j3 + 1];
            a[j] = x0r + x2r;
            a[j + 1] = x0i + x2i;
            a[j2] = x0r - x2r;
            a[j2 + 1] = x0i - x2i;
            a[j1] = x1r - x3i;
            a[j1 + 1] = x1i + x3r;
            a[j3] = x1r + x3i;
            a[j3 + 1] = x1i - x3r;
            j += 2;
            if j >= l {
                break;
            }
        }
    } else {
        let mut j = 0usize;
        loop {
            let j1 = j + l;
            let x0r = a[j] - a[j1];
            let x0i = a[j + 1] - a[j1 + 1];
            a[j] += a[j1];
            a[j + 1] += a[j1 + 1];
            a[j1] = x0r;
            a[j1 + 1] = x0i;
            j += 2;
            if j >= l {
                break;
            }
        }
    }
}

fn rftfsub(n: usize, a: &mut [f32], nc0: usize, c: &[f32]) {
    let m = n >> 1;
    let ks = 2 * nc0 / m;
    let mut kk = ks;
    let mut j = 2usize;
    let mut k = n;
    let mut nc = nc0;
    loop {
        k -= 2;
        nc -= ks;
        let wkr = 0.5f32 - c[nc];
        let wki = c[kk];
        let xr = a[j] - a[k];
        let xi = a[j + 1] + a[k + 1];
        let yr = wkr * xr - wki * xi;
        let yi = wkr * xi + wki * xr;
        a[j] -= yr;
        a[j + 1] -= yi;
        a[k] += yr;
        a[k + 1] -= yi;
        kk += ks;
        j += 2;
        if j >= m {
            break;
        }
    }
}

/// The reference forward `rdft` (`n` in `{4, 256, 1024, 2048}`).
///
/// `ip` is the per-instance bit-reversal table; `ip[0] = 512` and `ip[1] = 512`
/// are the frozen `nw`/`nc` values from `Generate_FFT_Tables(2048)`.
pub fn rdft(n: usize, a: &mut [f32], ip: &mut [i32; 4096]) {
    if n > 4 {
        bitrv2(n, &mut ip[2..], a);
        cftfsub(n, a, &W);
        rftfsub(n, a, ip[1] as usize, &W[ip[0] as usize..]);
    } else if n == 4 {
        cftfsub(n, a, &W);
    }
    let xi = a[0] - a[1];
    a[0] += a[1];
    a[1] = xi;
}

/// Creates the per-instance FFT index table: `nw = nc = 512` for the maximum
/// 2048-point transform.
#[must_use]
pub fn new_fft_index() -> [i32; 4096] {
    let mut ip = [0i32; 4096];
    ip[0] = 512;
    ip[1] = 512;
    ip
}

/// `PowSpec256`: window with `Hann_256`, FFT, 128 power lines.
pub fn pow_spec_256(x: &[f32], erg: &mut [f32], a: &mut [f32], ip: &mut [i32; 4096]) {
    for i in 0..256 {
        a[i] = x[i] * HANN_256[i];
    }
    rdft(256, &mut a[..256], ip);
    for i in 0..128 {
        erg[i] = a[i * 2] * a[i * 2] + a[i * 2 + 1] * a[i * 2 + 1];
    }
}

/// `PowSpec1024`: window with `Hann_1024`, FFT, 512 power lines.
pub fn pow_spec_1024(x: &[f32], erg: &mut [f32], a: &mut [f32], ip: &mut [i32; 4096]) {
    for i in 0..1024 {
        a[i] = x[i] * HANN_1024[i];
    }
    rdft(1024, &mut a[..1024], ip);
    for i in 0..512 {
        erg[i] = a[i * 2] * a[i * 2] + a[i * 2 + 1] * a[i * 2 + 1];
    }
}

/// `PowSpec2048`: window with `Hann_1600` centred in 2048, FFT, 1024 lines.
pub fn pow_spec_2048(x: &[f32], erg: &mut [f32], a: &mut [f32], ip: &mut [i32; 4096]) {
    for slot in a[..224].iter_mut() {
        *slot = 0.0;
    }
    for i in 0..1600 {
        a[i + 224] = x[i] * HANN_1600[i];
    }
    for slot in a[1824..2048].iter_mut() {
        *slot = 0.0;
    }
    rdft(2048, &mut a[..2048], ip);
    for i in 0..1024 {
        erg[i] = a[i * 2] * a[i * 2] + a[i * 2 + 1] * a[i * 2 + 1];
    }
}

/// `PolarSpec1024`: window with `Hann_1024`, FFT, 512 power/phase lines.
pub fn polar_spec_1024(
    x: &[f32],
    erg: &mut [f32],
    phs: &mut [f32],
    a: &mut [f32],
    ip: &mut [i32; 4096],
) {
    for i in 0..1024 {
        a[i] = x[i] * HANN_1024[i];
    }
    rdft(1024, &mut a[..1024], ip);
    for i in 0..512 {
        erg[i] = a[i * 2] * a[i * 2] + a[i * 2 + 1] * a[i * 2 + 1];
        phs[i] = my_atan2(a[i * 2 + 1], a[i * 2]);
    }
}

/// `Cepstrum2048`: mirrors the spectrum, runs a 2048-point FFT in place and
/// keeps the real part scaled by `0.9888 / 2048`.
pub fn cepstrum2048(cep: &mut [f32], max_line: usize, ip: &mut [i32; 4096]) {
    let mut j = 1024usize;
    for i in 0..1024 {
        cep[1024 + j] = cep[i];
        j -= 1;
    }
    rdft(2048, &mut cep[..2048], ip);
    for i in 0..=max_line {
        cep[i] = cep[i * 2] * (0.9888f64 / 2048.0) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/psy");

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
            let vals: Vec<u32> = (0..n)
                .map(|j| u32::from_str_radix(lines[i + 1 + j].trim(), 16).unwrap())
                .collect();
            out.insert(name, vals);
            i += 1 + n;
        }
        out
    }

    fn lcg(s: &mut u32) -> u32 {
        *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *s
    }

    fn fval(s: &mut u32) -> f32 {
        (lcg(s) >> 9) as f32 * (1.0 / 4194304.0) - 1.0
    }

    fn gen_signal(n: usize, kind: usize) -> Vec<f32> {
        let mut s = 0x12345678u32;
        (0..n)
            .map(|i| match kind {
                0 => 0.0,
                1 => {
                    if i == n / 3 {
                        1.0
                    } else {
                        0.0
                    }
                }
                2 => 0.5,
                3 => {
                    if i & 1 == 1 {
                        -0.5
                    } else {
                        0.5
                    }
                }
                _ => fval(&mut s),
            })
            .collect()
    }

    fn check_bits(name: &str, actual: &[f32], expected: &[u32]) {
        assert_eq!(actual.len(), expected.len(), "{name} length");
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(a.to_bits(), *e, "{name}[{i}]");
        }
    }

    #[test]
    fn spectrum_primitives_match_the_reference() {
        for kind in 0..5 {
            let mut ip = new_fft_index();
            let mut scratch = [0f32; 2048];

            let x = gen_signal(256, kind);
            let mut erg = [0f32; 128];
            pow_spec_256(&x, &mut erg, &mut scratch, &mut ip);
            let sec = parse_sections(
                &std::fs::read_to_string(format!("{DIR}/powspec256_k{kind}.txt")).unwrap(),
            );
            check_bits("powspec256", &erg, &sec["erg"]);

            let x = gen_signal(1024, kind);
            let mut erg = [0f32; 512];
            pow_spec_1024(&x, &mut erg, &mut scratch, &mut ip);
            let sec = parse_sections(
                &std::fs::read_to_string(format!("{DIR}/powspec1024_k{kind}.txt")).unwrap(),
            );
            check_bits("powspec1024", &erg, &sec["erg"]);

            let x = gen_signal(2048, kind);
            let mut erg = [0f32; 1024];
            pow_spec_2048(&x, &mut erg, &mut scratch, &mut ip);
            let sec = parse_sections(
                &std::fs::read_to_string(format!("{DIR}/powspec2048_k{kind}.txt")).unwrap(),
            );
            check_bits("powspec2048", &erg, &sec["erg"]);

            let x = gen_signal(1024, kind);
            let mut erg = [0f32; 512];
            let mut phs = [0f32; 512];
            polar_spec_1024(&x, &mut erg, &mut phs, &mut scratch, &mut ip);
            let sec = parse_sections(
                &std::fs::read_to_string(format!("{DIR}/polar1024_k{kind}.txt")).unwrap(),
            );
            check_bits("polar_erg", &erg, &sec["erg"]);
            check_bits("polar_phs", &phs, &sec["phs"]);
        }
    }

    #[test]
    fn forward_rdft2048_matches_the_reference() {
        let mut ip = new_fft_index();
        let mut a = [0f32; 2048];
        let mut s = 0xDEAD_BEEFu32;
        for slot in a.iter_mut() {
            *slot = fval(&mut s) * 10.0;
        }
        let sec = parse_sections(&std::fs::read_to_string(format!("{DIR}/rdft2048.txt")).unwrap());
        check_bits("rdft_in", &a, &sec["in"]);
        rdft(2048, &mut a, &mut ip);
        check_bits("rdft_out", &a, &sec["out"]);
    }

    #[test]
    fn cepstrum2048_matches_the_reference() {
        let mut ip = new_fft_index();
        let mut cep = [0f32; 4096];
        let mut s = 0x0BADF00Du32;
        for slot in cep.iter_mut().take(512) {
            *slot = fval(&mut s) * 10.0;
        }
        for slot in cep.iter_mut().take(1025).skip(512) {
            *slot = 0.0;
        }
        let sec =
            parse_sections(&std::fs::read_to_string(format!("{DIR}/cepstrum2048.txt")).unwrap());

        // mirror (same as `cepstrum2048`'s first step)
        let mut j = 1024usize;
        for i in 0..1024 {
            cep[1024 + j] = cep[i];
            j -= 1;
        }
        check_bits("cep_in", &cep[..2048], &sec["cep_in"]);

        rdft(2048, &mut cep[..2048], &mut ip);
        check_bits("cep_raw", &cep[..2048], &sec["cep_raw"]);

        for i in 0..=900 {
            cep[i] = cep[i * 2] * (0.9888f64 / 2048.0) as f32;
        }
        check_bits("cepstrum", &cep[..901], &sec["cep"]);
    }

    // -- 15H.2 candidate micro-benchmark (measurement only) ------------------
    //
    // Splits `polar_spec_1024`'s cost into window+FFT+power (`pow_spec_1024`)
    // and the 512 `my_atan2` phase evaluations, so Candidate B can tell how
    // much of polar_spec is even eligible for arithmetic vectorisation
    // (atan2 stays scalar per the compatibility contract).
    #[test]
    #[ignore = "benchmark; run with --release --ignored --nocapture"]
    fn bench_micro_polar_split() {
        use std::time::{Duration, Instant};

        fn sink(data: &[f32]) -> f32 {
            let mut acc = 0.0f32;
            for (i, v) in data.iter().enumerate() {
                acc += *v * (i as f32 + 1.0);
            }
            acc
        }

        fn measure(iters: usize, mut f: impl FnMut()) -> Vec<Duration> {
            for _ in 0..3 {
                f();
            }
            let mut times = Vec::with_capacity(iters);
            for _ in 0..iters {
                let t0 = Instant::now();
                for _ in 0..4 {
                    f();
                }
                times.push(t0.elapsed());
            }
            times
        }

        let mut s = 0x9ABC_DEF0u32;
        let mut x = vec![0.0f32; 1024];
        for slot in x.iter_mut() {
            *slot = fval(&mut s);
        }
        let mut erg = [0f32; 512];
        let mut phs = [0f32; 512];
        let mut a = vec![0f32; 2048];
        let mut ip = new_fft_index();

        let t_pow = measure(15, || {
            pow_spec_1024(&x, &mut erg, &mut a, &mut ip);
            std::hint::black_box(sink(&erg));
        });
        let t_pol = measure(15, || {
            polar_spec_1024(&x, &mut erg, &mut phs, &mut a, &mut ip);
            std::hint::black_box(sink(&erg) + sink(&phs));
        });
        fn stats(v: &[Duration]) -> (Duration, Duration) {
            let mut s = v.to_vec();
            s.sort_unstable();
            (*s.iter().min().unwrap(), s[s.len() / 2])
        }
        // Each iteration runs 4 calls; report per call.
        let (pmin, pmed) = stats(&t_pow);
        let (qmin, qmed) = stats(&t_pol);
        println!("per polar/pow call (1024-pt, 512 lines, 4 calls/iter):");
        println!(
            "  pow_spec_1024   {:>10.3?}/call (min {:>10.3?})",
            pmed / 4,
            pmin / 4
        );
        println!(
            "  polar_spec_1024 {:>10.3?}/call (min {:>10.3?})",
            qmed / 4,
            qmin / 4
        );
        println!("  (difference ~= 512x my_atan2 + phase stores)");

        // Raw rdft-1024 share (also informs Candidate E).
        let mut b = [0.0f32; 1024];
        for (i, slot) in b.iter_mut().enumerate() {
            *slot = fval(&mut s) * (i % 7) as f32;
        }
        let mut ip2 = new_fft_index();
        let t_rdft = measure(15, || {
            rdft(1024, &mut b, &mut ip2);
            std::hint::black_box(sink(&b[..512]));
        });
        let (rmin, rmed) = stats(&t_rdft);
        println!(
            "  rdft_1024       {:>10.3?}/call (min {:>10.3?})",
            rmed / 4,
            rmin / 4
        );
    }
}
