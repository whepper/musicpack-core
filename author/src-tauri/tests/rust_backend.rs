// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

//! R4.3 host-level vertical: the Tauri application's Rust authoring backend.
//!
//! These tests exercise the exact boundary the Tauri commands use
//! (`musicpack_author_lib::rust_backend::RustBackend`) end to end:
//!
//! ```text
//! host request -> RustBackend -> musicpack-author -> musicpack-core
//!              -> .mpack -> core verify -> .mpak -> (re)inspect
//! ```
//!
//! The last test is a permanent cutover invariant: the default runtime must
//! never spawn the legacy `musicpack` / `mpcenc` / `musicpack-sonic`
//! binaries. It poisons `PATH` with sentinel-writing stubs and asserts they
//! are never executed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use musicpack_author_lib::rust_backend::RustBackend;
use serde_json::{json, Value};

/// Serializes the one test that mutates the process `PATH`.
static PATH_LOCK: Mutex<()> = Mutex::new(());

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "musicpack-author-host-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/audio")
        .join(name)
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A minimal album source tree (one FLAC track plus assets).
fn album_root(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("album");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), root.join("one.flac")).unwrap();
    std::fs::write(root.join("front.jpg"), b"cover").unwrap();
    std::fs::write(root.join("one.lrc"), b"[00:01.00]line\n").unwrap();
    root
}

fn draft_json(root: &Path) -> String {
    format!(
        r#"{{
  "schema": "musicpack-draft",
  "version": 1,
  "sourceRoot": {root},
  "album": {{ "title": "Host Album", "artists": [{{ "name": "Artist" }}], "releaseType": "album" }},
  "identifiers": {{ "musicbrainzReleaseGroupId": "11111111-1111-1111-1111-111111111111", "musicbrainzReleaseId": "22222222-2222-2222-2222-222222222222" }},
  "media": [{{ "disc": 1, "tracks": [{{ "track": 1, "title": "One", "audioPath": "one.flac", "lyrics": [{{ "path": "one.lrc", "lang": "en" }}] }}] }}],
  "artwork": [{{ "role": "front", "path": "front.jpg" }}],
  "booklet": [],
  "lyrics": [],
  "extras": []
}}"#,
        root = json_string(&root.to_string_lossy())
    )
}

fn verified(path: &str) -> Value {
    let mut backend = RustBackend::new();
    backend.verify_package(path).unwrap()
}

#[test]
fn vertical_builds_verifies_packs_and_reinspects() {
    let temp = TempDir::new("vertical");
    let root = album_root(&temp);
    let draft = draft_json(&root);

    let mut backend = RustBackend::new();

    // validate: clean verdict
    let verdict = backend.validate_draft(&draft).unwrap();
    assert_eq!(verdict["ok"], json!(true), "{verdict}");
    assert_eq!(verdict["errors"], json!([]));

    // create .mpack
    let output = temp.path().join("Host Album.mpack");
    let created = backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 6.0)
        .unwrap();
    assert_eq!(created["ok"], json!(true));
    assert!(output.join("audio/01 - One.mpc").is_file());
    assert!(output.join("lyrics/one.lrc").is_file());
    assert!(output.join("analysis/waveform/01-01.wfm").is_file());
    assert!(output.join("manifest.json").is_file());

    // verify through the host
    assert_eq!(verified(output.to_str().unwrap())["ok"], json!(true));

    // create .mpak directly from a draft
    let mpak = temp.path().join("Host Album.mpak");
    let packed = backend
        .create_mpak(&draft, mpak.to_str().unwrap(), 6.0)
        .unwrap();
    assert_eq!(packed["ok"], json!(true));
    assert!(mpak.is_file());
    // The intermediate .mpack is gone.
    assert!(!temp.path().join("Host Album.mpack.tmp").exists());
    assert_eq!(verified(mpak.to_str().unwrap())["ok"], json!(true));

    // pack an existing .mpack
    let converted = temp.path().join("Converted.mpak");
    let pack_result = backend
        .pack_package(output.to_str().unwrap(), converted.to_str().unwrap())
        .unwrap();
    assert_eq!(pack_result["ok"], json!(true));
    assert_eq!(
        std::fs::read(&converted).unwrap(),
        std::fs::read(&mpak).unwrap()
    );

    // inspect the built package back into a draft
    let inspected = backend.inspect_album(output.to_str().unwrap()).unwrap();
    assert_eq!(inspected["album"]["title"], json!("Host Album"));
    assert_eq!(
        inspected["media"][0]["tracks"][0]["audioPath"],
        json!("audio/01 - One.mpc")
    );
    let (c, r) = (
        inspected["openedFrom"].as_str().unwrap_or_default(),
        output.canonicalize().unwrap(),
    );
    assert_eq!(Path::new(c), r);

    // Rebuild from the inspected draft (in place, atomically replaced).
    let replaced = backend
        .create_package(
            &serde_json::to_string(&inspected).unwrap(),
            output.to_str().unwrap(),
            true,
            false,
            6.0,
        )
        .unwrap();
    assert_eq!(replaced["ok"], json!(true));
    assert_eq!(verified(output.to_str().unwrap())["ok"], json!(true));
}

