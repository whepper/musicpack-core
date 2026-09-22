//! Portability pin for the authoring/package-builder path.
//!
//! Regression coverage for the Windows CI failure that shipped with R4:
//! `musicpack_core::authoring` and `musicpack_core::storage::directory`
//! were `#[cfg(unix)]`-gated while `musicpack-author` consumed them
//! unconditionally, so `cargo build --workspace --all-targets` failed on
//! Windows with E0432/E0433. The core builder is portable std-only; the
//! directory backend applies the reference's own Windows checks there
//! (see `docs/adr/0015-windows-directory-adapter.md`).
//!
//! This file runs on every native target and pins both the shared
//! behavior (build → verify → pack round trip) and the per-platform
//! backend contract, so re-gating either module breaks a named test
//! instead of only a far-away consumer build.

use std::fs;
use std::path::{Path, PathBuf};

use musicpack_core::authoring::{
    AuthoringDraft, BuildOptions, DraftAsset, DraftDisc, DraftTrack, build_directory,
};
use musicpack_core::format::checksum;
use musicpack_core::format::manifest::{Album, Artist};
use musicpack_core::storage::PackageBackend;
use musicpack_core::storage::directory::{DirectoryBackend, pack_directory, verify_directory};

/// A self-removing temporary directory.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "musicpack-portable-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn minimal_draft() -> AuthoringDraft {
    AuthoringDraft {
        album: Album {
            title: "Portable Album".to_string(),
            artists: vec![Artist {
                name: "Test Artist".to_string(),
                ..Artist::default()
            }],
            release_type: None,
            original_release_date: None,
            genres: Vec::new(),
        },
        release: None,
        identifiers: None,
        identity: None,
        source: None,
        media: vec![DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![DraftTrack {
                number: 1,
                title: "One".to_string(),
                artists: Vec::new(),
                identifiers: None,
                source: None,
                source_audio: None,
                audio_codec: None,
                audio: DraftAsset::new("audio/01 - One.flac", "one.flac"),
                waveform: None,
                lyrics: Vec::new(),
                representations: Vec::new(),
            }],
        }],
        artwork: Vec::new(),
        booklet: Vec::new(),
        lyrics: Vec::new(),
        extras: Vec::new(),
        analysis: Vec::new(),
        provenance: None,
    }
}

/// The authoring modules every native consumer needs are available here.
/// (If either module is ever re-gated to `unix`, this file — compiled for
/// every native target — fails to build with the same E0432/E0433 the
/// Windows CI job reported.)
#[test]
fn build_verify_and_pack_round_trip() {
    let temp = TempDir::new("roundtrip");
    let root = temp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/reference/audio/flac-mono-44k.flac"),
        root.join("one.flac"),
    )
    .unwrap();
    let out = temp.path().join("Portable.mpack");

    let outcome = build_directory(&minimal_draft(), &root, &out, &BuildOptions::default()).unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert!(out.join("audio/01 - One.flac").is_file());

    // The staged package verifies through the directory backend.
    let report = verify_directory(&out).unwrap();
    assert!(report.is_ok(), "{:?}", report.findings());

    // The backend opens the built package and exposes its manifest.
    let backend = DirectoryBackend::open(&out).unwrap();
    assert!(!backend.manifest_bytes().is_empty());

    // A verified directory packs into a container that re-verifies.
    let mpak = temp.path().join("Portable.mpak");
    pack_directory(&out, &mpak).unwrap();
    let packed = musicpack_core::storage::mpak::verify_mpak_file(&mpak).unwrap();
    assert!(packed.is_ok(), "{:?}", packed.findings());
    assert_eq!(
        checksum::sha256_hex(&fs::read(out.join("manifest.json")).unwrap()),
        outcome.manifest_sha256
    );
}

/// The per-platform backend contract: stable object identity exists on
/// unix; off unix (Windows) the reference disables inode dedup, so the
/// backend reports none and verification hashes every asset instead.
#[test]
fn object_identity_follows_the_platform_contract() {
    let temp = TempDir::new("identity");
    let root = temp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/reference/audio/flac-mono-44k.flac"),
        root.join("one.flac"),
    )
    .unwrap();
    let out = temp.path().join("Portable.mpack");
    build_directory(&minimal_draft(), &root, &out, &BuildOptions::default()).unwrap();

    let backend = DirectoryBackend::open(&out).unwrap();
    #[cfg(unix)]
    assert!(backend.object_id("audio/01 - One.flac").is_some());
    #[cfg(not(unix))]
    assert!(backend.object_id("audio/01 - One.flac").is_none());
}

/// The unreferenced-file walk is unix-only: the reference skips it on
/// Windows, so the backend lists no files there (and verification emits
/// no unreferenced-file warnings).
#[test]
fn file_enumeration_follows_the_platform_contract() {
    let temp = TempDir::new("listing");
    let root = temp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/reference/audio/flac-mono-44k.flac"),
        root.join("one.flac"),
    )
    .unwrap();
    let out = temp.path().join("Portable.mpack");
    build_directory(&minimal_draft(), &root, &out, &BuildOptions::default()).unwrap();

    let backend = DirectoryBackend::open(&out).unwrap();
    #[cfg(unix)]
    assert_eq!(
        backend.list_files(),
        vec!["audio/01 - One.flac", "manifest.json"]
    );
    #[cfg(not(unix))]
    assert!(backend.list_files().is_empty());
}
