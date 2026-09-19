//! Bit-exact differential tests against the frozen filterbank oracle.
//!
//! `tests/data/filterbank/` holds outputs extracted from the reference C
//! filterbank (see that directory's `README.md` and the temporary tool
//! `tools/extract_filterbank_oracle.c`). These tests reproduce the same
//! deterministic PCM inputs in Rust and require **identical IEEE-754 bit
//! patterns**; they never invoke the C encoder and keep working after it is
//! deleted.

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::filterbank::{
    AnalysisFilterbank, CENTER, CI_OPT, MODULATION, PCM_BLOCK, SUBBANDS, Subband,
};
use sha2::{Digest, Sha256};

const SUBBAND_SAMPLES: usize = 36;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/filterbank")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Silence,
    Impulse,
    Constant,
    Alternating,
    Ramp,
    Transient,
    Stereo,
    StateCarry,
}

impl Kind {
    fn parse(name: &str) -> Self {
        match name {
            "silence" => Self::Silence,
            "impulse" => Self::Impulse,
            "constant" => Self::Constant,
            "alternating" => Self::Alternating,
            "ramp" => Self::Ramp,
            "transient" => Self::Transient,
            "stereo" => Self::Stereo,
            "state_carry" => Self::StateCarry,
            other => panic!("unknown filterbank case kind {other}"),
        }
    }
}

struct Case {
    name: String,
    kind: Kind,
    frames: usize,
    calls: usize,
    max_band: usize,
    bytes: usize,
    sha256: String,
    init_l_bits: u32,
    init_r_bits: u32,
    input_xor: u32,
}

fn parse_index() -> Vec<Case> {
    let text = std::fs::read_to_string(data_dir().join("index.txt")).expect("filterbank index");
    let mut cases = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(f.len(), 10, "index line: {line}");
        cases.push(Case {
            name: f[0].to_owned(),
            kind: Kind::parse(f[1]),
            frames: f[2].parse().unwrap(),
            calls: f[3].parse().unwrap(),
            max_band: f[4].parse().unwrap(),
            bytes: f[5].parse().unwrap(),
            sha256: f[6].to_owned(),
            init_l_bits: u32::from_str_radix(f[7], 16).unwrap(),
            init_r_bits: u32::from_str_radix(f[8], 16).unwrap(),
            input_xor: u32::from_str_radix(f[9], 16).unwrap(),
        });
    }
    assert!(!cases.is_empty(), "no filterbank cases");
    cases
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The reference LCG float generator: exact in `f32`.
struct Lcg(u32);

impl Lcg {
    fn draw(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.0 >> 9) as f32) * (1.0f32 / 4_194_304.0) - 1.0
    }
}

fn fill(
    kind: Kind,
    frame: usize,
    rng_l: &mut Lcg,
    rng_r: &mut Lcg,
    left: &mut [f32],
    right: &mut [f32],
) {
    left.fill(0.0);
    right.fill(0.0);
    let block = 1152usize;
    match kind {
        Kind::Silence => {}
        Kind::Impulse => left[CENTER] = 1.0,
        Kind::Constant => {
            for j in 0..block {
                left[CENTER + j] = 0.5;
                right[CENTER + j] = 0.5;
            }
        }
        Kind::Alternating => {
            for j in 0..block {
                let v = if j & 1 == 1 { -0.5 } else { 0.5 };
                left[CENTER + j] = v;
                right[CENTER + j] = v;
            }
        }
        Kind::Ramp => {
            for j in 0..block {
                let t = (frame * block + j) % 512;
                let v = (t as i32 - 256) as f32 * (1.0f32 / 512.0);
                left[CENTER + j] = v;
                right[CENTER + j] = v;
            }
        }
        Kind::Transient => {
            for j in 0..block {
                let v = if j == 100 {
                    1.0
                } else if j == 500 {
                    -0.5
                } else {
                    0.0
                };
                left[CENTER + j] = v;
                right[CENTER + j] = v;
            }
        }
        Kind::Stereo | Kind::StateCarry => {
            for j in 0..block {
                left[CENTER + j] = rng_l.draw();
                right[CENTER + j] = rng_r.draw();
            }
        }
    }
}

fn read_bits(path: &Path) -> Vec<u32> {
    let bytes = std::fs::read(path).expect("fixture bytes");
    assert!(bytes.len() % 4 == 0, "fixture size not a multiple of 4");
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Compares one frame's subbands against the expected bit patterns.
fn compare_frame(
    case: &str,
    frame: usize,
    max_band: usize,
    expected: &[u32],
    out: &[Subband; SUBBANDS],
) -> usize {
    let mut index = 0usize;
    for (band, subband) in out.iter().enumerate().take(max_band + 1) {
        for (channel, samples) in [("L", &subband.left), ("R", &subband.right)] {
            for (n, actual) in samples.iter().enumerate() {
                let expected = f32::from_bits(expected[index]);
                if actual.to_bits() != expected.to_bits() {
                    panic!(
                        "{case} frame {frame} channel {channel} band {band} sample {n}: \
                         expected {:#010x} ({expected:e}), actual {:#010x} ({actual:e})",
                        expected.to_bits(),
                        actual.to_bits(),
                    );
                }
                index += 1;
            }
        }
    }
    index
}

#[test]
fn frozen_modulation_matrix_matches_the_reference_bits() {
    let bits = parse_hex_bits(&data_dir().join("modulation_bits.txt"));
    assert_eq!(bits.len(), MODULATION.len());
    for (i, (&expected, actual)) in bits.iter().zip(MODULATION.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected,
            "MODULATION[{i}] differs from the extracted reference"
        );
    }
}

