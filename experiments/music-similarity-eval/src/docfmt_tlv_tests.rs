use std::path::PathBuf;

use crate::docfixtures;
use crate::docfmt;

// ---------------------------------------------------------------------------
// 4b. The canonical profile-fingerprint encoding
//
// The fingerprint is the comparison and partition key, so two independent
// implementations must be able to derive the same 32 bytes from the same
// logical profile. The rules below are normative (FORMAT_SPEC.md appendix A,
// ADR 0017 §5.4). The two shipped fixture profiles leave tags 4, 12 and 13
// absent, so the coverage profile below exists to pin the sub-encodings they do
// not reach.
// ---------------------------------------------------------------------------

/// Every field populated, with one-byte values wherever the encoding allows, so
/// the expected bytes can be read straight off this function and checked
/// against the field table by eye.
fn fully_populated_profile() -> docfmt::ProfileFields {
    docfmt::ProfileFields {
        profile_id: "t".to_string(),    // 0x74
        model_family: "f".to_string(),  // 0x66
        model_variant: "v".to_string(), // 0x76
        model_sha256: Some([0xab; 32]),
        preprocessing_version: "p".to_string(), // 0x70
        patch_hop: 1,
        pooling: "m".to_string(),       // 0x6d
        normalization: "n".to_string(), // 0x6e
        metric: "c".to_string(),        // 0x63
        dimensions: 2,
        output_encoding: "e".to_string(),         // 0x65
        album_aggregation: Some("a".to_string()), // 0x61
        runtime_id: Some("r".to_string()),
        runtime_version: Some("s".to_string()),
        numeric_policy: "z".to_string(), // 0x7a
    }
}

/// The documented framing, written out longhand from FORMAT_SPEC.md appendix A
/// rather than reusing `docfmt`, so that it is a genuine second implementation
/// and a disagreement is a real finding rather than a tautology.
fn tlv_from_documented_rules(fields: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, value) in fields {
        out.push(*tag);
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
    }
    out
}

fn field_value(tlv: &[u8], wanted: u8) -> Vec<u8> {
    let at = position_of_tag(tlv, wanted);
    field_value_at(tlv, at)
}

fn field_value_at(tlv: &[u8], at: usize) -> Vec<u8> {
    let length = u32::from_be_bytes([tlv[at + 1], tlv[at + 2], tlv[at + 3], tlv[at + 4]]) as usize;
    tlv[at + 5..at + 5 + length].to_vec()
}

fn position_of_tag(tlv: &[u8], wanted: u8) -> usize {
    let mut at = 0usize;
    while at + 5 <= tlv.len() {
        if tlv[at] == wanted {
            return at;
        }
        let length =
            u32::from_be_bytes([tlv[at + 1], tlv[at + 2], tlv[at + 3], tlv[at + 4]]) as usize;
        at += 5 + length;
    }
    panic!("tag {wanted} is not present");
}

fn tags_of(tlv: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 5 <= tlv.len() {
        out.push(tlv[at]);
        let length =
            u32::from_be_bytes([tlv[at + 1], tlv[at + 2], tlv[at + 3], tlv[at + 4]]) as usize;
        at += 5 + length;
    }
    out
}

fn consumed(tlv: &[u8]) -> usize {
    let mut at = 0usize;
    while at + 5 <= tlv.len() {
        let length =
            u32::from_be_bytes([tlv[at + 1], tlv[at + 2], tlv[at + 3], tlv[at + 4]]) as usize;
        at += 5 + length;
    }
    at
}

#[test]
fn the_canonical_tlv_sub_encoding_rules_are_normative_and_pinned() {
    let tlv = docfmt::profile_tlv(&fully_populated_profile());

    // The whole encoding, written from the documented field table.
    let expected = tlv_from_documented_rules(&[
        (1, b"t".to_vec()),
        (2, b"f".to_vec()),
        (3, b"v".to_vec()),
        (4, vec![0xab; 32]),
        (5, b"p".to_vec()),
        (6, 1u32.to_be_bytes().to_vec()),
        (7, b"m".to_vec()),
        (8, b"n".to_vec()),
        (9, b"c".to_vec()),
        (10, 2u32.to_be_bytes().to_vec()),
        (11, b"e".to_vec()),
        (12, b"a".to_vec()),
        (13, b"r\0s".to_vec()),
        (14, b"z".to_vec()),
    ]);
    assert_eq!(
        tlv, expected,
        "the canonical encoding must match the documented rules exactly"
    );

    // Rule: an integer value is exactly four bytes, big-endian.
    assert_eq!(field_value(&tlv, 6), vec![0x00, 0x00, 0x00, 0x01]);
    assert_eq!(field_value(&tlv, 10), vec![0x00, 0x00, 0x00, 0x02]);

    // Rule: a digest field is its raw bytes, length carried by the framing.
    assert_eq!(field_value(&tlv, 4), vec![0xab; 32]);
    let at = position_of_tag(&tlv, 4);
    assert_eq!(
        &tlv[at + 1..at + 5],
        &[0x00, 0x00, 0x00, 0x20],
        "the length prefix is four big-endian bytes"
    );

    // Rule: tag 13 is the runtime id, one NUL, then the version.
    let runtime = field_value(&tlv, 13);
    assert_eq!(runtime, b"r\0s".to_vec(), "tag 13 is id NUL version");
    assert_eq!(
        runtime.iter().filter(|byte| **byte == 0).count(),
        1,
        "exactly one NUL separator"
    );

    // Rule: tags ascend and the buffer is consumed exactly.
    assert_eq!(tags_of(&tlv), (1u8..=14).collect::<Vec<u8>>());
    assert_eq!(consumed(&tlv), tlv.len(), "no trailing bytes");

    // And the fingerprint is the digest of exactly these bytes.
    assert_eq!(
        docfmt::fingerprint_hex(&docfmt::profile_fingerprint(&fully_populated_profile())),
        docfmt::sha256_hex(&expected)
    );
}

