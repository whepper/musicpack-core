//! Conformance tests for the similarity document v1 format spike.
//!
//! These tests are the executable half of `FORMAT_SPEC.md`. They exist to make
//! four classes of claim checkable rather than rhetorical:
//!
//! 1. **Determinism** — the same logical document serializes to the same bytes,
//!    every time, and to the committed bytes.
//! 2. **Validation** — each committed invalid fixture fails with the exact code
//!    the specification names, and each valid one passes.
//! 3. **Documentation agreement** — `fixtures/similarity-doc/MANIFEST.txt`
//!    describes the committed bytes: same names, same digests, same sizes, same
//!    results, and no file in the directory that the manifest does not mention.
//! 4. **Containment** — no fixture and no line of the manifest carries a local
//!    path, a username, or any text at all beyond the four magic bytes.
//!
//! No model, audio, library, or database is involved, and none may be: the whole
//! point of ADR 0016 §17 Slice 1 is that the format is reviewable without one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::docfixtures::{self, MANIFEST_NAME};
use crate::docfmt::{
    self, Admission, Document, ENCODING_F16LE, ENCODING_F32LE, ENTRY_BYTES, HEADER_BYTES, Member,
    STATUS_FAILED, STATUS_INSUFFICIENT_AUDIO, STATUS_OK, STATUS_UNSUPPORTED, f16_bits_to_f32,
    f32_to_f16_bits,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(docfixtures::FIXTURE_DIRECTORY)
}

fn manifest_text() -> String {
    std::fs::read_to_string(fixture_dir().join(MANIFEST_NAME)).expect("committed fixture manifest")
}

/// One manifest record: a fixture name plus its `key=value` lines.
struct Record {
    name: String,
    fields: BTreeMap<String, String>,
}

impl Record {
    fn get(&self, key: &str) -> &str {
        self.fields
            .get(key)
            .unwrap_or_else(|| panic!("{}: missing field {key}", self.name))
    }
}

fn parse_manifest(text: &str) -> Vec<Record> {
    let mut records: Vec<Record> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            records.push(Record {
                name: line.trim().to_string(),
                fields: BTreeMap::new(),
            });
            continue;
        }
        let record = records
            .last_mut()
            .expect("an indented line always follows a record name");
        let (key, value) = line
            .trim()
            .split_once('=')
            .unwrap_or_else(|| panic!("{}: malformed field line {line:?}", record.name));
        record.fields.insert(key.to_string(), value.to_string());
    }
    records
}

fn committed(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name))
        .unwrap_or_else(|error| panic!("cannot read committed fixture {name}: {error}"))
}

// ---------------------------------------------------------------------------
// 1. The committed set is exactly the set
// ---------------------------------------------------------------------------

#[test]
fn committed_fixtures_match_the_reference_set_byte_for_byte() {
    for fixture in docfixtures::fixtures() {
        let path = fixture_dir().join(&fixture.name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("missing committed fixture {}: {error}", fixture.name));
        assert_eq!(
            bytes, fixture.bytes,
            "{} drifted from the reference encoder; re-run `docfmt emit` and review the diff",
            fixture.name
        );
    }
}

#[test]
fn no_orphan_files_in_the_fixture_directory() {
    let mut on_disk: Vec<String> = std::fs::read_dir(fixture_dir())
        .expect("fixture directory exists")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(docfixtures::FIXTURE_EXTENSION))
        .collect();
    on_disk.sort();
    let mut expected: Vec<String> = docfixtures::fixtures()
        .into_iter()
        .map(|fixture| fixture.name)
        .collect();
    expected.sort();
    assert_eq!(
        on_disk, expected,
        "every committed fixture must be produced by the reference encoder"
    );
}

// ---------------------------------------------------------------------------
// 2. Determinism
// ---------------------------------------------------------------------------

