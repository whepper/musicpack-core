//! J.1 integer-parity differential matrix (whole encoder).
//!
//! Replays `tests/data/encoder/matrix_manifest.txt`: every integer quality
//! `0..=10` × every SV8 sample rate (44 configurations) with two deterministic
//! signal kinds, mono at q5 × four rates, and long multi-`AP`-block cases.
//! For every row the Rust encoder must reproduce the committed reference
//! `mpcenc` stream **byte for byte**.
//!
//! Each produced stream is additionally validated structurally as SV8 (block
//! framing, `SH` CRC and fields, `EI` profile/PNS/version, `SO`/`ST`/`SE`
//! presence and layout), so a row proves both byte identity against the
//! oracle and a well-formed, decodable container.
//!
//! The test never invokes the C encoder; the fixtures are frozen C output.
//! The original 21-case corpus stays covered by `encoder_whole.rs`
//! unchanged — rows here whose names collide with that corpus deliberately
//! point at the same frozen files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};
use musicpack_musepack_encoder::sv8::crc32;

/// Integer qualities `0..=10`.
const QUALITIES: [u32; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
/// The four SV8 sample rates.
const RATES: [u32; 4] = [44100, 48000, 37800, 32000];

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

/// Deterministic integer-only PCM matching `tools/gen_encoder_fixtures.py`.
/// For `channels == 1` only the left/LCG stream is emitted (mono).
fn gen_pcm(kind: &str, frames: usize, channels: u32) -> Vec<i16> {
    assert!(
        channels == 1 || channels == 2,
        "matrix covers mono and stereo"
    );
    let mut sl = 0x1234_5678u32;
    let mut sr = 0x9ABC_DEF0u32;
    let mut out = Vec::with_capacity(frames * channels as usize);
    for i in 0..frames {
        let l: i16 = match kind {
            "noise" => {
                sl = lcg(&mut sl);
                sr = lcg(&mut sr);
                signed16(sl)
            }
            "transient" => {
                let v = match i {
                    100 => 30000,
                    5000 => -20000,
                    20000 => 25000,
                    _ => 0,
                };
                v as i16
            }
            other => panic!("unknown kind {other}"),
        };
        out.push(l);
        if channels == 2 {
            let r = match kind {
                "noise" => signed16(sr),
                _ => l,
            };
            out.push(r);
        }
    }
    out
}

struct Row {
    name: String,
    quality: u32,
    rate: u32,
    kind: String,
    frames: usize,
    channels: u32,
}

fn load_manifest() -> Vec<Row> {
    let text = std::fs::read_to_string(data_dir().join("matrix_manifest.txt"))
        .expect("matrix_manifest.txt");
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(f.len(), 8, "manifest row must have 8 fields: {line}");
        rows.push(Row {
            name: f[0].to_string(),
            quality: f[1].parse().unwrap(),
            rate: f[2].parse().unwrap(),
            kind: f[3].to_string(),
            frames: f[4].parse().unwrap(),
            channels: f[5].parse().unwrap(),
        });
    }
    assert!(!rows.is_empty(), "empty matrix manifest");
    rows
}

// ---------------------------------------------------------------------------
// Structural SV8 validation
// ---------------------------------------------------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            assert!(
                self.bit < self.data.len() * 8,
                "SH payload truncated at bit {}",
                self.bit
            );
            let byte = self.data[self.bit >> 3];
            v = (v << 1) | u32::from((byte >> (7 - (self.bit & 7))) & 1);
            self.bit += 1;
        }
        v
    }

    /// SV8 size field: one byte per 7 bits, continuation on all but the last.
    fn read_varint_bytes(&mut self) -> u64 {
        assert_eq!(self.bit % 8, 0, "size fields are byte-aligned");
        let mut v = 0u64;
        loop {
            let b = self.read(8);
            v = v * 128 + u64::from(b & 0x7F);
            if b & 0x80 == 0 {
                return v;
            }
        }
    }
}

fn read_byte_varint(data: &[u8], at: &mut usize) -> u64 {
    let mut v = 0u64;
    loop {
        let b = *data.get(*at).expect("truncated size field");
        *at += 1;
        v = v * 128 + u64::from(b & 0x7F);
        if b & 0x80 == 0 {
            return v;
        }
    }
}

const RATE_INDEX: [u32; 4] = [44100, 48000, 37800, 32000];

