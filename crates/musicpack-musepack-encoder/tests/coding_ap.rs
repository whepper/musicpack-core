//! Bit-exact AP-payload tests against the frozen coding oracle.
//!
//! Reconstructs the frame-coding state from `coding-q{4,5,6,7}.bin` (Res,
//! post-allocation SCF_Index, MS_Flag, Q) and compares the flushed `AP` blocks
//! with `ap-q{4,5,6,7}.bin`. Never invokes the C encoder.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::coding::FrameEncoder;

const FRAME_BYTES: usize = 36_864;
const OFF_MS: usize = 8_832;
const OFF_SCF_ALLOC: usize = 22_272;
const OFF_RES: usize = 22_016;
const OFF_Q: usize = 32_256;
const SUBBANDS: usize = 32;
const SAMPLES: usize = 36;

struct Case {
    quality: u8,
    max_band: i32,
    ms_channelmode: u32,
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/coding")
}

fn i32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn i16_at(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}

/// Reads one `ap-qN.bin`: records of `u32 marker`, `u32 size`, then `size`
/// bytes. Returns the concatenated block bytes.
fn read_ap_fixture(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("ap fixture");
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let marker = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        assert_eq!(marker, 0x4150_5A00, "ap record marker");
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        at += 8;
        out.extend_from_slice(&bytes[at..at + size]);
        at += size;
    }
    out
}

fn check_case(case: &Case) {
    let coding = std::fs::read(data_dir().join(format!("coding-q{}.bin", case.quality)))
        .expect("coding fixture");
    assert_eq!(coding.len() % FRAME_BYTES, 0);
    let frames = coding.len() / FRAME_BYTES;

    let mut encoder = FrameEncoder::new(2);
    for frame in 0..frames {
        let b = &coding[frame * FRAME_BYTES..(frame + 1) * FRAME_BYTES];

        let mut res_l = [0i32; SUBBANDS];
        let mut res_r = [0i32; SUBBANDS];
        let mut off = OFF_RES;
        for band in 0..SUBBANDS {
            res_l[band] = i32_at(b, off);
            off += 4;
            res_r[band] = i32_at(b, off);
            off += 4;
        }

        let mut scf_l = [[0i32; 3]; SUBBANDS];
        let mut scf_r = [[0i32; 3]; SUBBANDS];
        let mut off = OFF_SCF_ALLOC;
        for band in 0..SUBBANDS {
            for k in 0..3 {
                scf_l[band][k] = i32_at(b, off);
                off += 4;
                scf_r[band][k] = i32_at(b, off);
                off += 4;
            }
        }

        let mut ms_flag = [0i32; SUBBANDS];
        for (band, slot) in ms_flag.iter_mut().enumerate() {
            *slot = i32_at(b, OFF_MS + band * 4);
        }

        let mut q_l = [[0i16; SAMPLES]; SUBBANDS];
        let mut q_r = [[0i16; SAMPLES]; SUBBANDS];
        let mut off = OFF_Q;
        for band in 0..SUBBANDS {
            for n in 0..SAMPLES {
                q_l[band][n] = i16_at(b, off);
                off += 2;
                q_r[band][n] = i16_at(b, off);
                off += 2;
            }
        }

        encoder
            .encode_frame(
                case.max_band,
                case.ms_channelmode,
                &res_l,
                &res_r,
                &scf_l,
                &scf_r,
                &ms_flag,
                &q_l,
                &q_r,
            )
            .unwrap();
    }

    let expected = read_ap_fixture(&data_dir().join(format!("ap-q{}.bin", case.quality)));
    let actual = encoder.into_blocks();

    if actual != expected {
        let first = actual
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        let byte = first / 8;
        let bit = first % 8;
        panic!(
            "q{}: AP payload mismatch at byte {byte} bit {bit} \
             (expected {:#04x}, actual {:#04x}); expected {} bytes, actual {} bytes",
            case.quality,
            expected.get(first).copied().unwrap_or(0),
            actual.get(first).copied().unwrap_or(0),
            expected.len(),
            actual.len()
        );
    }

    eprintln!(
        "AP q{}: {frames} frames, {} block bytes matched",
        case.quality,
        actual.len()
    );
}

#[test]
fn ap_payload_matches_the_reference_bit_for_bit() {
    check_case(&Case {
        quality: 4,
        max_band: 22,
        ms_channelmode: 6,
    });
    check_case(&Case {
        quality: 5,
        max_band: 28,
        ms_channelmode: 11,
    });
    check_case(&Case {
        quality: 6,
        max_band: 31,
        ms_channelmode: 12,
    });
    check_case(&Case {
        quality: 7,
        max_band: 31,
        ms_channelmode: 13,
    });
}