#[test]
fn an_absent_field_emits_nothing_at_all() {
    // A present-but-empty field is a different thing from an absent one, exactly
    // as in the identity TLV precedent (`src/identity.rs`), so absence must
    // produce no bytes rather than a zero length.
    let tlv = docfmt::profile_tlv(&docfixtures::profile_a());
    for absent in [4u8, 12, 13] {
        assert!(
            !tags_of(&tlv).contains(&absent),
            "tag {absent} must not be emitted when the field is absent"
        );
    }
    assert_eq!(tags_of(&tlv), vec![1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 14]);
}

#[test]
fn a_single_field_change_changes_the_fingerprint_and_nothing_else() {
    // The property the fingerprint exists for: one changed field, one changed
    // fingerprint, and no silent coexistence of two partitions.
    let base = docfixtures::profile_a();
    let mut other = base.clone();
    other.runtime_id = Some("x".to_string());
    other.runtime_version = Some("y".to_string());
    assert_ne!(
        docfmt::profile_fingerprint(&base),
        docfmt::profile_fingerprint(&other),
        "adding a runtime must change the fingerprint"
    );
    // Tags are emitted in ascending order, so tag 13 is *inserted* before
    // tag 14 rather than appended. Removing that one field must reproduce the
    // original encoding byte for byte, which is the precise statement that no
    // other field was disturbed.
    let a = docfmt::profile_tlv(&base);
    let b = docfmt::profile_tlv(&other);
    let at = position_of_tag(&b, 13);
    let inserted = 5 + field_value_at(&b, at).len();
    let mut without_tag_13 = b.clone();
    without_tag_13.drain(at..at + inserted);
    assert_eq!(
        without_tag_13, a,
        "removing tag 13 must leave the original encoding untouched"
    );
    assert_eq!(field_value_at(&b, at), b"x\0y".to_vec());
}

#[test]
fn the_fixture_profile_encodings_are_pinned_byte_for_byte() {
    // Golden vectors. Both lines are reproducible from the field table in
    // FORMAT_SPEC.md appendix A; `docfmt profiles` prints the same hex, and the
    // fingerprints are recorded in FORMAT_SPEC.md §16 and the fixture manifest.
    let a = docfmt::hex_encode(&docfmt::profile_tlv(&docfixtures::profile_a()));
    let b = docfmt::hex_encode(&docfmt::profile_tlv(&docfixtures::profile_b()));
    assert_eq!(a.len(), 382, "191 bytes, hex encoded");
    assert_eq!(b.len(), 390, "195 bytes, hex encoded");
    assert!(a.starts_with("010000001f6d757369637061636b2d73696d696c61726974792d6669787475"));
    assert!(
        a.ends_with("0b000000056633326c650e000000157363616c61722d6e6f2d636f6e7472616374696f6e")
    );
    assert!(b.starts_with("01000000236d757369637061636b2d73696d696c61726974792d6669787475"));
    assert!(
        b.ends_with("0b000000056631366c650e000000157363616c61722d6e6f2d636f6e7472616374696f6e")
    );
    // Only tag 1 (the name) and tag 11 (the encoding) differ between the two
    // fixture profiles, and the fingerprints differ because of it.
    let a_bytes = docfmt::profile_tlv(&docfixtures::profile_a());
    let b_bytes = docfmt::profile_tlv(&docfixtures::profile_b());
    assert_eq!(field_value(&a_bytes, 11), b"f32le".to_vec());
    assert_eq!(field_value(&b_bytes, 11), b"f16le".to_vec());
    assert_eq!(
        field_value(&a_bytes, 1),
        b"musicpack-similarity-fixture-v1".to_vec()
    );
    assert_eq!(
        field_value(&b_bytes, 1),
        b"musicpack-similarity-fixture-f16-v1".to_vec()
    );
    for tag in [2u8, 3, 5, 6, 7, 8, 9, 10, 14] {
        assert_eq!(
            field_value(&a_bytes, tag),
            field_value(&b_bytes, tag),
            "tag {tag} must not differ between the two profiles"
        );
    }
    assert_eq!(
        docfmt::fingerprint_hex(&docfixtures::fingerprint_a()),
        docfmt::sha256_hex(&a_bytes)
    );
    assert_eq!(
        docfmt::fingerprint_hex(&docfixtures::fingerprint_b()),
        docfmt::sha256_hex(&b_bytes)
    );
}

#[test]
fn the_specification_pins_the_same_tlv_hex_the_encoder_produces() {
    // The digest table and appendix A in FORMAT_SPEC.md are what an independent
    // implementer reads. If the encoder and the document disagree, one of them
    // is wrong and this fails.
    let spec =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("FORMAT_SPEC.md"))
            .expect("FORMAT_SPEC.md");
    // The golden vectors in appendix A are wrapped for readability, so compare
    // against the document with all whitespace removed.
    let unwrapped: String = spec.chars().filter(|c| !c.is_whitespace()).collect();
    for (label, fields) in [
        ("musicpack-similarity-fixture-v1", docfixtures::profile_a()),
        (
            "musicpack-similarity-fixture-f16-v1",
            docfixtures::profile_b(),
        ),
    ] {
        let hex = docfmt::hex_encode(&docfmt::profile_tlv(&fields));
        assert!(
            unwrapped.contains(&hex),
            "FORMAT_SPEC.md appendix A must carry {label}'s canonical TLV hex"
        );
    }
}