/// Walks every block of an SV8 stream, asserts the framing and the expected
/// header sequence, and validates the `SH`/`EI`/`ST` fields against the row's
/// configuration. Panics on any structural inconsistency.
fn parse_and_validate_sv8(stream: &[u8], row: &Row, expected_max_band: usize) {
    assert_eq!(&stream[0..4], b"MPCK", "{}: magic", row.name);
    let mut at = 4usize;
    let mut keys: Vec<[u8; 2]> = Vec::new();
    let mut sh_payload: Option<Vec<u8>> = None;
    let mut ei_payload: Option<Vec<u8>> = None;
    let mut st_payload: Option<Vec<u8>> = None;

    while at < stream.len() {
        let key: [u8; 2] = [stream[at], stream[at + 1]];
        assert!(
            key[0].is_ascii_uppercase() && key[1].is_ascii_uppercase(),
            "{}: invalid block key {:?}",
            row.name,
            String::from_utf8_lossy(&key)
        );
        let mut size_at = at + 2;
        let declared = read_byte_varint(stream, &mut size_at);
        assert!(
            declared as usize >= size_at - at,
            "{}: block size smaller than its header",
            row.name
        );
        let end = at + declared as usize;
        assert!(
            end <= stream.len(),
            "{}: block overruns the stream",
            row.name
        );
        let header_len = size_at - at;
        let payload = &stream[at + header_len..end];

        match &key {
            b"SH" => {
                assert!(payload.len() > 4, "{}: SH payload too short", row.name);
                let stored = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let computed = crc32(&payload[4..]);
                assert_eq!(stored, computed, "{}: SH CRC", row.name);
                sh_payload = Some(payload[4..].to_vec());
            }
            b"RG" => {
                assert_eq!(
                    payload,
                    &[1u8, 0, 0, 0, 0, 0, 0, 0, 0],
                    "{}: RG must be the reference zeroed payload",
                    row.name
                );
            }
            b"EI" => ei_payload = Some(payload.to_vec()),
            b"SO" => assert_eq!(payload.len(), 5, "{}: SO placeholder", row.name),
            b"AP" => {}
            b"ST" => st_payload = Some(payload.to_vec()),
            b"SE" => assert!(payload.is_empty(), "{}: SE payload", row.name),
            other => panic!(
                "{}: unexpected block {:?}",
                row.name,
                String::from_utf8_lossy(other)
            ),
        }
        keys.push(key);
        at = end;
    }
    assert_eq!(at, stream.len(), "{}: trailing bytes", row.name);

    // Expected header/footer sequence: SH RG EI SO <AP…> ST SE.
    assert_eq!(
        keys.first().copied(),
        Some(*b"SH"),
        "{}: first block",
        row.name
    );
    assert_eq!(
        keys.last().copied(),
        Some(*b"SE"),
        "{}: last block",
        row.name
    );
    assert_eq!(&keys[1], b"RG", "{}: second block", row.name);
    assert_eq!(&keys[2], b"EI", "{}: third block", row.name);
    assert_eq!(&keys[3], b"SO", "{}: fourth block", row.name);
    let st_at = keys.iter().position(|k| k == b"ST").expect("ST present");
    let ap_count = keys.iter().filter(|k| *k == b"AP").count();
    assert!(ap_count >= 1, "{}: at least one AP block", row.name);
    assert!(
        keys[4..st_at].iter().all(|k| k == b"AP"),
        "{}: only AP blocks between SO and ST",
        row.name
    );
    assert!(st_at >= 5, "{}: ST follows at least one AP", row.name);
    assert_eq!(&keys[st_at - 1], b"AP", "{}: block before ST", row.name);
    assert_eq!(st_at + 1, keys.len() - 1, "{}: ST is penultimate", row.name);
    assert_eq!(&keys[st_at + 1], b"SE", "{}: ST followed by SE", row.name);

    // --- SH fields ---
    let sh = sh_payload.expect("SH seen");
    let mut br = BitReader::new(&sh);
    let version = br.read(8);
    assert_eq!(version, 8, "{}: stream version", row.name);
    let samples = br.read_varint_bytes();
    assert_eq!(samples, row.frames as u64, "{}: SH samples", row.name);
    let skip = br.read_varint_bytes();
    assert_eq!(skip, 0, "{}: SH beg_silence", row.name);
    let rate_index = br.read(3) as usize;
    assert_eq!(
        RATE_INDEX[rate_index], row.rate,
        "{}: SH sample rate",
        row.name
    );
    let max_band = br.read(5) as usize + 1;
    assert_eq!(max_band, expected_max_band, "{}: SH max_band", row.name);
    let channels = br.read(4) as usize + 1;
    assert_eq!(channels as u32, row.channels, "{}: SH channels", row.name);
    let ms = br.read(1);
    assert_eq!(
        ms, 1,
        "{}: SH MS flag (set for every integer quality)",
        row.name
    );
    let block_pwr = br.read(3) * 2;
    assert_eq!(block_pwr, 6, "{}: SH frames-per-block power", row.name);

    // --- EI fields ---
    let ei = ei_payload.expect("EI seen");
    assert_eq!(ei.len(), 4, "{}: EI payload length", row.name);
    let mut br = BitReader::new(&ei);
    let profile = br.read(7) as u64;
    let pns = br.read(1);
    let major = br.read(8);
    let minor = br.read(8);
    let build = br.read(8);
    let expected_profile = ((f64::from(row.quality) + 5.0) * 8.0 + 0.5) as u64;
    assert_eq!(profile, expected_profile, "{}: EI profile", row.name);
    // PNS is enabled by the profile table for qualities 0..=4 only.
    assert_eq!(
        pns,
        u32::from(row.quality <= 4),
        "{}: EI PNS flag",
        row.name
    );
    assert_eq!(
        (major, minor, build),
        (1, 32, 0),
        "{}: EI version",
        row.name
    );

    // --- ST fields ---
    let st = st_payload.expect("ST seen");
    let mut at = 0usize;
    let entries = read_byte_varint(&st, &mut at);
    assert!(entries >= 1, "{}: ST has at least one entry", row.name);
    let seek_pwr = u32::from(st[at] >> 4);
    assert_eq!(seek_pwr, 1, "{}: ST seek power", row.name);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The matrix manifest must cover the complete integer matrix for both signal
/// kinds plus the committed mono rows — this is the coverage gate itself.
#[test]
fn matrix_manifest_covers_the_complete_integer_matrix() {
    let rows = load_manifest();
    for kind in ["noise", "transient"] {
        let pairs: BTreeSet<(u32, u32)> = rows
            .iter()
            .filter(|r| r.kind == kind && r.channels == 2)
            .map(|r| (r.quality, r.rate))
            .collect();
        let expected: BTreeSet<(u32, u32)> = QUALITIES
            .iter()
            .flat_map(|&q| RATES.iter().map(move |&r| (q, r)))
            .collect();
        assert_eq!(
            pairs, expected,
            "the `{kind}` rows must cover all 44 integer (quality, rate) pairs"
        );
    }
    let mono: BTreeSet<u32> = rows
        .iter()
        .filter(|r| r.channels == 1)
        .map(|r| r.rate)
        .collect();
    for rate in RATES {
        assert!(mono.contains(&rate), "mono coverage missing at {rate} Hz");
    }
    assert!(
        rows.iter().all(|r| r.channels == 1 || r.channels == 2),
        "J.1 does not add >2-channel support"
    );
}

/// Every matrix row must reproduce the frozen C reference bytes, and every
/// produced stream must parse as a well-formed SV8 container with the
/// expected header fields for its configuration.
#[test]
fn every_integer_configuration_is_byte_identical_to_the_reference() {
    let rows = load_manifest();
    let mut cases = 0usize;
    let mut bytes_total = 0usize;
    let mut per_config: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut first_divergence: Option<String> = None;

    for row in &rows {
        let pcm = gen_pcm(&row.kind, row.frames, row.channels);
        let config = EncoderConfig::new(row.quality as f32, row.rate, row.channels);
        let mut encoder = MusepackEncoder::new(config)
            .unwrap_or_else(|e| panic!("{}: rejected configuration: {e}", row.name));
        let expected_max_band = encoder.max_band();
        let actual = encoder
            .encode(&pcm)
            .unwrap_or_else(|e| panic!("{}: encode failed: {e}", row.name));

        let expected =
            std::fs::read(data_dir().join(format!("{}.mpc", row.name))).unwrap_or_else(|e| {
                panic!("{}: missing fixture: {e}", row.name);
            });

        // Well-formed, decodable SV8 with the right header fields.
        parse_and_validate_sv8(&actual, row, expected_max_band);
        parse_and_validate_sv8(&expected, row, expected_max_band);

        cases += 1;
        bytes_total += expected.len();
        per_config.insert((row.quality, row.rate));

        if actual != expected {
            let first = actual
                .iter()
                .zip(expected.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
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

    assert_eq!(
        per_config.len(),
        44,
        "differential coverage must include all 44 integer configurations"
    );
    assert!(
        cases >= 97,
        "expected the full matrix manifest, got {cases} rows"
    );
    if let Some(msg) = first_divergence {
        panic!("matrix mismatch: {msg}");
    }
    eprintln!(
        "integer matrix: {cases} rows, {bytes_total} bytes, \
         {} distinct (quality, rate) configurations byte-identical",
        per_config.len()
    );
}
