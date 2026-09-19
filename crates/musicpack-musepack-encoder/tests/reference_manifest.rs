//! Validates the frozen encoder compatibility manifest shipped with this
//! crate.
//!
//! The manifest is the permanent compatibility boundary for the Rust encoder:
//! 94 deterministic WAV inputs encoded at q5/q6/q7 by the pinned reference
//! build, yielding 282 expected whole-file SHA-256 values. This test pins the
//! *structure* and the *bytes* of that artifact so that no later phase can
//! quietly regenerate, edit or "fix" it to accommodate a Rust result.
//!
//! The test is self-contained (it does not need the C reference repository or
//! the generated WAV corpus) and therefore keeps working after the legacy C
//! encoder is removed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// SHA-256 of the frozen manifest mirror, recorded at freeze time. Any change
/// to the manifest changes this digest and fails the test.
const FROZEN_MANIFEST_SHA256: &str =
    "2f628c79efedd6062d2ae86373d8ebfc78230c330cb7f58d643dbb1acd215bc9";

/// Expected number of `<input> <quality> <sha256>` data lines.
const EXPECTED_VECTORS: usize = 282;
/// Expected number of distinct input files.
const EXPECTED_INPUTS: usize = 94;
/// The quality levels each input must cover.
const EXPECTED_QUALITIES: [u8; 3] = [5, 6, 7];

fn manifest_path() -> PathBuf {
    match std::env::var_os("MUSICPACK_ENCODER_MANIFEST") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/encoder_reference_manifest.txt"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// One parsed `<input> <quality> <sha256>` manifest row.
struct Vector {
    input: String,
    quality: u8,
    hash: String,
}

fn load_manifest() -> Vec<Vector> {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read frozen manifest {}: {error}", path.display()));
    let mut vectors = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let input = fields
            .next()
            .unwrap_or_else(|| panic!("manifest line {} has no input name", index + 1));
        let quality = fields
            .next()
            .and_then(|q| q.parse::<u8>().ok())
            .unwrap_or_else(|| panic!("manifest line {} has no valid quality", index + 1));
        let hash = fields
            .next()
            .unwrap_or_else(|| panic!("manifest line {} has no hash", index + 1));
        assert!(
            fields.next().is_none(),
            "manifest line {} has trailing fields",
            index + 1
        );
        vectors.push(Vector {
            input: input.to_owned(),
            quality,
            hash: hash.to_owned(),
        });
    }
    vectors
}

#[test]
fn frozen_manifest_bytes_are_unchanged() {
    let path = manifest_path();
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("cannot read frozen manifest {}: {error}", path.display()));
    assert_eq!(
        sha256_hex(&bytes),
        FROZEN_MANIFEST_SHA256,
        "the frozen encoder manifest was modified; it must never be regenerated or edited \
         to accommodate an encoder result (see crates/musicpack-musepack-encoder/README.md)"
    );
}

#[test]
fn manifest_has_the_expected_vector_count() {
    let vectors = load_manifest();
    assert_eq!(
        vectors.len(),
        EXPECTED_VECTORS,
        "expected {EXPECTED_VECTORS} reference vectors"
    );
}

#[test]
fn manifest_covers_every_input_at_every_quality_exactly_once() {
    let vectors = load_manifest();
    let mut by_input: BTreeMap<String, BTreeSet<u8>> = BTreeMap::new();
    let mut pairs: BTreeSet<(String, u8)> = BTreeSet::new();

    for vector in &vectors {
        assert!(
            pairs.insert((vector.input.clone(), vector.quality)),
            "duplicate manifest entry for {} q{}",
            vector.input,
            vector.quality
        );
        by_input
            .entry(vector.input.clone())
            .or_default()
            .insert(vector.quality);
    }

    assert_eq!(
        by_input.len(),
        EXPECTED_INPUTS,
        "expected {EXPECTED_INPUTS} distinct inputs"
    );
    for (input, qualities) in &by_input {
        let expected: BTreeSet<u8> = EXPECTED_QUALITIES.into_iter().collect();
        assert_eq!(
            qualities, &expected,
            "input {input} does not cover exactly qualities {EXPECTED_QUALITIES:?}"
        );
    }
}

#[test]
fn every_manifest_hash_is_a_lowercase_sha256() {
    for vector in load_manifest() {
        assert_eq!(
            vector.hash.len(),
            64,
            "{} q{}: hash is not 64 hex characters",
            vector.input,
            vector.quality
        );
        assert!(
            vector
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{} q{}: hash is not lowercase hexadecimal",
            vector.input,
            vector.quality
        );
    }
}