#[test]
fn replacement_refuses_existing_output_without_replace() {
    let temp = TempDir::new("replace");
    let root = album_root(&temp);
    let draft = draft_json(&root);
    let output = temp.path().join("Out.mpack");
    let mut backend = RustBackend::new();
    backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 6.0)
        .unwrap();

    let err = backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 6.0)
        .unwrap_err();
    assert_eq!(err.code, "io_failed");
    assert!(err.message.contains("already exists"), "{err:?}");
    // The previous package is intact.
    assert_eq!(verified(output.to_str().unwrap())["ok"], json!(true));
}

#[test]
fn encode_and_waveform_stages_rewrite_the_draft() {
    let temp = TempDir::new("stages");
    let root = album_root(&temp);
    let draft = draft_json(&root);
    let mut backend = RustBackend::new();

    let staging = temp.path().join("encode");
    let mut done = 0usize;
    let transformed = backend
        .encode_stage(
            &draft,
            &staging,
            6.0,
            &mut |d, total, _disc, _track, _title| {
                done = d;
                assert_eq!(total, 1);
                true
            },
        )
        .unwrap();
    assert_eq!(done, 1);
    let value: Value = serde_json::from_str(&transformed).unwrap();
    assert_eq!(value["sourceRoot"], json!(staging.to_string_lossy()));
    assert!(value["media"][0]["tracks"][0]["audioPath"]
        .as_str()
        .unwrap()
        .ends_with(".mpc"));

    let wf_staging = temp.path().join("wf");
    let mut wf_done = 0usize;
    let wf = backend
        .waveform_stage(&transformed, &wf_staging, &mut |d, _total, entry| {
            wf_done = d;
            assert!(entry.points > 0);
            true
        })
        .unwrap();
    assert_eq!(wf_done, 1);
    let wf_value: Value = serde_json::from_str(&wf).unwrap();
    assert_eq!(wf_value["waveformAnalysis"]["status"], json!("ready"));
}

#[test]
fn lyrics_variants_regress_through_the_host() {
    let temp = TempDir::new("lyrics");
    let root = album_root(&temp);
    // Plain (untimed) and synced lyric documents, plus a lyric-less track.
    std::fs::write(root.join("plain.lrc"), b"a plain line\nsecond line\n").unwrap();
    std::fs::write(
        root.join("synced.lrc"),
        b"[00:01.00]timed one\n[00:02.00]timed two\n",
    )
    .unwrap();
    let draft = format!(
        r#"{{
  "schema": "musicpack-draft",
  "version": 1,
  "sourceRoot": {root},
  "album": {{ "title": "Lyrics Album", "artists": [{{ "name": "Artist" }}], "releaseType": "album" }},
  "media": [{{ "disc": 1, "tracks": [
    {{ "track": 1, "title": "None", "audioPath": "one.flac" }},
    {{ "track": 2, "title": "Plain", "audioPath": "one.flac", "lyrics": [{{ "path": "plain.lrc", "lang": "en" }}] }},
    {{ "track": 3, "title": "Synced", "audioPath": "one.flac", "lyrics": [{{ "path": "synced.lrc" }}] }}
  ]}}]}}"#,
        root = json_string(&root.to_string_lossy())
    );
    let mut backend = RustBackend::new();
    let output = temp.path().join("Lyrics.mpack");
    backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 6.0)
        .unwrap();
    assert_eq!(verified(output.to_str().unwrap())["ok"], json!(true));
    assert!(output.join("lyrics/plain.lrc").is_file());
    assert!(output.join("lyrics/synced.lrc").is_file());

    // Inspect: per-track lyric refs survive with language present/absent.
    let inspected = backend.inspect_album(output.to_str().unwrap()).unwrap();
    assert!(inspected["media"][0]["tracks"][0].get("lyrics").is_none());
    assert_eq!(
        inspected["media"][0]["tracks"][1]["lyrics"][0]["lang"],
        json!("en")
    );
    assert!(inspected["media"][0]["tracks"][2]["lyrics"][0]
        .get("lang")
        .is_none());

    // Removal: drop track 2's lyrics and rebuild in place.
    let mut edited = inspected.clone();
    edited["media"][0]["tracks"][1]
        .as_object_mut()
        .unwrap()
        .remove("lyrics");
    backend
        .create_package(
            &serde_json::to_string(&edited).unwrap(),
            output.to_str().unwrap(),
            true,
            false,
            6.0,
        )
        .unwrap();
    assert_eq!(verified(output.to_str().unwrap())["ok"], json!(true));
    let after = backend.inspect_album(output.to_str().unwrap()).unwrap();
    assert!(after["media"][0]["tracks"][1].get("lyrics").is_none());
    assert!(!output.join("lyrics/plain.lrc").exists());
    // Track 3's lyrics are unaffected.
    assert!(output.join("lyrics/synced.lrc").is_file());
}

