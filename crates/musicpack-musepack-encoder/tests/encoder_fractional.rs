//! J.2 fractional-quality differential coverage.
//!
//! Three oracles, deliberately kept separate (the J.2 investigation proved
//! stream equality on one input is *not* evidence of table equivalence):
//!
//! * **whole-stream:** every row of `fractional_manifest.txt` must be
//!   byte-identical to the scalar C `mpcenc`1.32.0 fixture;
//! * **quality semantics:** the C-generated `fractional_params.txt` oracle
//!   pins profile selection/interpolation bits (f32 parse, clip, mix,
//!   discrete rows) for the corpus qualities;
//! * **tables** are covered bit-for-bit against the committed C dumps by
//!   `psy::computed`'s unit tests (all44 integer configs +23 fractional
//!   pairs).
//!
//! Clipping identities compare against the committed C-generated `q0`/`q10`
//! fixtures, and non-finite qualities must be rejected with a typed error
//! (intentional boundary: C's `NaN` path is undefined behaviour).

use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};
use musicpack_musepack_encoder::error::EncoderError;
use musicpack_musepack_encoder::psy::{PsyParams, PsychoacousticModel};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/encoder")
}

fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

fn signed16(state: u32) -> i16 {
    (((state >> 16) & 0xFFFF) as i32 - 32768) as i16
}

/// Deterministic PCM matching `tools/gen_encoder_fixtures.py` for the two
/// signal kinds the fractional corpus uses: `noise` (stochastic) and `ramp`
/// (deterministic periodic sawtooth — the tonal input).
fn gen_pcm(kind: &str, frames: usize) -> Vec<i16> {
    let mut sl = 0x1234_5678u32;
    let mut sr = 0x9ABC_DEF0u32;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let (l, r): (i16, i16) = match kind {
            "noise" => (signed16(lcg(&mut sl)), signed16(lcg(&mut sr))),
            "ramp" => {
                let v = (((i % 512) as i32 - 256) * 60) as i16;
                (v, v)
            }
            other => panic!("unknown fractional-corpus kind {other}"),
        };
        out.push(l);
        out.push(r);
    }
    out
}

fn encode(quality: f32, rate: u32, kind: &str, frames: usize) -> Vec<u8> {
    let pcm = gen_pcm(kind, frames);
    let mut enc =
        MusepackEncoder::new(EncoderConfig::new(quality, rate, 2)).expect("encoder accepts pair");
    enc.encode(&pcm).expect("encode")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(data_dir().join(format!("{name}.mpc")))
        .unwrap_or_else(|e| panic!("missing fixture {name}: {e}"))
}

fn first_diff(actual: &[u8], expected: &[u8]) -> usize {
    actual
        .iter()
        .zip(expected)
        .position(|(a, b)| a != b)
        .unwrap_or(actual.len().min(expected.len()))
}

/// Whole-stream parity for every fractional corpus row: Rust == scalar C.
#[test]
fn fractional_streams_match_the_c_reference_byte_for_byte() {
    let manifest =
        std::fs::read_to_string(data_dir().join("fractional_manifest.txt")).expect("manifest");
    let mut cases = 0usize;
    let mut total = 0usize;
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let name = f[0];
        let quality: f32 = f[1].parse().unwrap();
        let rate: u32 = f[2].parse().unwrap();
        let kind = f[3];
        let frames: usize = f[4].parse().unwrap();

        let expected = fixture(name);
        let actual = encode(quality, rate, kind, frames);
        cases += 1;
        total += expected.len();
        assert_eq!(
            actual,
            expected,
            "{name}: first divergence at byte {} (expected {} bytes, got {} bytes)",
            first_diff(&actual, &expected),
            expected.len(),
            actual.len()
        );
    }
    assert_eq!(cases, 27, "the J.2 fractional corpus has27 rows");
    assert!(total > 100_000, "compared a substantial byte volume");
}

