//! Level-3 psychoacoustic model oracle: exact `SMR`, `Transient`, `TransientenCalc`
//! and `ANSspec` for every frozen `(quality, rate)` and PCM case.

mod psy_common;

use musicpack_musepack_encoder::psy::{
    PcmFrame, PsychoacousticModel, Smr, raise_smr, transienten_calc,
};
use psy_common::{Kind, data_dir, frame_window, generate, read_f32le, read_manifest};

const FRAME_F32: usize = 2374;

fn check(actual: &[f32], expected: &[f32], label: &str, case: &str, frame: usize) -> usize {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{case} f{frame} {label} length"
    );
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            a.to_bits(),
            e.to_bits(),
            "{case} f{frame} {label}[{i}]: got {a:?} ({:#010x}), want {e:?} ({:#010x})",
            a.to_bits(),
            e.to_bits()
        );
    }
    actual.len()
}

fn check_i32(actual: &[i32], expected: &[f32], label: &str, case: &str, frame: usize) -> usize {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{case} f{frame} {label} length"
    );
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            *a as f32, *e,
            "{case} f{frame} {label}[{i}]: got {a}, want {e}"
        );
    }
    actual.len()
}

fn smr_slice(s: &Smr) -> [&[f32]; 4] {
    [&s.l, &s.r, &s.m, &s.s]
}

#[test]
fn model_outputs_match_the_reference() {
    let cases = read_manifest();
    assert!(!cases.is_empty());
    let mut compared = 0usize;
    let mut frames = 0usize;
    let mut per_config: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();

    for case in &cases {
        let kind = Kind::from_name(&case.name[case.config.len() + 1..]);
        let total = case.frames * psy_common::BLOCK + psy_common::BLOCK;
        let (gl, gr) = generate(kind, total);

        let mut model = PsychoacousticModel::new(case.qual, case.rate).expect("frozen config");
        assert_eq!(model.max_band(), case.max_band, "{} max_band", case.name);

        let data = read_f32le(&data_dir().join(format!("{}.f32le", case.name)));
        assert_eq!(data.len(), case.frames * FRAME_F32, "{} size", case.name);

        for f in 0..case.frames {
            let wl = frame_window(&gl, f);
            let wr = frame_window(&gr, f);
            let wm: Vec<f32> = wl.iter().zip(&wr).map(|(l, r)| (l + r) * 0.5).collect();
            let ws: Vec<f32> = wl.iter().zip(&wr).map(|(l, r)| (l - r) * 0.5).collect();
            let pcm = PcmFrame {
                l: &wl,
                r: &wr,
                m: &wm,
                s: &ws,
            };
            let out = model.analyse_frame(&pcm);

            let mut raised = out.smr.clone();
            if case.min_smr > 0.0 {
                raise_smr(case.max_band, case.min_smr, &mut raised);
            }
            let trans = transienten_calc(&out.transient_l, &out.transient_r);

            let rec = &data[f * FRAME_F32..(f + 1) * FRAME_F32];
            let raw = smr_slice(&out.smr);
            let raised_s = smr_slice(&raised);
            let defined = case.max_band as usize + 1;
            for (c, arr) in raw.iter().enumerate() {
                compared += check(
                    &arr[..defined],
                    &rec[c * 32..c * 32 + defined],
                    "SMR",
                    &case.name,
                    f,
                );
            }
            for (c, arr) in raised_s.iter().enumerate() {
                compared += check(
                    &arr[..defined],
                    &rec[128 + c * 32..128 + c * 32 + defined],
                    "SMRr",
                    &case.name,
                    f,
                );
            }
            compared += check_i32(
                &out.transient_l,
                &rec[256..275],
                "TransientL",
                &case.name,
                f,
            );
            compared += check_i32(
                &out.transient_r,
                &rec[275..294],
                "TransientR",
                &case.name,
                f,
            );
            compared += check_i32(&trans, &rec[294..326], "Transient", &case.name, f);
            compared += check(
                model.ans_spec_l(),
                &rec[326..838],
                "ANSspecL",
                &case.name,
                f,
            );
            compared += check(
                model.ans_spec_r(),
                &rec[838..1350],
                "ANSspecR",
                &case.name,
                f,
            );
            compared += check(
                model.ans_spec_m(),
                &rec[1350..1862],
                "ANSspecM",
                &case.name,
                f,
            );
            compared += check(
                model.ans_spec_s(),
                &rec[1862..2374],
                "ANSspecS",
                &case.name,
                f,
            );
            frames += 1;
        }
        *per_config.entry(case.config.clone()).or_default() += case.frames;
    }

    eprintln!(
        "psy model: {} cases, {} frames, {} values compared",
        cases.len(),
        frames,
        compared
    );
    eprintln!("frames per config: {per_config:?}");
}
