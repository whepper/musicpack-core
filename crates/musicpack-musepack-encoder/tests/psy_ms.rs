//! Level-2 `MS_LR_Entscheidung` sub-oracle: exact MS flags, SMR rewrite and
//! M/S sample rewrite for deterministic synthetic inputs.

mod psy_common;

use musicpack_musepack_encoder::psy::{PsychoacousticModel, Smr, ms_lr_entscheidung};
use psy_common::{data_dir, fval, parse_hex_sections};

const CONFIGS: [(&str, f32, f32); 7] = [
    ("q4-44100", 4.0, 44100.0),
    ("q5-44100", 5.0, 44100.0),
    ("q6-44100", 6.0, 44100.0),
    ("q7-44100", 7.0, 44100.0),
    ("q5-48000", 5.0, 48000.0),
    ("q5-37800", 5.0, 37800.0),
    ("q5-32000", 5.0, 32000.0),
];

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn build(k: usize) -> (Smr, [[f32; 36]; 32], [[f32; 36]; 32]) {
    let mut s = 0xC0FF_EE11u32.wrapping_add((k as u32) * 7919);
    let mut smr = Smr {
        l: [0.0; 32],
        r: [0.0; 32],
        m: [0.0; 32],
        s: [0.0; 32],
    };
    let mut x_l = [[0.0f32; 36]; 32];
    let mut x_r = [[0.0f32; 36]; 32];
    for band in 0..32 {
        for n in 0..36 {
            x_l[band][n] = fval(&mut s);
            x_r[band][n] = fval(&mut s);
        }
        smr.l[band] = fval(&mut s).abs() * 2000.0;
        smr.r[band] = fval(&mut s).abs() * 2000.0;
        smr.m[band] = fval(&mut s).abs() * 2000.0;
        smr.s[band] = fval(&mut s).abs() * 2000.0;
    }
    (smr, x_l, x_r)
}

#[test]
fn ms_lr_entscheidung_matches_the_reference() {
    let mut compared = 0usize;
    for (name, qual, rate) in CONFIGS {
        let max_band = PsychoacousticModel::new(qual, rate).unwrap().max_band();
        for k in 0..8 {
            let (mut smr, mut x_l, mut x_r) = build(k);
            let mut ms = [0u8; 32];
            ms_lr_entscheidung(max_band, &mut ms, &mut smr, &mut x_l, &mut x_r);

            let text = std::fs::read_to_string(data_dir().join(format!("ms_{name}_k{k}.txt")))
                .expect("ms fixture");
            let sec = parse_hex_sections(&text);

            let ms_actual: Vec<f32> = ms.iter().map(|v| *v as f32).collect();
            let defined = max_band as usize + 1;
            assert_eq!(
                bits(&ms_actual[..defined]),
                sec["ms"][..defined],
                "{name} k{k} ms"
            );
            assert_eq!(
                bits(&smr.l[..defined]),
                sec["smr_L"][..defined],
                "{name} k{k} smr_L"
            );
            assert_eq!(
                bits(&smr.r[..defined]),
                sec["smr_R"][..defined],
                "{name} k{k} smr_R"
            );
            assert_eq!(
                bits(&smr.m[..defined]),
                sec["smr_M"][..defined],
                "{name} k{k} smr_M"
            );
            assert_eq!(
                bits(&smr.s[..defined]),
                sec["smr_S"][..defined],
                "{name} k{k} smr_S"
            );

            let mut xl = Vec::with_capacity(1152);
            let mut xr = Vec::with_capacity(1152);
            for band in 0..32 {
                xl.extend_from_slice(&x_l[band]);
                xr.extend_from_slice(&x_r[band]);
            }
            assert_eq!(bits(&xl), sec["x_L"], "{name} k{k} x_L");
            assert_eq!(bits(&xr), sec["x_R"], "{name} k{k} x_R");
            compared += 32 + 128 + 2304;
        }
    }
    eprintln!("ms oracle: {} values compared", compared);
}
