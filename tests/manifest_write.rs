//! Byte-identity tests for canonical manifest serialization.
//!
//! `fixtures/reference/canonical-*.json` are **unmodified copies** of
//! manifests rewritten by the reference implementation's own tooling
//! (`musicpack update-metadata`, which serializes through
//! `musicpack_manifest_write_with_original`): the canonical key order,
//! `%.8g` numbers, and — for `canonical-with-unknown-fields.json` — the
//! unknown-root-field preservation rule (unknown fields appended last, in
//! original order).
//!
//! The `*.mpack-manifest.json` fixtures are the *originally committed*
//! fixture manifests (Python-generated, alphabetically sorted keys). They
//! are valid manifests but are **not** canonical-writer output; they are
//! used here for parsing/semantic tests only.
//!
//! Regeneration (run from a checkout of the reference repository; never
//! modify the reference fixtures in place — work on copies):
//!
//! ```sh
//! cp -R tests/reference/test-musicpack-album.mpack /tmp/canon/
//! musicpack update-metadata /tmp/canon/test-musicpack-album.mpack
//! cp /tmp/canon/test-musicpack-album.mpack/manifest.json fixtures/reference/
//! ```

use std::fs;

use musicpack_core::format::manifest::{Manifest, ParsedManifest};

fn fixture(name: &str) -> Vec<u8> {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("fixtures/reference");
    path.push(name);
    fs::read(path).expect("reference fixture present")
}

/// Parse → write must reproduce the reference tooling's canonical bytes.
#[test]
fn canonical_round_trip_is_byte_identical() {
    for name in [
        "canonical-test-musicpack-album.json",
        "canonical-test-flac-album.json",
        "canonical-with-unknown-fields.json",
    ] {
        let bytes = fixture(name);
        let parsed =
            ParsedManifest::parse(&bytes).unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
        let rewritten = parsed
            .write_canonical()
            .unwrap_or_else(|e| panic!("{name}: write failed: {e}"));
        assert_eq!(
            rewritten.as_bytes(),
            bytes.as_slice(),
            "{name}: canonical output diverges from the reference bytes"
        );
    }
}

/// The standalone model write (no original tree) drops unknown fields —
/// mirroring the C `musicpack_manifest_write`.
#[test]
fn model_write_drops_unknown_fields() {
    let bytes = fixture("canonical-with-unknown-fields.json");
    let parsed = ParsedManifest::parse(&bytes).expect("parses");
    let model: Manifest = parsed.into_manifest();
    let out = model.write_canonical().expect("writes");
    assert!(!out.contains("xFutureField"));
    assert!(!out.contains("zAnother"));
    // Still valid.
    ParsedManifest::parse(out.as_bytes()).expect("re-parses");
}

#[test]
fn parse_reports_expected_semantics() {
    // Spot-check parsed semantics on the richest reference manifest
    // (the originally committed, Python-sorted fixture).
    let bytes = fixture("test-musicpack-album.mpack-manifest.json");
    let parsed = ParsedManifest::parse(&bytes).expect("parses");
    let m = parsed.manifest();
    assert_eq!(m.album.title, "Synthetic Test Compilation");
    assert_eq!(
        m.album.release_type,
        Some(musicpack_core::format::manifest::ReleaseType::Compilation)
    );
    assert_eq!(m.album.artists.len(), 2);
    assert_eq!(m.media.len(), 1);
    assert_eq!(m.media[0].tracks.len(), 4);
    // Waveform references survived.
    let t = &m.media[0].tracks[0];
    let wf = t.waveform.as_ref().expect("track 1 has a waveform");
    assert_eq!(wf.points, 10);
    // Track loudness survived with both values.
    let loudness = t.loudness.expect("track 1 has loudness");
    assert!(loudness.lufs < 0.0 && loudness.true_peak_db < 0.0);
    // Album loudness survived.
    let album_loudness = m.loudness.as_ref().expect("album loudness present");
    assert_eq!(album_loudness.algorithm.as_deref(), Some("ITU-R BS.1770-5"));
}

#[test]
fn unknown_field_semantics() {
    let bytes = fixture("canonical-with-unknown-fields.json");
    let parsed = ParsedManifest::parse(&bytes).expect("parses");
    // The unknown fields are visible in the original tree.
    assert!(parsed.original().get("xFutureField").is_some());
    assert!(parsed.original().get("zAnother").is_some());
    // ...and preserved through a canonical write, in original order...
    let out = parsed.write_canonical().expect("writes");
    let x_pos = out.find("xFutureField").expect("xFutureField preserved");
    let z_pos = out.find("zAnother").expect("zAnother preserved");
    assert!(x_pos < z_pos, "unknown fields keep their original order");
    // ...and are the LAST members of the document.
    let out_trim = out.trim_end();
    let last_brace = out_trim.rfind('}').expect("object close");
    assert!(z_pos < last_brace);
    // The preserved values were re-serialized canonically (2.5, null, false).
    assert!(out.contains("2.5"));
    assert!(out.contains("null"));
    assert!(out.contains("false"));
}

#[test]
fn model_edits_round_trip() {
    // Parse → edit a field → write → re-parse: the edit is visible and
    // the document stays valid (the validate_for_write discipline).
    let bytes = fixture("test-flac-album.mpack-manifest.json");
    let mut parsed = ParsedManifest::parse(&bytes).expect("parses");
    parsed.manifest_mut().album.title = "Edited Title".to_string();
    let out = parsed.write_canonical().expect("writes");
    let reparsed = ParsedManifest::parse(out.as_bytes()).expect("re-parses");
    assert_eq!(reparsed.manifest().album.title, "Edited Title");
}

#[test]
fn loudness_precision_survives_a_round_trip() {
    // The reference manifests carry 7-decimal loudness values; they must
    // survive parse → write unchanged (the %.8g port in action).
    let bytes = fixture("test-musicpack-album.mpack-manifest.json");
    let parsed = ParsedManifest::parse(&bytes).expect("parses");
    let lufs = parsed.manifest().media[0].tracks[0]
        .loudness
        .as_ref()
        .expect("loudness")
        .lufs;
    let out = parsed.write_canonical().expect("writes");
    let reparsed = ParsedManifest::parse(out.as_bytes()).expect("re-parses");
    let lufs2 = reparsed.manifest().media[0].tracks[0]
        .loudness
        .as_ref()
        .expect("loudness")
        .lufs;
    assert_eq!(lufs, lufs2);
}