#[test]
fn cancellation_stops_the_encode_stage() {
    let temp = TempDir::new("cancel");
    let root = album_root(&temp);
    let draft = draft_json(&root);
    let mut backend = RustBackend::new();
    let staging = temp.path().join("encode");
    let err = backend
        .encode_stage(&draft, &staging, 6.0, &mut |_, _, _, _, _| false)
        .unwrap_err();
    assert_eq!(err.code, "cancelled");
}

// ---------------------------------------------------------------------
// cutover invariant: no legacy subprocess from the Rust runtime
// ---------------------------------------------------------------------

#[test]
fn rust_backend_source_never_spawns_a_process() {
    // A source-level invariant: the default runtime module must not contain
    // process-spawning calls. (The functional poison-PATH test below proves
    // the same thing at runtime; this catches regressions statically.)
    let source = include_str!("../src/rust_backend.rs");
    // Ignore comments so prose that names the legacy tools is not flagged.
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["Command::new", "process::Command", "Stdio", "spawn("] {
        assert!(
            !code.contains(forbidden),
            "rust_backend.rs must not reference `{forbidden}`"
        );
    }
    for name in ["mpcenc", "musicpack-sonic"] {
        assert!(
            !code.contains(name),
            "rust_backend.rs must not reference the legacy `{name}` binary"
        );
    }
}

#[test]
fn poisoned_path_is_never_consulted() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new("poison");
    let root = album_root(&temp);
    let draft = draft_json(&root);

    // Sentinel-writing stubs for every legacy binary.
    let poison = temp.path().join("bin");
    std::fs::create_dir_all(&poison).unwrap();
    let sentinel = temp.path().join("sentinel");
    for name in ["musicpack", "mpcenc", "musicpack-sonic"] {
        let script = poison.join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch {}\nexit 1\n", sentinel.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let original = std::env::var_os("PATH");
    // SAFETY-equivalent: the test holds PATH_LOCK; no other test spawns.
    std::env::set_var("PATH", &poison);

    let mut backend = RustBackend::new();
    let output = temp.path().join("Poison.mpack");
    let created = backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 6.0)
        .unwrap();
    let ok =
        created["ok"] == json!(true) && verified(output.to_str().unwrap())["ok"] == json!(true);

    match original {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }

    assert!(ok, "the Rust runtime failed to build with a poisoned PATH");
    assert!(
        !sentinel.exists(),
        "a legacy binary was executed by the Rust authoring runtime"
    );
}

/// The selected quality must never silently diverge from the quality the
/// package is actually built with: `create_package` with `quality = 7.0`
/// (a user who skips the encode stage) has to produce the q7 stream —
/// byte-identical to encoding the same source at 7.0, and different from
/// the default q6 stream that the old hard-coded behaviour produced.
#[test]
fn create_package_honours_the_selected_quality() {
    let temp = TempDir::new("quality");
    let root = album_root(&temp);
    let draft = draft_json(&root);
    let output = temp.path().join("Q7.mpack");

    let mut backend = RustBackend::new();
    backend
        .create_package(&draft, output.to_str().unwrap(), false, false, 7.0)
        .unwrap();

    let built = std::fs::read(output.join("audio/01 - One.mpc")).unwrap();

    // Reference encodes of the same source through the same Author stage.
    let q7 = temp.path().join("ref-q7.mpc");
    musicpack_author_pipeline::encode::encode_to(&root.join("one.flac"), &q7, 7.0).unwrap();
    let q6 = temp.path().join("ref-q6.mpc");
    musicpack_author_pipeline::encode::encode_to(&root.join("one.flac"), &q6, 6.0).unwrap();

    assert_eq!(
        built,
        std::fs::read(&q7).unwrap(),
        "the package must be built at the selected quality (7.0)"
    );
    assert_ne!(
        built,
        std::fs::read(&q6).unwrap(),
        "the package must not silently fall back to the default quality"
    );
}