#[test]
fn the_same_logical_document_encodes_to_identical_bytes() {
    // Two independently constructed values, not one value encoded twice: the
    // property under test is that construction order and allocation identity
    // cannot leak into the output.
    let build = || Document {
        major: docfmt::FORMAT_MAJOR,
        minor: docfmt::FORMAT_MINOR,
        fingerprint: docfixtures::fingerprint_a(),
        dimensions: 4,
        encoding: ENCODING_F32LE,
        flags: 0,
        members: vec![
            Member {
                disc: 1,
                track: 1,
                status: STATUS_OK,
                vector: Some(vec![0.0, 0.0, 0.0, 1.0]),
            },
            Member {
                disc: 1,
                track: 2,
                status: STATUS_OK,
                vector: Some(vec![0.5, 0.5, 0.5, 0.5]),
            },
        ],
    };
    let first = docfmt::encode(&build()).expect("encodes");
    let second = docfmt::encode(&build()).expect("encodes");
    assert_eq!(first, second, "serialization is not deterministic");
    assert_eq!(
        first,
        committed("determinism-ok.msim"),
        "the committed determinism fixture is not what the encoder produces"
    );
    assert_eq!(
        docfmt::sha256_hex(&first),
        docfmt::sha256_hex(&committed("determinism-ok.msim")),
        "digest drift"
    );
}