/// The C `params` oracle pins the full quality path: f32 parse (including
/// the parse-merge boundaries `4.9999999 ->5.0` and `6.0000001 ->6.0`),
/// clipping/`int_part`, the interpolated profile scalars and every discrete
/// profile selection.
#[test]
fn fractional_quality_params_match_the_c_oracle() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/psy/fractional_params.txt");
    let text = std::fs::read_to_string(&path).expect("fractional_params.txt");
    let hex_keys = [
        "qf_bits", "tmn", "nmt", "bw", "pns", "shortthr", "transdet", "varltq", "off", "lmax",
        "minsmr",
    ];
    let int_keys = [
        "int_part",
        "MainQual",
        "off_i",
        "lmax_i",
        "ear",
        "minval_choice",
        "tmpmask",
        "cvd",
        "ms",
        "ns",
        "comb",
    ];
    let mut lines_seen = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            line.starts_with("params "),
            "unexpected oracle line: {line}"
        );
        let mut kv = std::collections::HashMap::new();
        for tok in line.split_whitespace().skip(1) {
            let (k, v) = tok.split_once('=').expect("key=value");
            kv.insert(k.to_string(), v.to_string());
        }
        let qin = kv.get("qin").expect("qin").clone();
        let qf: f32 = qin.parse().expect("qin parses as f32");
        let p = PsyParams::from_quality(qf);
        lines_seen += 1;

        let actual_hex = [
            ("qf_bits", format!("{:08x}", qf.to_bits())),
            ("tmn", format!("{:08x}", p.tmn.to_bits())),
            ("nmt", format!("{:08x}", p.nmt.to_bits())),
            ("bw", format!("{:08x}", p.band_width.to_bits())),
            ("pns", format!("{:08x}", p.pns.to_bits())),
            ("shortthr", format!("{:08x}", p.short_thr.to_bits())),
            ("transdet", format!("{:08x}", p.trans_detect.to_bits())),
            ("varltq", format!("{:08x}", p.var_ltq.to_bits())),
            ("off", format!("{:08x}", p.ltq_offset.to_bits())),
            ("lmax", format!("{:08x}", p.ltq_max.to_bits())),
            ("minsmr", format!("{:08x}", p.min_smr.to_bits())),
        ];
        for (key, got) in actual_hex {
            assert!(hex_keys.contains(&key));
            let want = kv.get(key).expect(key);
            assert_eq!(want, &got, "q{qin}: {key} C={want} rust={got}");
        }
        let qc = qf.clamp(0.0, 10.0);
        let actual_int = [
            ("int_part", qc as i32),
            ("MainQual", p.main_qual),
            ("off_i", p.ltq_offset as i32),
            ("lmax_i", p.ltq_max as i32),
            ("ear", p.ear_model_flag as i32),
            ("minval_choice", p.min_val_choice),
            ("tmpmask", i32::from(p.tmp_mask_used)),
            ("cvd", i32::from(p.cvd_used)),
            ("ms", i32::from(p.ms_channelmode)),
            ("ns", p.ns_order as i32),
            ("comb", p.comb_penalities),
        ];
        for (key, got) in actual_int {
            assert!(int_keys.contains(&key));
            let want: i32 = kv.get(key).expect(key).parse().unwrap();
            assert_eq!(want, got, "q{qin}: {key} C={want} rust={got}");
        }
        // FullQual: C prints the (f64-widened) f32 with %.9g, which carries
        // enough digits to round-trip back to the identical f32.
        let want_fq: f32 = kv.get("FullQual").expect("FullQual").parse().unwrap();
        assert_eq!(
            want_fq.to_bits(),
            p.full_qual.to_bits(),
            "q{qin}: FullQual C={want_fq:?} rust={:?}",
            p.full_qual
        );
        // The oracle's own consistency: qin parses to the recorded bits.
        let want_qf = kv.get("qf_bits").unwrap();
        assert_eq!(want_qf, &format!("{:08x}", qf.to_bits()), "q{qin} parse");
    }
    assert_eq!(lines_seen, 11, "the params oracle pins11 corpus qualities");
}

/// C clips finite qualities before profile selection (`profile.c`:
/// `clip(qual,0,10)`), so negative qualities produce byte-identical streams
/// to `q0` and qualities above10 to `q10` — asserted against the committed
/// **C-generated** endpoint fixtures. `9.9999` must NOT snap to `q10`
/// (clipping happens only beyond the boundary).
#[test]
fn clipping_produces_byte_identical_endpoint_streams() {
    let q0 = fixture("q0-44100-noise");
    let q10 = fixture("q10-44100-noise");

    for q in [-100.0f32, -1.0, -0.0001] {
        let bytes = encode(q, 44100, "noise", 5000);
        assert_eq!(
            bytes,
            q0,
            "q{q}: first divergence at byte {}",
            first_diff(&bytes, &q0)
        );
    }
    for q in [10.0001f32, 11.0, 100.0] {
        let bytes = encode(q, 44100, "noise", 5000);
        assert_eq!(
            bytes,
            q10,
            "q{q}: first divergence at byte {}",
            first_diff(&bytes, &q10)
        );
    }
    // Endpoint controls: the integer paths still equal the C fixtures.
    assert_eq!(encode(0.0, 44100, "noise", 5000), q0);
    assert_eq!(encode(10.0, 44100, "noise", 5000), q10);

    // No snap-to-q10 below the boundary: q9.9999 differs from q10 and
    // equals its own C fixture.
    let near = encode(9.9999, 44100, "noise", 5000);
    assert_ne!(near, q10, "q9.9999 must not be snapped to q10");
    assert_eq!(near, fixture("q9.9999-44100-noise"));
}

/// Non-finite qualities are rejected with a typed error before any
/// psychoacoustic calculation — an intentional compatibility boundary, not
/// C parity (C's `NaN` path is undefined/platform-dependent).
#[test]
fn non_finite_qualities_are_rejected_with_a_typed_error() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let via_encoder = MusepackEncoder::new(EncoderConfig::new(bad, 44100, 2));
        assert!(
            matches!(via_encoder, Err(EncoderError::NonFiniteQuality(q)) if q.is_nan() || q.is_infinite()),
            "q{bad} must be rejected at the encoder"
        );
        let via_model = PsychoacousticModel::new(bad, 44100.0);
        assert!(
            matches!(via_model, Err(EncoderError::NonFiniteQuality(q)) if q.is_nan() || q.is_infinite()),
            "q{bad} must be rejected at the psy model"
        );
    }
}
