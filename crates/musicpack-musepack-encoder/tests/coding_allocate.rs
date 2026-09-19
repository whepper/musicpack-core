//! Bit-exact allocation/PNS tests against the frozen coding oracle.
//!
//! The allocation inputs are taken from the fixture; the post-SCF `X` is
//! produced by the crate's already-proven SCF stage (the fixtures are frozen
//! and must not be regenerated). Never invokes the C encoder.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::coding::{ScfState, Smr, allocate, scf_extraktion};
use musicpack_musepack_encoder::filterbank::{SUBBANDS, Subband};

const FRAME_BYTES: usize = 36_864;
const OFF_SMR: usize = 0;
const OFF_TRANSIENT: usize = 8_704;
const OFF_X_MS: usize = 8_960;
const OFF_POWER: usize = 18_176;
const OFF_SCF_SCF: usize = 18_944;
const OFF_SNR_NS: usize = 21_760;
const OFF_RES: usize = 22_016;
const OFF_SCF_ALLOC: usize = 22_272;
const OFF_X_ALLOC: usize = 23_040;

struct Case {
    quality: u8,
    max_band: usize,
    comb: i32,
    pns: f32,
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
fn read_32_f32(b: &[u8], mut off: usize) -> [f32; SUBBANDS] {
    let mut v = [0.0f32; SUBBANDS];
    for slot in v.iter_mut() {
        *slot = f32_at(b, off);
        off += 4;
    }
    v
}
fn read_subbands(b: &[u8], mut off: usize) -> [Subband; SUBBANDS] {
    let mut x = [Subband::ZERO; SUBBANDS];
    for band in x.iter_mut() {
        for n in 0..36 {
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
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(bytes.len() % FRAME_BYTES, 0);
    let frames = bytes.len() / FRAME_BYTES;

    let mut scf_state = ScfState::default();
    let mut counts = (0usize, 0usize, 0usize);

    for frame in 0..frames {
        let b = &bytes[frame * FRAME_BYTES..(frame + 1) * FRAME_BYTES];

        let smr = Smr {
            l: read_32_f32(b, OFF_SMR),
            r: read_32_f32(b, OFF_SMR + 128),
            m: read_32_f32(b, OFF_SMR + 256),
            s: read_32_f32(b, OFF_SMR + 384),
        };
        let mut transient = [0i32; SUBBANDS];
        for (band, slot) in transient.iter_mut().enumerate() {
            *slot = i32_at(b, OFF_TRANSIENT + band * 4);
        }

        let mut power_l = [[0.0f32; 3]; SUBBANDS];
        let mut power_r = [[0.0f32; 3]; SUBBANDS];
        let mut off = OFF_POWER;
        for band in 0..SUBBANDS {
            for k in 0..3 {
                power_l[band][k] = f32_at(b, off);
                off += 4;
                power_r[band][k] = f32_at(b, off);
                off += 4;
            }
        }

        let comp_l = read_32_f32(b, OFF_SNR_NS);
        let comp_r = read_32_f32(b, OFF_SNR_NS + 128);

        // Post-SCF X via the proven SCF stage (fixtures are frozen).
        let mut x = read_subbands(b, OFF_X_MS);
        scf_extraktion(&mut scf_state, case.max_band, case.comb, &mut x).unwrap();

        // Sanity: the SCF output must match the fixture's post-SCF indices.
        let mut off = OFF_SCF_SCF;
        for band in 0..=case.max_band {
            for k in 0..3 {
                assert_eq!(i32_at(b, off), scf_state.scf_l[band][k], "pre-alloc SCF L");
                off += 4;
                assert_eq!(i32_at(b, off), scf_state.scf_r[band][k], "pre-alloc SCF R");
                off += 4;
            }
        }

        let mut scf_l = scf_state.scf_l;
        let mut scf_r = scf_state.scf_r;
        let out = allocate(
            case.max_band,
            case.pns,
            &transient,
            &smr,
            &power_l,
            &power_r,
            &comp_l,
            &comp_r,
            &mut scf_l,
            &mut scf_r,
            &mut x,
        )
        .unwrap();

        // Res (all 32 bands; the reference memsets it).
        let mut off = OFF_RES;
        for band in 0..SUBBANDS {
            let el = i32_at(b, off);
            off += 4;
            let er = i32_at(b, off);
            off += 4;
            assert_eq!(
                out.res_l[band], el,
                "q{} frame {frame} Res_L[{band}]",
                case.quality
            );
            assert_eq!(
                out.res_r[band], er,
                "q{} frame {frame} Res_R[{band}]",
                case.quality
            );
            counts.0 += 2;
        }

        // SCF_Index after Allocate.
        let mut off = OFF_SCF_ALLOC;
        for band in 0..SUBBANDS {
            for k in 0..3 {
                assert_eq!(
                    scf_l[band][k],
                    i32_at(b, off),
                    "q{} frame {frame} alloc SCF_L[{band}][{k}]",
                    case.quality
                );
                off += 4;
                assert_eq!(
                    scf_r[band][k],
                    i32_at(b, off),
                    "q{} frame {frame} alloc SCF_R[{band}][{k}]",
                    case.quality
                );
                off += 4;
                counts.1 += 2;
            }
        }

        // X after Allocate (band-major, all 32 bands).
        let mut off = OFF_X_ALLOC;
        for (band, subband) in x.iter().enumerate() {
            for n in 0..36 {
                let el = f32_at(b, off);
                off += 4;
                let er = f32_at(b, off);
                off += 4;
                assert_eq!(
                    subband.left[n].to_bits(),
                    el.to_bits(),
                    "q{} frame {frame} alloc X_L[{band}][{n}]",
                    case.quality
                );
                assert_eq!(
                    subband.right[n].to_bits(),
                    er.to_bits(),
                    "q{} frame {frame} alloc X_R[{band}][{n}]",
                    case.quality
                );
                counts.2 += 2;
            }
        }
    }

    eprintln!(
        "ALLOC q{}: {frames} frames, Res {} values, SCF {} values, X {} values matched",
        case.quality, counts.0, counts.1, counts.2
    );
}

#[test]
fn allocate_matches_the_reference_bit_for_bit() {
    check_case(&Case {
        quality: 5,
        max_band: 28,
        comb: 9,
        pns: 0.0,
    });
    check_case(&Case {
        quality: 6,
        max_band: 31,
        comb: 7,
        pns: 0.0,
    });
    check_case(&Case {
        quality: 7,
        max_band: 31,
        comb: 5,
        pns: 0.0,
    });
    // q4 (Radio) has PNS > 0 and exercises the PNS_SCF path.
    check_case(&Case {
        quality: 4,
        max_band: 22,
        comb: 10,
        pns: 0.27,
    });
}