#[test]
fn every_valid_fixture_round_trips_through_the_reader_and_writer() {
    for name in ["minimal-ok.msim", "multi-track.msim", "f16-encoding.msim"] {
        let bytes = committed(name);
        let document = docfmt::validate(&bytes)
            .unwrap_or_else(|error| panic!("{name} should be valid, got {error}"));
        assert_eq!(
            docfmt::encode(&document).expect("re-encodes"),
            bytes,
            "{name} does not round-trip"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Validation rules
// ---------------------------------------------------------------------------

#[test]
fn each_invalid_fixture_fails_with_the_specified_code() {
    // (fixture, code) — one per validation rule in FORMAT_SPEC.md §10.2.
    let expected: &[(&str, &str)] = &[
        ("bad-magic.msim", "magic_invalid"),
        ("unsupported-major.msim", "unsupported_version"),
        ("unsupported-minor.msim", "unsupported_version"),
        ("zero-fingerprint.msim", "zero_fingerprint"),
        ("dimensions-zero.msim", "dimensions_zero"),
        ("dimensions-too-large.msim", "dimensions_too_large"),
        ("unsupported-encoding.msim", "unsupported_encoding"),
        ("reserved-flag-set.msim", "reserved_nonzero"),
        ("header-reserved-nonzero.msim", "reserved_nonzero"),
        ("entry-reserved-nonzero.msim", "reserved_nonzero"),
        ("track-count-zero.msim", "track_count_zero"),
        ("track-count-too-large.msim", "track_count_too_large"),
        ("unknown-status.msim", "unknown_status"),
        ("invalid-member.msim", "invalid_member"),
        ("unsorted-table.msim", "unsorted_table"),
        ("duplicate-member.msim", "duplicate_member"),
        ("truncated-header.msim", "truncated_header"),
        ("truncated-table.msim", "truncated_table"),
        ("truncated.msim", "truncated"),
        ("trailing-bytes.msim", "trailing_bytes"),
        ("non-finite-vector.msim", "non_finite_vector"),
    ];
    for (name, code) in expected {
        let bytes = committed(name);
        let error = docfmt::validate(&bytes)
            .err()
            .unwrap_or_else(|| panic!("{name} should be invalid"));
        assert_eq!(error.code, *code, "{name}: wrong code ({error})");
    }
    // Every rule the specification lists has a fixture.
    let covered: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();
    let mut all: Vec<String> = docfixtures::fixtures()
        .into_iter()
        .map(|fixture| fixture.name)
        .filter(|name| {
            !matches!(
                name.as_str(),
                "minimal-ok.msim"
                    | "multi-track.msim"
                    | "determinism-ok.msim"
                    | "f16-encoding.msim"
                    | "all-zero-vector.msim"
            )
        })
        .collect();
    all.sort();
    let mut sorted = covered.clone();
    sorted.sort();
    assert_eq!(
        sorted, all,
        "an invalid fixture is not covered by this test"
    );
}

#[test]
fn the_reserved_album_aggregate_flag_is_rejected_not_ignored() {
    let error = docfmt::validate(&committed("reserved-flag-set.msim")).expect_err("rejected");
    assert_eq!(error.code, "reserved_nonzero");
    assert!(
        error.detail.contains("flags"),
        "the report should name the field: {error}"
    );
    assert_eq!(
        docfmt::FLAG_RESERVED_ALBUM_AGGREGATE,
        1,
        "the reserved bit is bit 0, per the specification"
    );
}

#[test]
fn validation_reports_the_first_failure_in_the_normative_order() {
    // A document that is wrong in four ways at once still has one answer, and
    // that answer is the first check in FORMAT_SPEC.md §10.1.
    let mut bytes = committed("track-count-too-large.msim");
    bytes[2] = b'X'; // magic
    bytes[5] = 0x00; // major -> 0
    bytes[44] = 0x00; // track_count -> 0
    assert_eq!(
        docfmt::validate(&bytes).expect_err("invalid").code,
        "magic_invalid"
    );

    let mut bytes = committed("track-count-too-large.msim");
    bytes[5] = 0x00;
    assert_eq!(
        docfmt::validate(&bytes).expect_err("invalid").code,
        "unsupported_version"
    );
}

#[test]
fn a_writer_cannot_claim_success_without_a_vector_or_a_failure_with_one() {
    // The two error classes that make a fabricated zero vector impossible. They
    // are unrepresentable in bytes, so they are only reachable through the
    // writer's own validation.
    let base = |status, vector| Document {
        major: docfmt::FORMAT_MAJOR,
        minor: docfmt::FORMAT_MINOR,
        fingerprint: docfixtures::fingerprint_a(),
        dimensions: 4,
        encoding: ENCODING_F32LE,
        flags: 0,
        members: vec![Member {
            disc: 1,
            track: 1,
            status,
            vector,
        }],
    };
    assert_eq!(
        docfmt::encode(&base(STATUS_OK, None))
            .expect_err("rejected")
            .code,
        "vector_missing_for_ok"
    );
    for status in [STATUS_INSUFFICIENT_AUDIO, STATUS_UNSUPPORTED, STATUS_FAILED] {
        assert_eq!(
            docfmt::encode(&base(status, Some(vec![0.0; 4])))
                .expect_err("rejected")
                .code,
            "vector_present_for_non_ok"
        );
    }
    // And a non-finite value is refused at write time too, not just at read time.
    assert_eq!(
        docfmt::encode(&base(STATUS_OK, Some(vec![f32::NAN, 0.0, 0.0, 0.0])))
            .expect_err("rejected")
            .code,
        "non_finite_vector"
    );
    // An f32 that is finite but not representable in binary16 is a distinct
    // failure: silently producing an infinity would be the bug.
    let mut half = base(STATUS_OK, Some(vec![1.0e30, 0.0, 0.0, 0.0]));
    half.encoding = ENCODING_F16LE;
    assert_eq!(
        docfmt::encode(&half).expect_err("rejected").code,
        "vector_element_not_representable"
    );
}

#[test]
fn status_carries_the_meaning_and_the_sizes_follow_from_it() {
    let bytes = committed("multi-track.msim");
    let document = docfmt::validate(&bytes).expect("valid");
    let statuses: Vec<u8> = document.members.iter().map(|m| m.status).collect();
    assert_eq!(
        statuses,
        vec![
            STATUS_OK,
            STATUS_INSUFFICIENT_AUDIO,
            STATUS_UNSUPPORTED,
            STATUS_FAILED
        ]
    );
    assert_eq!(document.contributors().count(), 1, "exactly one vector");
    assert_eq!(
        document.total_bytes(),
        HEADER_BYTES + 4 * ENTRY_BYTES + 4 * 4,
        "a non-vector member contributes no bytes at all"
    );
    assert_eq!(bytes.len(), document.total_bytes());
    // Every non-ok member has no vector, structurally.
    for member in &document.members {
        assert_eq!(
            member.vector.is_some(),
            member.status == STATUS_OK,
            "vector presence must follow status exactly"
        );
    }
}

#[test]
fn the_all_zero_vector_is_format_valid_and_only_a_consumer_can_judge_it() {
    // The trap fixture. An all-zero vector is four finite values, so the format
    // accepts it; what makes it detectable is that the profile declares L2
    // normalization and the consumer checks the norm.
    let document = docfmt::validate(&committed("all-zero-vector.msim")).expect("valid");
    let vector = document.members[0].vector.as_ref().expect("a vector");
    assert!(vector.iter().all(|value| *value == 0.0));
    let norm = vector
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    assert_eq!(norm, 0.0, "a consumer's L2 check would catch this");
    // And it is still a lie about membership: the status says "ok".
    assert_eq!(document.members[0].status, STATUS_OK);
}

#[test]
fn vector_elements_are_addressable_without_any_offset_field() {
    let document = docfmt::validate(&committed("multi-track.msim")).expect("valid");
    assert_eq!(document.vector_offset(0), HEADER_BYTES + 4 * ENTRY_BYTES);
    // A non-ok member reserves no space, so member 3's slot would be the fourth
    // element's slot; index arithmetic stays trivial.
    assert_eq!(
        document.vector_offset(1),
        HEADER_BYTES + 4 * ENTRY_BYTES + 16
    );
}

// ---------------------------------------------------------------------------
// 4. Profile identity
// ---------------------------------------------------------------------------

#[test]
fn a_different_output_encoding_is_a_different_profile() {
    let a = docfixtures::fingerprint_a();
    let b = docfixtures::fingerprint_b();
    assert_ne!(
        a, b,
        "encoding is a fingerprint field, so it must change identity"
    );
    let f32_document = docfmt::validate(&committed("minimal-ok.msim")).expect("valid");
    let f16_document = docfmt::validate(&committed("f16-encoding.msim")).expect("valid");
    assert_eq!(f32_document.fingerprint, a);
    assert_eq!(f16_document.fingerprint, b);
    // Admission is a byte comparison against what the consumer is serving.
    assert_eq!(docfmt::admit(&f32_document, &a), Admission::Admitted);
    assert_eq!(docfmt::admit(&f32_document, &b), Admission::ProfileMismatch);
    assert_eq!(docfmt::admit(&f16_document, &b), Admission::Admitted);
    assert_eq!(docfmt::admit(&f16_document, &a), Admission::ProfileMismatch);
}

#[test]
fn profile_identity_is_recomputable_from_its_documented_fields() {
    // Two independent invocations of the same field list must agree, and the
    // manifest's recorded digests must match the fields documented in
    // FORMAT_SPEC.md appendix A.
    assert_eq!(
        docfmt::profile_fingerprint(&docfixtures::profile_a()),
        docfixtures::fingerprint_a()
    );
    let tlv = docfmt::profile_tlv(&docfixtures::profile_a());
    assert_eq!(
        tlv[0], 1,
        "tags are emitted in ascending order, starting at 1"
    );
    // The absent fields emit nothing: a model-free profile has no weights hash
    // (tag 4) and no runtime (tag 13).
    let mut tags = Vec::new();
    let mut at = 0usize;
    while at + 5 <= tlv.len() {
        let length =
            u32::from_be_bytes([tlv[at + 1], tlv[at + 2], tlv[at + 3], tlv[at + 4]]) as usize;
        tags.push(tlv[at]);
        at += 5 + length;
    }
    assert_eq!(at, tlv.len(), "the TLV is a well-formed sequence");
    assert_eq!(tags, vec![1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 14]);
    let text = manifest_text();
    for (label, fingerprint) in [
        ("profile A", docfixtures::fingerprint_a()),
        ("profile B", docfixtures::fingerprint_b()),
    ] {
        let hex = docfmt::fingerprint_hex(&fingerprint);
        assert!(
            text.contains(&hex),
            "the manifest must record {label}'s fingerprint {hex}"
        );
    }
}

#[test]
fn dimensions_are_a_profile_value_not_a_format_value() {
    // A future profile with a different dimension count needs no format change:
    // the count is a header field, and the table and vector region follow from
    // it arithmetically.
    let mut document = docfmt::validate(&committed("minimal-ok.msim")).expect("valid");
    document.dimensions = 8;
    document.members[0].vector = Some(vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let bytes = docfmt::encode(&document).expect("encodes");
    assert_eq!(bytes.len(), HEADER_BYTES + ENTRY_BYTES + 8 * 4);
    let parsed = docfmt::validate(&bytes).expect("valid");
    assert_eq!(parsed.dimensions, 8);
    assert_eq!(parsed.members[0].vector.as_ref().map(Vec::len), Some(8));
}

// ---------------------------------------------------------------------------
// 5. binary16
// ---------------------------------------------------------------------------

#[test]
fn binary16_bit_patterns_are_the_standard_ones() {
    for (value, expected) in [
        (0.0f32, 0x0000u16),
        (-0.0f32, 0x8000),
        (1.0, 0x3c00),
        (-1.0, 0xbc00),
        (0.5, 0x3800),
        (2.0, 0x4000),
        (65504.0, 0x7bff), // largest finite binary16
    ] {
        assert_eq!(
            f32_to_f16_bits(value),
            Some(expected),
            "{value} should encode to {expected:#06x}"
        );
        assert_eq!(f16_bits_to_f32(expected), value, "widen {expected:#06x}");
    }
    // Not representable: non-finite input, and overflow to infinity.
    assert_eq!(f32_to_f16_bits(f32::INFINITY), None);
    assert_eq!(f32_to_f16_bits(f32::NAN), None);
    assert_eq!(f32_to_f16_bits(65_536.0), None); // 2^16 > largest finite
    assert_eq!(f32_to_f16_bits(65_504.0), Some(0x7bff)); // rounds down, stays finite
    // Subnormals: the smallest positive binary16, and a value that rounds to it.
    assert_eq!(f32_to_f16_bits(2.0f32.powi(-24)), Some(0x0001));
    // Exactly half of the smallest subnormal: a tie, and 0 is the even neighbour.
    assert_eq!(f32_to_f16_bits(2.0f32.powi(-25)), Some(0x0000));
    // 1.5 * 2^-24 is a tie between k=1 and k=2; 2 is even, so it wins.
    assert_eq!(f32_to_f16_bits(3.0 * 2.0f32.powi(-25)), Some(0x0002));
    assert_eq!(f32_to_f16_bits(2.0f32.powi(-14)), Some(0x0400)); // smallest normal
}

#[test]
fn binary16_rounding_is_to_nearest_with_ties_to_even() {
    // 1 + 2^-11 sits exactly between two representable values.
    let tie_up = 1.0f32 + 2.0f32.powi(-11);
    let tie_down = 1.0f32 + 3.0 * 2.0f32.powi(-12);
    assert_eq!(f32_to_f16_bits(tie_up), Some(0x3c00)); // 1.0 is even
    assert_eq!(f32_to_f16_bits(tie_down), Some(0x3c01));
    // Across [0.5, 1) the spacing is 2^-11, so no rounding may exceed half of it.
    let half_ulp = 2.0f64.powi(-12);
    for step in 0..64 {
        let value = 0.5f32 + step as f32 * 0.003;
        let bits = f32_to_f16_bits(value).expect("finite");
        let widened = f64::from(f16_bits_to_f32(bits));
        assert!(
            (widened - f64::from(value)).abs() <= half_ulp,
            "{value} rounded to {widened}, more than half an ulp away"
        );
    }
}

#[test]
fn the_f16_fixture_is_bit_exact_rather_than_approximately_right() {
    let f32_document = docfmt::validate(&committed("determinism-ok.msim")).expect("valid");
    let f16_document = docfmt::validate(&committed("f16-encoding.msim")).expect("valid");
    assert_eq!(f16_document.dimensions, f32_document.dimensions);
    assert_eq!(f16_document.members.len(), f32_document.members.len());
    for (left, right) in f32_document.members.iter().zip(f16_document.members.iter()) {
        assert_eq!(
            (left.disc, left.track, left.status),
            (right.disc, right.track, right.status)
        );
        if left.status == STATUS_OK {
            assert_eq!(
                left.vector, right.vector,
                "the f16 fixture is only useful if its values survive the narrowing exactly"
            );
        }
    }
    // Two vectors of four values each: [0, 0, 0, 1] then [0.5, 0.5, 0.5, 0.5].
    let first = HEADER_BYTES + 2 * ENTRY_BYTES;
    let second = first + 8;
    let bytes = committed("f16-encoding.msim");
    assert_eq!(
        &bytes[first + 6..first + 8],
        &[0x00, 0x3c],
        "1.0 as binary16"
    );
    assert_eq!(&bytes[second..second + 2], &[0x00, 0x38], "0.5 as binary16");
    assert_eq!(bytes.len(), first + 16);
}

// ---------------------------------------------------------------------------
// 6. Package coherence
// ---------------------------------------------------------------------------

struct Package {
    pairs: Vec<(u32, u32)>,
}

impl docfmt::ManifestTracks for Package {
    fn pairs(&self) -> Vec<(u32, u32)> {
        self.pairs.clone()
    }
}

#[test]
fn coherence_with_the_package_is_separate_from_document_validity() {
    let document = docfmt::validate(&committed("minimal-ok.msim")).expect("valid");
    let matching = Package {
        pairs: vec![(1, 1)],
    };
    assert_eq!(docfmt::validate_coherence(&document, &matching), Ok(()));

    let missing = Package {
        pairs: vec![(1, 1), (1, 2)],
    };
    assert_eq!(
        docfmt::validate_coherence(&document, &missing)
            .expect_err("mismatch")
            .code,
        "member_count_mismatch"
    );

    let same_count_wrong_track = Package {
        pairs: vec![(1, 2)],
    };
    assert_eq!(
        docfmt::validate_coherence(&document, &same_count_wrong_track)
            .expect_err("mismatch")
            .code,
        "unknown_member"
    );
    // The document is still structurally valid in every one of those cases: a
    // coherence failure is about which package this document belongs to.
    assert!(docfmt::validate(&committed("minimal-ok.msim")).is_ok());
}

// ---------------------------------------------------------------------------
// 7. The manifest describes the committed bytes
// ---------------------------------------------------------------------------

#[test]
fn the_manifest_agrees_with_the_committed_bytes() {
    let records = parse_manifest(&manifest_text());
    let expected: Vec<String> = docfixtures::fixtures()
        .into_iter()
        .map(|fixture| fixture.name)
        .collect();
    let mut actual: Vec<String> = records.iter().map(|record| record.name.clone()).collect();
    let mut sorted_expected = expected.clone();
    sorted_expected.sort();
    actual.sort();
    assert_eq!(
        actual, sorted_expected,
        "the manifest and the directory disagree"
    );

    for (fixture, record) in docfixtures::fixtures().iter().zip(records.iter()) {
        assert_eq!(record.name, fixture.name);
        assert_eq!(
            record.get("bytes").parse::<usize>().expect("bytes"),
            fixture.bytes.len()
        );
        assert_eq!(record.get("sha256"), docfmt::sha256_hex(&fixture.bytes));
        assert_eq!(
            record.get("purpose"),
            fixture.purpose,
            "{}: purpose text drifted",
            fixture.name
        );
        // The recorded fingerprint must be what the bytes say, rendered as hex
        // and nothing else. (A fingerprint is already a digest; hex-encoding it a
        // second time is a silent, plausible-looking bug.)
        let mut from_bytes = [0u8; 32];
        if fixture.bytes.len() >= HEADER_BYTES {
            from_bytes.copy_from_slice(&fixture.bytes[8..40]);
        }
        let expected_fingerprint = if fixture.bytes.len() >= HEADER_BYTES {
            docfmt::fingerprint_hex(&from_bytes)
        } else {
            "absent".to_string()
        };
        assert_eq!(
            record.get("profile_fingerprint"),
            expected_fingerprint,
            "{}: the manifest's fingerprint is not the one in the bytes",
            fixture.name
        );
        let result = record.get("result");
        match docfmt::validate(&fixture.bytes) {
            Ok(_) => assert_eq!(result, "valid", "{}: expected a valid result", fixture.name),
            Err(error) => {
                let expected = format!("invalid code={} detail={}", error.code, error.detail);
                assert_eq!(result, expected, "{}: wrong result line", fixture.name);
            }
        }
        // The package effect is a documented constant, and it is the whole point
        // of ADR 0017 §5.3: a document finding never invalidates a package.
        assert_eq!(
            record.get("package_effect"),
            "none",
            "{}: a document finding must not affect package validity",
            fixture.name
        );
    }
}

#[test]
fn the_manifest_lists_every_member_of_every_valid_fixture() {
    for record in parse_manifest(&manifest_text()) {
        if record.get("result") != "valid" {
            continue;
        }
        let bytes = committed(&record.name);
        let document = docfmt::validate(&bytes).expect("valid");
        for (index, member) in document.members.iter().enumerate() {
            let line = record
                .fields
                .get(&format!("entry{index}"))
                .unwrap_or_else(|| panic!("{}: no entry{index} line", record.name));
            let status = docfmt::status_name(member.status).expect("defined");
            assert!(
                line.contains(&format!("disc={} ", member.disc)),
                "{}: entry{index} disc mismatch: {line}",
                record.name
            );
            assert!(
                line.contains(&format!("track={} ", member.track)),
                "{}: entry{index} track mismatch: {line}",
                record.name
            );
            assert!(
                line.contains(&format!("status={status} ")),
                "{}: entry{index} status mismatch: {line}",
                record.name
            );
            if member.status == STATUS_OK {
                assert!(line.contains("vector=["));
            } else {
                assert!(line.contains("vector=none"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 8. Containment: no local data in the fixtures
// ---------------------------------------------------------------------------

#[test]
fn no_fixture_can_carry_a_token_of_four_characters_or_more() {
    // A similarity document has no string field of any kind, so there is nowhere
    // to put a name, a path, a user or a library id. This is the tripwire for
    // that claim: the only run of four or more readable bytes in any fixture is
    // the four-byte magic at offset 0. (Isolated readable bytes do occur — they
    // are digest and float payload bytes — so the threshold is a run, not a
    // byte.)
    for fixture in docfixtures::fixtures() {
        let mut long_runs: Vec<(usize, usize)> = Vec::new();
        let mut start = 0usize;
        let mut run = 0usize;
        for (offset, byte) in fixture.bytes.iter().enumerate() {
            if byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_' {
                if run == 0 {
                    start = offset;
                }
                run += 1;
            } else {
                if run >= 4 {
                    long_runs.push((start, run));
                }
                run = 0;
            }
        }
        if run >= 4 {
            long_runs.push((start, run));
        }
        assert_eq!(
            long_runs,
            vec![(0, 4)],
            "{}: readable runs other than the magic: {long_runs:?}",
            fixture.name
        );
        // Exactly one fixture departs from the magic, and it is the one whose
        // purpose is to prove the magic is checked.
        let expected: &[u8; 4] = if fixture.name == "bad-magic.msim" {
            b"MSXM"
        } else {
            b"MSIM"
        };
        assert_eq!(&fixture.bytes[0..4], *expected, "{}", fixture.name);
    }
}

#[test]
fn the_manifest_contains_no_local_path_or_username() {
    let text = manifest_text();
    for forbidden in ["/Users/", "/home/", "\\Users\\", "C:\\", "file://", "~/"] {
        assert!(
            !text.contains(forbidden),
            "the manifest mentions {forbidden:?}"
        );
    }
    for (number, line) in text.lines().enumerate() {
        assert!(
            !line.contains('\\'),
            "manifest line {} looks like a Windows path: {line}",
            number + 1
        );
    }
}

#[test]
fn the_dump_renders_every_fixture_without_panicking() {
    for fixture in docfixtures::fixtures() {
        let text = docfixtures::dump(&fixture.name, &fixture.bytes);
        assert!(text.contains(&format!("{} bytes", fixture.bytes.len())));
        assert!(text.contains(&docfmt::sha256_hex(&fixture.bytes)));
    }
}

#[test]
fn the_specification_lists_every_fixture_with_its_committed_digest() {
    // The digest table in FORMAT_SPEC.md §16 is what a reviewer reads. It is
    // generated from the same bytes, and this test is what keeps it honest.
    let spec =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("FORMAT_SPEC.md"))
            .expect("FORMAT_SPEC.md");
    for fixture in docfixtures::fixtures() {
        let digest = docfmt::sha256_hex(&fixture.bytes);
        let row = format!(
            "| `{}` | {} | `{digest}` |",
            fixture.name,
            fixture.bytes.len()
        );
        assert!(
            spec.contains(&row),
            "FORMAT_SPEC.md §16 is missing or has stale values for {}\n  expected a row like:\n  {row}",
            fixture.name
        );
    }
    for (label, fingerprint) in [
        (
            "musicpack-similarity-fixture-v1",
            docfixtures::fingerprint_a(),
        ),
        (
            "musicpack-similarity-fixture-f16-v1",
            docfixtures::fingerprint_b(),
        ),
    ] {
        assert!(
            spec.contains(&format!(
                "| `{label}` | `{}` |",
                docfmt::fingerprint_hex(&fingerprint)
            )),
            "FORMAT_SPEC.md must record {label}'s fingerprint"
        );
    }
}
