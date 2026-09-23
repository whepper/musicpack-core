//! J.6 wide-PCM differential coverage (whole encoder).
//!
//! Replays `tests/data/encoder/wide_manifest.txt`: deterministic 24- and
//! 32-bit inputs encoded through [`MusepackEncoder::encode_s32`] must
//! reproduce the committed scalar C `mpcenc` 1.32.0 streams **byte for
//! byte**, without ever truncating the source to 16 bits.
//!
//! The corpus covers, per the J.6 slice:
//!
//! * **24-bit:** deterministic `ramp`, `lowamp` (entirely below the 16-bit
//!   truncation threshold), `fullrange` (exact min/max + full-range LCG
//!   noise), the representative `noise` signal at all four SV8 rates, and
//!   a mono row (the C mono conversion branch);
//! * **32-bit:** deterministic `ramp`, `lowamp` and `fullrange`.
//!
//! Three independent assertions per slice:
//!
//! 1. byte identity against the frozen C fixtures (with first-divergence
//!    diagnostics — no hash-only acceptance);
//! 2. every row **differs** from what its `>> 16` `i16` truncation would
//!    encode, so the wide path cannot silently collapse to the old
//!    16-bit input;
//! 3. a coverage gate over the manifest itself.
//!
//! The *intermediate* conversion values (pre-fix `f32`, stored `L/R/M/S`)
//! are pinned separately against the C-generated
//! `pcm_conversion_oracle.txt` by the `read_block_matches_the_c_pcm_conversion_oracle`
//! unit test. The existing 16-bit corpora stay covered by
//! `encoder_whole.rs`, `encoder_matrix.rs` and `encoder_fractional.rs`
//! unchanged.
//!
//! The test never invokes the C encoder; the fixtures are frozen C output.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};

/// The four SV8 sample rates.
const RATES: [u32; 4] = [44100, 48000, 37800, 32000];

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/encoder")
}

fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

/// The signed `depth`-bit reinterpretation of an LCG word, matching
/// `tools/gen_encoder_fixtures.py::signed_bits`.
fn signed_bits(state: u32, depth: u32) -> i32 {
    if depth == 24 {
        let v = (state & 0x00FF_FFFF) as i32;
        if v >= 0x0080_0000 { v - 0x0100_0000 } else { v }
    } else {
        state as i32
    }
}

/// Deterministic integer-only wide PCM matching
/// `tools/gen_encoder_fixtures.py::gen_wide`, returned **left-aligned**
/// (`value << (32 - depth)`, the `encode_s32` input contract). For
/// `channels == 1` only the left/LCG stream is emitted (mono).
fn gen_pcm(depth: u32, kind: &str, frames: usize, channels: u32) -> Vec<i32> {
    assert!(matches!(depth, 24 | 32), "wide corpus is 24/32-bit");
    assert!(
        channels == 1 || channels == 2,
        "J.6 adds no >2-channel support"
    );
    let shift = 32 - depth;
    let (lo, hi) = if depth == 24 {
        (-(1i32 << 23), (1i32 << 23) - 1)
    } else {
        (i32::MIN, i32::MAX)
    };
    let mut sl = 0x1234_5678u32;
    let mut sr = 0x9ABC_DEF0u32;
    let mut out = Vec::with_capacity(frames * channels as usize);
    for i in 0..frames {
        let (l, r): (i32, i32) = match kind {
            "ramp" => {
                let step = if depth == 24 { 32767 } else { 8388607 };
                let v = ((i % 512) as i32 - 256) * step;
                (v, v)
            }
            "lowamp" => {
                let mask: u32 = if depth == 24 { 0xFF } else { 0xFFFF };
                sl = lcg(&mut sl);
                let l = (sl & mask) as i32;
                sr = lcg(&mut sr);
                let r = (sr & mask) as i32;
                (l, r)
            }
            "fullrange" => match i {
                0 => (hi, lo),
                1 => (lo, hi),
                _ => {
                    sl = lcg(&mut sl);
                    sr = lcg(&mut sr);
                    (signed_bits(sl, depth), signed_bits(sr, depth))
                }
            },
            "noise" => {
                sl = lcg(&mut sl);
                sr = lcg(&mut sr);
                (signed_bits(sl, depth), signed_bits(sr, depth))
            }
            other => panic!("unknown wide-corpus kind {other}"),
        };
        out.push(l << shift);
        if channels == 2 {
            out.push(r << shift);
        }
    }
    out
}

struct Row {
    name: String,
    quality: f32,
    rate: u32,
    kind: String,
    depth: u32,
    frames: usize,
    channels: u32,
}

/// Parses `tests/data/encoder/wide_manifest.txt`
/// (`name quality rate kind depth frames channels bytes sha256`).
fn load_manifest() -> Vec<Row> {
    let text =
        std::fs::read_to_string(data_dir().join("wide_manifest.txt")).expect("wide_manifest.txt");
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(f.len(), 9, "wide manifest row shape: {line}");
        rows.push(Row {
            name: f[0].to_string(),
            quality: f[1].parse().expect("quality"),
            rate: f[2].parse().expect("rate"),
            kind: f[3].to_string(),
            depth: f[4].parse().expect("depth"),
            frames: f[5].parse().expect("frames"),
            channels: f[6].parse().expect("channels"),
        });
    }
    rows
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

