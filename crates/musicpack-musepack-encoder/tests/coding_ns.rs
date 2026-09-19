//! Bit-exact noise-shaping tests against the frozen coding oracle.
//!
//! Consumes `tests/data/coding/coding-q{5,6,7}.bin` (see that directory's
//! README) and never invokes the C encoder.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::coding::{Anspec, Smr, ns_analyse};

const FRAME_BYTES: usize = 36_864;
const OFF_SMR: usize = 0;
const OFF_ANSPEC: usize = 512;
const OFF_TRANSIENT: usize = 8_704;
const OFF_MS: usize = 8_832;
const OFF_SCF: usize = 18_944;
const OFF_SNR_SCF: usize = 19_712;
const OFF_NS_ORDER: usize = 19_968;
const OFF_FIR: usize = 20_224;
const OFF_SNR_NS: usize = 21_760;

const SUBBANDS: usize = 32;
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

fn read_32(b: &[u8], mut off: usize) -> [f32; SUBBANDS] {
    let mut v = [0.0f32; SUBBANDS];
    for slot in v.iter_mut() {
        *slot = f32_at(b, off);
        off += 4;
    }
    v
}

fn read_512(b: &[u8], mut off: usize) -> [f32; 512] {
    let mut v = [0.0f32; 512];
    for slot in v.iter_mut() {
        *slot = f32::from_bits(u32::from_le_bytes([
            b[off],
            b[off + 1],
            b[off + 2],
            b[off + 3],
        ]));
        off += 4;
    }
    v
}

fn check_case(case: &Case) {
    let path = data_dir().join(format!("coding-q{}.bin", case.quality));
    let bytes = std::fs::read(&path).expect("coding fixture");
    assert_eq!(bytes.len() % FRAME_BYTES, 0);
    let frames = bytes.len() / FRAME_BYTES;

    let mut counts = (0usize, 0usize, 0usize, 0usize); // order, fir, snr, frames
    for frame in 0..frames {
        let b = &bytes[frame * FRAME_BYTES..(frame + 1) * FRAME_BYTES];

        let smr = Smr {
            l: read_32(b, OFF_SMR),
            r: read_32(b, OFF_SMR + 128),
            m: read_32(b, OFF_SMR + 256),
            s: read_32(b, OFF_SMR + 384),
        };
        let anspec = Anspec {
            l: read_512(b, OFF_ANSPEC),
            r: read_512(b, OFF_ANSPEC + 2048),
            m: read_512(b, OFF_ANSPEC + 4096),
            s: read_512(b, OFF_ANSPEC + 6144),
        };
        let mut ms_flag = [0i32; SUBBANDS];
        let mut transient = [0i32; SUBBANDS];
        for band in 0..SUBBANDS {
            transient[band] = i32_at(b, OFF_TRANSIENT + band * 4);
            ms_flag[band] = i32_at(b, OFF_MS + band * 4);
        }
        let mut scf_l = [[0i32; 3]; SUBBANDS];
        let mut scf_r = [[0i32; 3]; SUBBANDS];
        let mut off = OFF_SCF;
        for band in 0..SUBBANDS {
            for sub in 0..3 {
                scf_l[band][sub] = i32_at(b, off);
                off += 4;
                scf_r[band][sub] = i32_at(b, off);
                off += 4;
            }
        }
        let snr_in_l = read_32(b, OFF_SNR_SCF);
        let snr_in_r = read_32(b, OFF_SNR_SCF + 128);

        let out = ns_analyse(
            case.max_band,
            &ms_flag,
            &smr,
            &anspec,
            &scf_l,
            &scf_r,
            &transient,
            &snr_in_l,
            &snr_in_r,
        )
        .unwrap();

        // NS_Order (all 32 bands; the reference memsets them to 0).
        let mut off = OFF_NS_ORDER;
        for band in 0..SUBBANDS {
            let el = i32_at(b, off);
            off += 4;
            let er = i32_at(b, off);
            off += 4;
            assert_eq!(
                out.order_l[band] as i32, el,
                "q{} frame {frame} NS_Order_L[{band}]",
                case.quality
            );
            assert_eq!(
                out.order_r[band] as i32, er,
                "q{} frame {frame} NS_Order_R[{band}]",
                case.quality
            );
            counts.0 += 2;
        }

        // FIR (all 32 bands × 6 taps; the reference memsets them to 0).
        let mut off = OFF_FIR;
        for band in 0..SUBBANDS {
            for k in 0..MAX_NS_ORDER {
                let el = f32_at(b, off);
                off += 4;
                let er = f32_at(b, off);
                off += 4;
                assert_eq!(
                    out.fir_l[band][k].to_bits(),
                    el.to_bits(),
                    "q{} frame {frame} FIR_L[{band}][{k}]",
                    case.quality
                );
                assert_eq!(
                    out.fir_r[band][k].to_bits(),
                    er.to_bits(),
                    "q{} frame {frame} FIR_R[{band}][{k}]",
                    case.quality
                );
                counts.1 += 2;
            }
        }

        // SNR_comp after NS (meaningful bands only; bands above max_band are
        // uninitialised in the reference).
        let mut off = OFF_SNR_NS;
        for band in 0..SUBBANDS {
            let el = f32_at(b, off);
            off += 4;
            if band <= case.max_band {
                assert_eq!(
                    out.snr_comp_l[band].to_bits(),
                    el.to_bits(),
                    "q{} frame {frame} SNR_comp_L[{band}] after NS",
                    case.quality
                );
                counts.2 += 1;
            }
        }
        for band in 0..SUBBANDS {
            let er = f32_at(b, off);
            off += 4;
            if band <= case.max_band {
                assert_eq!(
                    out.snr_comp_r[band].to_bits(),
                    er.to_bits(),
                    "q{} frame {frame} SNR_comp_R[{band}] after NS",
                    case.quality
                );
                counts.2 += 1;
            }
        }
        counts.3 += 1;
    }

    eprintln!(
        "NS q{}: {} frames, order {} values, fir {} values, snr {} values matched",
        case.quality, counts.3, counts.0, counts.1, counts.2
    );
}

#[test]
fn ns_matches_the_reference_bit_for_bit() {
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
