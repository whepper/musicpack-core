//! Bit-exact SCF-extraction tests against the frozen coding oracle.
//!
//! Consumes `tests/data/coding/coding-q{5,6,7}.bin` (see that directory's
//! README) and never invokes the C encoder.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::coding::{ScfState, scf_extraktion};
use musicpack_musepack_encoder::filterbank::{SUBBANDS, Subband};

const FRAME_BYTES: usize = 36_864;
const OFF_MS_FLAG: usize = 8_832;
const OFF_X_MS: usize = 8_960;
const OFF_POWER: usize = 18_176;
const OFF_SCF: usize = 18_944;
const OFF_SNR: usize = 19_712;

struct Case {
    quality: u8,
    max_band: usize,
    comb: i32,
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/coding")
}

fn read_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn read_i32(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn x_from_fixture(b: &[u8]) -> [Subband; SUBBANDS] {
    let mut x = [Subband::ZERO; SUBBANDS];
    let mut off = OFF_X_MS;
    for subband in x.iter_mut() {
        for n in 0..36 {
            subband.left[n] = read_f32(b, off);
            off += 4;
            subband.right[n] = read_f32(b, off);
            off += 4;
        }
    }
    x
}

fn check_case(case: &Case) {
    let path = data_dir().join(format!("coding-q{}.bin", case.quality));
    let bytes = std::fs::read(&path).expect("coding fixture");
    assert_eq!(bytes.len() % FRAME_BYTES, 0);
    let frames = bytes.len() / FRAME_BYTES;

    let mut state = ScfState::default();
    let mut compared = 0usize;
    for frame in 0..frames {
        let b = &bytes[frame * FRAME_BYTES..(frame + 1) * FRAME_BYTES];
        let _ = OFF_MS_FLAG;
        let mut x = x_from_fixture(b);
        let out = scf_extraktion(&mut state, case.max_band, case.comb, &mut x).unwrap();

        let mut off = OFF_POWER;
        for band in 0..SUBBANDS {
            for sub in 0..3 {
                let el = read_f32(b, off);
                off += 4;
                let er = read_f32(b, off);
                off += 4;
                if band > case.max_band {
                    continue; // the reference leaves bands above max_band unset
                }
                assert_eq!(
                    out.power_l[band][sub].to_bits(),
                    el.to_bits(),
                    "q{} frame {frame} Power_L[{band}][{sub}]",
                    case.quality
                );
                assert_eq!(
                    out.power_r[band][sub].to_bits(),
                    er.to_bits(),
                    "q{} frame {frame} Power_R[{band}][{sub}]",
                    case.quality
                );
                compared += 2;
            }
        }

        let mut off = OFF_SCF;
        for band in 0..SUBBANDS {
            for sub in 0..3 {
                let el = read_i32(b, off);
                off += 4;
                let er = read_i32(b, off);
                off += 4;
                if band > case.max_band {
                    continue;
                }
                assert_eq!(
                    state.scf_l[band][sub], el,
                    "q{} frame {frame} SCF_Index_L[{band}][{sub}]",
                    case.quality
                );
                assert_eq!(
                    state.scf_r[band][sub], er,
                    "q{} frame {frame} SCF_Index_R[{band}][{sub}]",
                    case.quality
                );
                compared += 2;
            }
        }

        let mut off = OFF_SNR;
        for band in 0..SUBBANDS {
            let el = read_f32(b, off);
            off += 4;
            if band > case.max_band {
                continue;
            }
            assert_eq!(
                out.snr_comp_l[band].to_bits(),
                el.to_bits(),
                "q{} frame {frame} SNR_comp_L[{band}]",
                case.quality
            );
            compared += 1;
        }
        for band in 0..SUBBANDS {
            let er = read_f32(b, off);
            off += 4;
            if band > case.max_band {
                continue;
            }
            assert_eq!(
                out.snr_comp_r[band].to_bits(),
                er.to_bits(),
                "q{} frame {frame} SNR_comp_R[{band}]",
                case.quality
            );
            compared += 1;
        }
    }
    assert!(compared > 0);
    eprintln!(
        "SCF q{}: {frames} frames, {compared} values matched",
        case.quality
    );
}

#[test]
fn scf_matches_the_reference_bit_for_bit() {
    check_case(&Case {
        quality: 5,
        max_band: 28,
        comb: 9,
    });
    check_case(&Case {
        quality: 6,
        max_band: 31,
        comb: 7,
    });
    check_case(&Case {
        quality: 7,
        max_band: 31,
        comb: 5,
    });
}
