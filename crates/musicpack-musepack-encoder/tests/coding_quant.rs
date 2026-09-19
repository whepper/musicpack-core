//! Bit-exact quantisation tests against the frozen coding oracle.
//!
//! Reconstructs the quantisation inputs directly from the fixture (`Res`,
//! post-allocation `X`, `NS_Order`, `FIR`) and compares `Q` exactly. Never
//! invokes the C encoder.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::coding::Quantizer;
use musicpack_musepack_encoder::filterbank::{SUBBANDS, Subband};

const FRAME_BYTES: usize = 36_864;
const OFF_NS_ORDER: usize = 19_968;
const OFF_FIR: usize = 20_224;
const OFF_RES: usize = 22_016;
const OFF_X_ALLOC: usize = 23_040;
const OFF_Q: usize = 32_256;

const SAMPLES: usize = 36;
const MAX_NS_ORDER: usize = 6;

struct Case {
    quality: u8,
    max_band: usize,
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/coding")
}

fn f32_at(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn i32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn i16_at(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}

fn read_x(b: &[u8]) -> [Subband; SUBBANDS] {
    let mut x = [Subband::ZERO; SUBBANDS];
    let mut off = OFF_X_ALLOC;
    for band in x.iter_mut() {
        for n in 0..SAMPLES {
            band.left[n] = f32_at(b, off);
            off += 4;
            band.right[n] = f32_at(b, off);
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

    let mut quantizer = Quantizer::new();
    let mut compared = 0usize;
    let mut pns_bands = 0usize;

    for frame in 0..frames {
        let b = &bytes[frame * FRAME_BYTES..(frame + 1) * FRAME_BYTES];

        let mut res_l = [0i32; SUBBANDS];
        let mut res_r = [0i32; SUBBANDS];
        let mut off = OFF_RES;
        for band in 0..SUBBANDS {
            res_l[band] = i32_at(b, off);
            off += 4;
            res_r[band] = i32_at(b, off);
            off += 4;
        }

        let mut ns_order_l = [0u32; SUBBANDS];
        let mut ns_order_r = [0u32; SUBBANDS];
        let mut off = OFF_NS_ORDER;
        for band in 0..SUBBANDS {
            ns_order_l[band] = i32_at(b, off) as u32;
            off += 4;
            ns_order_r[band] = i32_at(b, off) as u32;
            off += 4;
        }

        let mut fir_l = [[0.0f32; MAX_NS_ORDER]; SUBBANDS];
        let mut fir_r = [[0.0f32; MAX_NS_ORDER]; SUBBANDS];
        let mut off = OFF_FIR;
        for band in 0..SUBBANDS {
            for k in 0..MAX_NS_ORDER {
                fir_l[band][k] = f32_at(b, off);
                off += 4;
                fir_r[band][k] = f32_at(b, off);
                off += 4;
            }
        }

        let x = read_x(b);

        quantizer
            .quantize(
                case.max_band,
                &res_l,
                &res_r,
                &ns_order_l,
                &ns_order_r,
                &fir_l,
                &fir_r,
                &x,
            )
            .unwrap();

        let mut off = OFF_Q;
        for band in 0..SUBBANDS {
            for n in 0..SAMPLES {
                let el = i16_at(b, off);
                off += 2;
                let er = i16_at(b, off);
                off += 2;
                assert_eq!(
                    quantizer.q_l()[band][n],
                    el,
                    "q{} frame {frame} Q_L[{band}][{n}] (res {})",
                    case.quality,
                    res_l[band]
                );
                assert_eq!(
                    quantizer.q_r()[band][n],
                    er,
                    "q{} frame {frame} Q_R[{band}][{n}] (res {})",
                    case.quality,
                    res_r[band]
                );
                compared += 2;
            }
        }

        for band in 0..SUBBANDS {
            if res_l[band] == -1 {
                pns_bands += 1;
            }
            if res_r[band] == -1 {
                pns_bands += 1;
            }
        }
    }

    eprintln!(
        "QUANT q{}: {frames} frames, Q {compared} values matched, Res==-1 bands {pns_bands}",
        case.quality
    );
}

#[test]
fn quantize_matches_the_reference_bit_for_bit() {
    // q4 exercises PNS (Res == -1), which skips quantisation.
    check_case(&Case {
        quality: 4,
        max_band: 22,
    });
    check_case(&Case {
        quality: 5,
        max_band: 28,
    });
    check_case(&Case {
        quality: 6,
        max_band: 31,
    });
    check_case(&Case {
        quality: 7,
        max_band: 31,
    });
}
