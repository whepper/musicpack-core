//! Identity compatibility: every committed manifest must produce exactly
//! the keys the legacy C server recorded for it.
//!
//! Provenance: `tests/data/identity/manifests/*.manifest.json` are the
//! test inputs (18 synthetic edge cases + 2 real-world manifests extracted
//! from the legacy `docker/library`). Each was placed in a scratch
//! `.mpack` directory, scanned with the **built legacy `musicpack-server`
//! binary**, and the resulting `packages.fingerprint`,
//! `release_groups.group_key`, `releases.release_key` and
//! `packages.manifest_sha256` were dumped to
//! `tests/data/identity/vectors.jsonl` (one JSON object per line). The
//! generator asserted the dumped `manifest_sha256` against the committed
//! file bytes, so the vectors cannot drift from their inputs.
//!
//! Regeneration: repeat the scan-and-dump against the same committed
//! manifests; no C build is needed for ordinary test runs. Never edit a
//! vector to make a test pass — investigate the discrepancy instead.

use std::collections::HashMap;
use std::path::PathBuf;

use musicpack_core::format::manifest::ParsedManifest;
use musicpack_server::identity;

const MANIFEST_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/identity/manifests");
const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/identity/vectors.jsonl"
);

struct Vector {
    manifest_sha256: String,
    fingerprint: String,
    group_key: String,
    release_key: String,
}

fn load_vectors() -> HashMap<String, Vector> {
    let text = std::fs::read_to_string(VECTORS).unwrap();
    let mut map = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v = serde_json_lite(line, n);
        map.insert(v.0, v.1);
    }
    assert!(!map.is_empty(), "no vectors loaded");
    map
}

// Minimal inline JSON field extraction (the file format is fixed:
// flat string-valued objects; no external JSON dependency is warranted
// for a test harness).
fn serde_json_lite(line: &str, n: usize) -> (String, Vector) {
    let field = |key: &str| {
        let needle = format!("\"{key}\": \"");
        let start = line
            .find(&needle)
            .unwrap_or_else(|| panic!("line {n}: missing {key}"))
            + needle.len();
        let end = line[start..].find('"').unwrap() + start;
        line[start..end].to_string()
    };
    let name = field("manifest");
    (
        name,
        Vector {
            manifest_sha256: field("manifest_sha256"),
            fingerprint: field("fingerprint"),
            group_key: field("group_key"),
            release_key: field("release_key"),
        },
    )
}

fn manifest_bytes(name: &str) -> Vec<u8> {
    std::fs::read(PathBuf::from(MANIFEST_DIR).join(name)).unwrap()
}

#[test]
fn every_vector_reproduces_the_c_recorded_keys() {
    let vectors = load_vectors();
    // The committed corpus size is part of the contract: silently dropping
    // a case must fail loudly.
    assert_eq!(vectors.len(), 20, "expected 20 golden vectors");
    for (name, vector) in &vectors {
        let bytes = manifest_bytes(name);
        assert_eq!(
            identity::manifest_hash(&bytes),
            vector.manifest_sha256,
            "{name}: raw manifest hash drifted from its vector"
        );
        let parsed = ParsedManifest::parse(&bytes)
            .unwrap_or_else(|e| panic!("{name}: committed manifest no longer parses: {e}"));
        let manifest = parsed.manifest();
        assert_eq!(
            identity::package_fingerprint(manifest).unwrap(),
            vector.fingerprint,
            "{name}: fingerprint"
        );
        assert_eq!(
            identity::group_key(manifest),
            vector.group_key,
            "{name}: group_key"
        );
        assert_eq!(
            identity::release_key(manifest),
            vector.release_key,
            "{name}: release_key"
        );
    }
}

#[test]
fn musicbrainz_anchors_use_the_mb_prefix() {
    let vectors = load_vectors();
    let mb = &vectors["mb-full.manifest.json"];
    assert_eq!(mb.group_key, "mb:12345678-1234-1234-1234-1234567890ab");
    assert_eq!(mb.release_key, "mb:abcdef01-2345-6789-abcd-ef0123456789");
    // Uppercase hex is still a canonical anchor (the C uses isxdigit).
    let upper = &vectors["mb-upper.manifest.json"];
    assert_eq!(upper.group_key, "mb:12345678-1234-1234-1234-1234567890AB");
    // Malformed ids fall through to the hash path.
    let bad = &vectors["bad-mbid.manifest.json"];
    assert!(bad.group_key.starts_with("h:"));
    assert!(bad.release_key.starts_with("h:"));
}

#[test]
fn artist_order_does_not_change_the_group_key() {
    let vectors = load_vectors();
    let full = &vectors["full-fields.manifest.json"];
    let reordered = &vectors["artist-reorder.manifest.json"];
    assert_eq!(full.group_key, reordered.group_key);
    assert_eq!(full.release_key, reordered.release_key);
    // ...but the fingerprints differ (manifest bytes differ).
    assert_ne!(full.fingerprint, reordered.fingerprint);
}

#[test]
fn absent_empty_and_valued_roles_are_three_identities() {
    let vectors = load_vectors();
    let absent = &vectors["role-absent.manifest.json"].group_key;
    let empty = &vectors["role-empty.manifest.json"].group_key;
    let valued = &vectors["role-value.manifest.json"].group_key;
    assert_ne!(absent, empty, "absent role must differ from empty role");
    assert_ne!(empty, valued);
    assert_ne!(absent, valued);
}

#[test]
fn media_content_does_not_participate_in_identity() {
    let vectors = load_vectors();
    let single = &vectors["full-fields.manifest.json"];
    let multi = &vectors["multi-disc.manifest.json"];
    assert_eq!(single.group_key, multi.group_key);
    assert_eq!(single.release_key, multi.release_key);
    assert_ne!(single.fingerprint, multi.fingerprint);
}

#[test]
fn empty_release_input_hashes_to_the_empty_sha256() {
    // `no-release` carries no release object and no barcode: the release
    // TLV is empty, so the key is the well-known empty-string digest.
    let vectors = load_vectors();
    assert_eq!(
        vectors["no-release.manifest.json"].release_key,
        "h:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn whitespace_and_separator_bytes_are_significant() {
    let vectors = load_vectors();
    assert_ne!(
        vectors["full-fields.manifest.json"].group_key,
        vectors["trailing-space.manifest.json"].group_key,
        "a trailing space must change the key"
    );
    // The separator-heavy manifest parses and keys deterministically
    // (no delimiter ambiguity in the TLV).
    let sep = &vectors["separators.manifest.json"];
    assert!(sep.group_key.starts_with("h:"));
    assert_eq!(sep.group_key.len(), 66);
}