#[test]
fn computed_prototype_window_matches_the_reference_bits() {
    let bits = parse_hex_bits(&data_dir().join("ci_opt_bits.txt"));
    assert_eq!(bits.len(), CI_OPT.len());
    for (i, (&expected, actual)) in bits.iter().zip(CI_OPT.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected,
            "CI_OPT[{i}] differs from the extracted reference"
        );
    }
}

fn parse_hex_bits(path: &Path) -> Vec<u32> {
    std::fs::read_to_string(path)
        .expect("bit dump")
        .lines()
        .map(|line| u32::from_str_radix(line.trim(), 16).expect("hex bits"))
        .collect()
}

#[test]
fn filterbank_fixtures_match_the_reference_bit_for_bit() {
    let mut total_coefficients = 0usize;
    for case in parse_index() {
        let path = data_dir().join(format!("{}.f32le", case.name));
        let bytes = std::fs::read(&path).expect("case fixture");
        assert_eq!(
            bytes.len(),
            case.bytes,
            "{}: fixture size disagrees with the index",
            case.name
        );
        assert_eq!(
            bytes.len(),
            case.calls * (case.max_band + 1) * SUBBAND_SAMPLES * 2 * 4,
            "{}: fixture size",
            case.name
        );
        assert_eq!(
            sha256_hex(&bytes),
            case.sha256,
            "{}: frozen fixture was modified",
            case.name
        );
        let expected = read_bits(&path);

        let mut left = vec![0.0f32; PCM_BLOCK];
        let mut right = vec![0.0f32; PCM_BLOCK];
        let mut rng_l = Lcg(seed_for(&case, "l"));
        let mut rng_r = Lcg(seed_for(&case, "r"));
        let mut filterbank = AnalysisFilterbank::new();
        let mut out = [Subband::ZERO; SUBBANDS];

        if case.frames == 0 {
            // `init` case: Analyse_Init output with fixed constants.
            filterbank
                .init(0.25, -0.5, &mut out, case.max_band)
                .unwrap();
            let used = compare_frame(
                &case.name,
                0,
                case.max_band,
                &expected[..frame_len(case.max_band)],
                &out,
            );
            assert_eq!(used, frame_len(case.max_band));
            total_coefficients += used;
            continue;
        }

        fill(case.kind, 0, &mut rng_l, &mut rng_r, &mut left, &mut right);
        assert_eq!(
            left[CENTER].to_bits(),
            case.init_l_bits,
            "{}: generated input does not match the reference (L init)",
            case.name
        );
        assert_eq!(
            right[CENTER].to_bits(),
            case.init_r_bits,
            "{}: generated input does not match the reference (R init)",
            case.name
        );
        filterbank
            .init(left[CENTER], right[CENTER], &mut out, case.max_band)
            .unwrap();

        let mut input_xor = 0u32;
        let mut offset = 0usize;
        for frame in 0..case.frames {
            if frame > 0 {
                fill(
                    case.kind, frame, &mut rng_l, &mut rng_r, &mut left, &mut right,
                );
            }
            for j in CENTER..PCM_BLOCK {
                input_xor ^= left[j].to_bits();
                input_xor ^= right[j].to_bits();
            }
            filterbank
                .process(&left, &right, &mut out, case.max_band)
                .unwrap();
            let len = frame_len(case.max_band);
            compare_frame(
                &case.name,
                frame,
                case.max_band,
                &expected[offset..offset + len],
                &out,
            );
            offset += len;
            total_coefficients += len;
        }
        assert_eq!(
            input_xor, case.input_xor,
            "{}: generated input does not match the reference (xor)",
            case.name
        );
    }
    assert!(total_coefficients > 0);
}

const fn frame_len(max_band: usize) -> usize {
    (max_band + 1) * SUBBAND_SAMPLES * 2
}

fn seed_for(case: &Case, channel: &str) -> u32 {
    match (case.kind, channel) {
        (Kind::Stereo, "l") => 0x1234_5678,
        (Kind::Stereo, "r") => 0x9abc_def0,
        (Kind::StateCarry, "l") => 0x0bad_f00d,
        (Kind::StateCarry, "r") => 0xfeed_face,
        _ => 0,
    }
}
