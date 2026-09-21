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

// ---------------------------------------------------------------------
// Per-track lyrics references (docs/musicpack-lyrics-v1.md §6)
// ---------------------------------------------------------------------

use musicpack_core::format::manifest::LyricsRef;

const LYR_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn sha_of(bytes: &[u8]) -> String {
    use musicpack_core::format::checksum::sha256_hex;
    sha256_hex(bytes)
}

#[test]
fn track_lyrics_canonical_position_and_round_trip() {
    // A manifest with waveform + lyrics + representations on one track:
    // lyrics serialize between waveform and representations (spec §6.1),
    // and parse → write → re-parse preserves them exactly.
    let bytes = fixture("test-musicpack-album.mpack-manifest.json");
    let mut parsed = ParsedManifest::parse(&bytes).expect("parses");
    let track = &mut parsed.manifest_mut().media[0].tracks[0];
    track.lyrics = vec![
        LyricsRef {
            path: "lyrics/01.en.lrc".into(),
            sha256: LYR_SHA.into(),
            lang: Some("en".into()),
        },
        LyricsRef {
            path: "lyrics/01.de.lrc".into(),
            sha256: LYR_SHA.into(),
            lang: None,
        },
    ];
    // Give the track a representation too so the three-way canonical
    // order (waveform < lyrics < representations) is pinned in one
    // document.
    track.representations = vec![musicpack_core::format::manifest::Representation {
        path: "audio/01.flac".into(),
        sha256: LYR_SHA.into(),
        label: None,
        codec: Some("flac".into()),
    }];
    let out = parsed.write_canonical().expect("writes");
    let wave_pos = out.find("\"waveform\"").expect("waveform emitted");
    let lyr_pos = out.find("\"lyrics\": [").expect("track lyrics emitted");
    let rep_pos = out
        .find("\"representations\"")
        .expect("representations emitted");
    assert!(
        wave_pos < lyr_pos && lyr_pos < rep_pos,
        "canonical track order: waveform < lyrics < representations"
    );
    // lang omitted (never null) when absent; present verbatim otherwise.
    assert!(out.contains("\"lang\": \"en\""), "{out}");
    assert!(!out.contains("\"lang\": null"), "{out}");
    let reparsed = ParsedManifest::parse(out.as_bytes()).expect("re-parses");
    assert_eq!(reparsed.manifest().media[0].tracks[0].lyrics.len(), 2);
    assert_eq!(
        reparsed.manifest().media[0].tracks[0].lyrics[0]
            .lang
            .as_deref(),
        Some("en")
    );
}

#[test]
fn track_lyrics_omitted_entirely_when_empty() {
    // Absent and empty are equivalent on write; committed reference
    // fixtures carry no track lyrics, so their canonical bytes have no
    // track-level "lyrics" key at all.
    let bytes = fixture("canonical-test-musicpack-album.json");
    let parsed = ParsedManifest::parse(&bytes).expect("parses");
    let rewritten = parsed.write_canonical().expect("writes");
    assert_eq!(rewritten.as_bytes(), bytes.as_slice());
}

#[test]
fn track_lyrics_participate_in_canonical_identity() {
    // The canonical bytes (and therefore the package fingerprint, which
    // is their SHA-256) change exactly when the lyrics references
    // change — and are byte-identical otherwise.
    let bytes = fixture("test-flac-album.mpack-manifest.json");
    let base = ParsedManifest::parse(&bytes).expect("parses");
    let base_out = base.write_canonical().expect("writes");
    let mut edited = ParsedManifest::parse(&bytes).expect("parses");
    edited.manifest_mut().media[0].tracks[0].lyrics = vec![LyricsRef {
        path: "lyrics/01.lrc".into(),
        sha256: LYR_SHA.into(),
        lang: Some("en".into()),
    }];
    let edited_out = edited.write_canonical().expect("writes");
    assert_ne!(
        base_out, edited_out,
        "adding lyrics changes the canonical bytes"
    );
    assert_ne!(
        sha_of(base_out.as_bytes()),
        sha_of(edited_out.as_bytes()),
        "adding lyrics changes the manifest hash"
    );
    // A lang-only change changes identity too (the field is canonical).
    let mut relanged = ParsedManifest::parse(edited_out.as_bytes()).expect("parses");
    relanged.manifest_mut().media[0].tracks[0].lyrics[0].lang = Some("de".into());
    let relanged_out = relanged.write_canonical().expect("writes");
    assert_ne!(edited_out, relanged_out, "lang participates in identity");
    // Entry order is manifest order (deterministic): swapping entries
    // changes the bytes.
    let mut swapped = ParsedManifest::parse(edited_out.as_bytes()).expect("parses");
    swapped.manifest_mut().media[0].tracks[0]
        .lyrics
        .push(LyricsRef {
            path: "lyrics/01b.lrc".into(),
            sha256: LYR_SHA.into(),
            lang: None,
        });
    let two = swapped.write_canonical().expect("writes");
    swapped.manifest_mut().media[0].tracks[0].lyrics.swap(0, 1);
    let swapped_out = swapped.write_canonical().expect("writes");
    assert_ne!(two, swapped_out, "entry order is significant");
}