/// Light structural validation: every produced stream is an SV8 container
/// with a stream header, audio packets and an end marker. (Deep `SH`/`EI`/
/// `ST` parsing is covered by `encoder_matrix.rs`.)
fn assert_sv8(bytes: &[u8], name: &str) {
    assert_eq!(&bytes[0..4], b"MPCK", "{name}: missing MPCK magic");
    assert!(
        bytes.windows(2).any(|w| w == b"SH"),
        "{name}: missing SH block"
    );
    assert!(
        bytes.windows(2).any(|w| w == b"AP"),
        "{name}: missing AP block"
    );
    assert!(
        bytes.windows(2).any(|w| w == b"SE"),
        "{name}: missing SE block"
    );
}

/// The manifest must cover exactly the J.6 slice: the required signal
/// kinds at both depths, 24-bit `noise` at every SV8 rate, one mono row,
/// and nothing beyond 2 channels.
#[test]
fn wide_manifest_covers_the_j6_surface() {
    let rows = load_manifest();
    assert_eq!(rows.len(), 11, "the J.6 wide corpus has 11 rows");

    for depth in [24u32, 32] {
        let kinds: BTreeSet<&str> = rows
            .iter()
            .filter(|r| r.depth == depth && r.channels == 2)
            .map(|r| r.kind.as_str())
            .collect();
        for required in ["ramp", "lowamp", "fullrange"] {
            assert!(
                kinds.contains(required),
                "the {depth}-bit corpus must contain the deterministic `{required}` signal"
            );
        }
    }
    assert!(
        rows.iter().any(|r| r.depth == 24 && r.kind == "noise"),
        "the 24-bit corpus must contain the representative `noise` signal"
    );
    let noise_rates: BTreeSet<u32> = rows
        .iter()
        .filter(|r| r.depth == 24 && r.kind == "noise" && r.channels == 2)
        .map(|r| r.rate)
        .collect();
    let all: BTreeSet<u32> = RATES.iter().copied().collect();
    assert_eq!(
        noise_rates, all,
        "24-bit noise must cover every SV8 sample rate"
    );
    assert!(
        rows.iter().any(|r| r.depth == 24 && r.channels == 1),
        "the wide corpus must contain a mono row"
    );
    assert!(
        rows.iter().all(|r| r.channels == 1 || r.channels == 2),
        "J.6 does not add >2-channel support"
    );
    assert!(
        rows.iter().all(|r| r.quality == 5.0 && r.frames == 5000),
        "wide rows use the default configuration"
    );
}

/// Every wide row must reproduce the frozen scalar-C bytes through
/// `encode_s32`, with first-divergence diagnostics on failure.
#[test]
fn every_wide_row_is_byte_identical_to_the_c_reference() {
    let rows = load_manifest();
    let mut cases = 0usize;
    let mut bytes_total = 0usize;
    let mut per_depth: BTreeSet<u32> = BTreeSet::new();
    let mut first_divergence: Option<String> = None;

    for row in &rows {
        let pcm = gen_pcm(row.depth, &row.kind, row.frames, row.channels);
        let config = EncoderConfig::new(row.quality, row.rate, row.channels);
        let actual = MusepackEncoder::new(config)
            .unwrap_or_else(|e| panic!("{}: rejected configuration: {e}", row.name))
            .encode_s32(&pcm)
            .unwrap_or_else(|e| panic!("{}: encode_s32 failed: {e}", row.name));
        assert_sv8(&actual, &row.name);

        let expected = fixture(&row.name);
        cases += 1;
        bytes_total += expected.len();
        per_depth.insert(row.depth);

        if actual != expected {
            let first = first_diff(&actual, &expected);
            let msg = format!(
                "{}: first divergence at byte {first} (expected {:#04x}, actual {:#04x}); \
                 expected {} bytes, actual {} bytes",
                row.name,
                expected.get(first).copied().unwrap_or(0),
                actual.get(first).copied().unwrap_or(0),
                expected.len(),
                actual.len()
            );
            if first_divergence.is_none() {
                first_divergence = Some(msg.clone());
            }
            eprintln!("MISMATCH {msg}");
        } else {
            eprintln!("ok {} ({} bytes)", row.name, expected.len());
        }
    }

    assert_eq!(cases, 11, "all 11 wide rows compared");
    assert!(
        per_depth.contains(&24) && per_depth.contains(&32),
        "both 24- and 32-bit rows compared"
    );
    assert!(bytes_total > 40_000, "compared a substantial byte volume");
    if let Some(msg) = first_divergence {
        panic!("wide-PCM mismatch: {msg}");
    }
    eprintln!("wide encoder: {cases} cases, {bytes_total} bytes matched");
}

/// Phase 5 discrimination, per row: where the source carries low-order
/// information, `encode_s32` must produce a **different** stream than the
/// `>> 16` `i16` truncation of the same source would — 24-bit and 32-bit
/// input is not accidentally reduced anywhere in the new path.
#[test]
fn wide_rows_differ_from_their_i16_truncated_encodings() {
    let rows = load_manifest();
    for row in &rows {
        let pcm = gen_pcm(row.depth, &row.kind, row.frames, row.channels);
        let config = EncoderConfig::new(row.quality, row.rate, row.channels);

        let wide = MusepackEncoder::new(config)
            .unwrap()
            .encode_s32(&pcm)
            .unwrap();

        // The old reduction: top 16 bits of the left-aligned sample —
        // exactly what a caller that truncated to `i16` would feed.
        let truncated: Vec<i16> = pcm.iter().map(|&v| (v >> 16) as i16).collect();
        let narrow = MusepackEncoder::new(config)
            .unwrap()
            .encode(&truncated)
            .unwrap();

        assert_ne!(
            wide, narrow,
            "{}: the wide stream must differ from its i16 truncation \
             (the source carries distinguishing low-order information)",
            row.name
        );
    }
}
