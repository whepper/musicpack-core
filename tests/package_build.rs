//! R4.1 — core package-directory builder (authoring draft → `.mpack`).
//!
//! These tests exercise the builder directly (there is deliberately no
//! authoring CLI yet — R4.2). They cover the contract from
//! `docs/package-builder.md`: the minimal package, multi-disc structure,
//! asset materialization, lyrics, determinism, identity, typed failures,
//! path safety, atomicity, and compatibility with the legacy C builder /
//! verifier.
//!
//! The builder is portable (POSIX hardening on unix, reference-matching
//! checks elsewhere — see `docs/adr/0015-windows-directory-adapter.md`),
//! so these tests run on every native target. C-differential cases skip
//! gracefully when the reference CLI is absent.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use musicpack_core::authoring::{
    AuthoringDraft, BuildOptions, DraftAnalysis, DraftArtwork, DraftAsset, DraftDisc, DraftLyrics,
    DraftRepresentation, DraftTrack, DraftWaveform, LoudnessMode, build_directory,
};
use musicpack_core::error::Error;
use musicpack_core::format::checksum;
use musicpack_core::format::manifest::{
    Album, Artist, Identifiers, MediumFormat, ParsedManifest, ReleaseType,
};
use musicpack_core::storage::directory::verify_directory;
use musicpack_core::{identity, storage};

// ---------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------

/// A self-removing temporary directory.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("musicpack-build-{}-{name}-{n}", std::process::id()));
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

/// Absolute path of an audio fixture.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/reference/audio")
        .join(name)
}

/// The source root used by the builder tests: a directory containing
/// copies/links of the fixtures the test needs.
fn source_root(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    root
}

fn album(title: &str) -> Album {
    Album {
        title: title.to_string(),
        artists: vec![Artist {
            name: "Test Artist".to_string(),
            ..Artist::default()
        }],
        release_type: Some(ReleaseType::Album),
        original_release_date: None,
        genres: Vec::new(),
    }
}

fn draft_with(media: Vec<DraftDisc>) -> AuthoringDraft {
    AuthoringDraft {
        album: album("Test Album"),
        release: None,
        identifiers: None,
        identity: None,
        source: None,
        media,
        artwork: Vec::new(),
        booklet: Vec::new(),
        lyrics: Vec::new(),
        extras: Vec::new(),
        analysis: Vec::new(),
        provenance: None,
    }
}

fn track(number: i32, title: &str, package_path: &str, source: &str) -> DraftTrack {
    DraftTrack {
        number,
        title: title.to_string(),
        artists: Vec::new(),
        identifiers: None,
        source: None,
        source_audio: None,
        audio_codec: None,
        audio: DraftAsset::new(package_path, source),
        waveform: None,
        lyrics: Vec::new(),
        representations: Vec::new(),
    }
}

fn read_manifest(dir: &Path) -> (Vec<u8>, ParsedManifest) {
    let bytes = fs::read(dir.join("manifest.json")).unwrap();
    let parsed = ParsedManifest::parse(&bytes).unwrap();
    (bytes, parsed)
}

// ---------------------------------------------------------------------
// minimal + structure + determinism
// ---------------------------------------------------------------------

#[test]
fn minimal_package_builds_and_verifies() {
    let temp = TempDir::new("minimal");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    let out = temp.path().join("Album.mpack");

    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: Some(MediumFormat::Digital),
        title: None,
        tracks: vec![track(1, "One", "audio/01 - One.flac", "one.flac")],
    }]);

    let outcome = build_directory(&draft, &root, &out, &BuildOptions::default()).unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert_eq!(outcome.output, out);
    assert!(out.join("manifest.json").is_file());
    assert!(out.join("audio/01 - One.flac").is_file());

    let (bytes, parsed) = read_manifest(&out);
    assert_eq!(
        parsed.manifest().media[0].tracks[0].audio.sha256,
        checksum::sha256_hex(&fs::read(out.join("audio/01 - One.flac")).unwrap())
    );
    // Canonical bytes are stable under a re-serialization round trip.
    assert_eq!(
        parsed.manifest().write_canonical().unwrap().as_bytes(),
        bytes
    );
    // Loudness is measured by default.
    assert!(parsed.manifest().media[0].tracks[0].loudness.is_some());
    assert!(parsed.manifest().loudness.is_some());
    assert_eq!(
        parsed
            .manifest()
            .loudness
            .as_ref()
            .unwrap()
            .algorithm
            .as_deref(),
        Some(musicpack_core::audio::loudness::STANDARD)
    );
    // A single track is its own concatenated program: track and album
    // measurements coincide exactly.
    let track_loudness = parsed.manifest().media[0].tracks[0].loudness.unwrap();
    let album_loudness = parsed.manifest().loudness.as_ref().unwrap();
    assert_eq!(track_loudness.lufs, album_loudness.lufs);
    assert_eq!(track_loudness.true_peak_db, album_loudness.true_peak_db);
    assert!(parsed.manifest().media[0].tracks[0].duration.unwrap() > 0.0);
    // Identity inputs are available and derived from the built manifest.
    assert_eq!(
        outcome.fingerprint,
        identity::package_fingerprint(parsed.manifest()).unwrap()
    );
    assert_eq!(outcome.group_key, identity::group_key(parsed.manifest()));
    assert_eq!(
        outcome.release_key,
        identity::release_key(parsed.manifest())
    );
    assert_eq!(outcome.manifest_sha256, checksum::sha256_hex(&bytes));
    assert!(outcome.group_key.starts_with("h:"));
    assert!(outcome.release_key.starts_with("h:"));
}

#[test]
fn multi_disc_package_is_deterministic() {
    let temp = TempDir::new("determinism");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("a.flac")).unwrap();

    let draft = draft_with(vec![
        DraftDisc {
            number: 1,
            format: None,
            title: Some("Disc One".into()),
            tracks: vec![
                track(1, "One", "audio/01 - One.flac", "a.flac"),
                track(2, "Two", "audio/02 - Two.flac", "a.flac"),
            ],
        },
        DraftDisc {
            number: 2,
            format: Some(MediumFormat::Cd),
            title: None,
            tracks: vec![track(1, "Three", "audio/2-01 - Three.flac", "a.flac")],
        },
    ]);

    let out_a = temp.path().join("a.mpack");
    let out_b = temp.path().join("b.mpack");
    let opts = BuildOptions::default();
    let first = build_directory(&draft, &root, &out_a, &opts).unwrap();
    let second = build_directory(&draft, &root, &out_b, &opts).unwrap();

    let (bytes_a, _) = read_manifest(&out_a);
    let (bytes_b, _) = read_manifest(&out_b);
    assert_eq!(bytes_a, bytes_b, "identical draft must be byte-identical");
    assert_eq!(first.fingerprint, second.fingerprint);
    assert_eq!(first.group_key, second.group_key);
    assert_eq!(first.release_key, second.release_key);

    // Disc/track ids are preserved in draft order.
    let parsed = ParsedManifest::parse(&bytes_a).unwrap();
    assert_eq!(
        parsed
            .manifest()
            .media
            .iter()
            .map(|d| d.number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(parsed.manifest().media[0].tracks.len(), 2);
}

// ---------------------------------------------------------------------
// assets, waveform, lyrics, .mpak
// ---------------------------------------------------------------------

#[test]
fn assets_materialize_and_pack_round_trips() {
    let temp = TempDir::new("assets");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    fs::write(root.join("front.jpg"), b"front-bytes").unwrap();
    fs::write(root.join("notes.pdf"), b"booklet-bytes").unwrap();
    fs::write(root.join("root.lrc"), b"[00:01.00]root\n").unwrap();
    fs::write(root.join("bonus.txt"), b"extra-bytes").unwrap();
    fs::write(root.join("sonic.json"), b"{\"sonic\":true}").unwrap();
    fs::write(root.join("alt.flac"), b"alt-representation-bytes").unwrap();

    let mut t = track(1, "One", "audio/01 - One.flac", "one.flac");
    t.representations.push(DraftRepresentation {
        path: "audio/01 - One (alt).flac".into(),
        source: "alt.flac".into(),
        label: Some("FLAC 24/96".into()),
        codec: Some("flac".into()),
    });
    let draft = AuthoringDraft {
        artwork: vec![DraftArtwork {
            role: "front".into(),
            asset: DraftAsset::new("artwork/front.jpg", "front.jpg"),
        }],
        booklet: vec![DraftAsset::new("booklet/notes.pdf", "notes.pdf")],
        lyrics: vec![DraftAsset::new("lyrics/root.lrc", "root.lrc")],
        extras: vec![DraftAsset::new("extras/bonus.txt", "bonus.txt")],
        analysis: vec![DraftAnalysis {
            kind: "sonic".into(),
            profile: Some("openl3-v1".into()),
            asset: DraftAsset::new("analysis/sonic.json", "sonic.json"),
        }],
        ..draft_with(vec![DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![t],
        }])
    };

    let out = temp.path().join("Assets.mpack");
    let outcome = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());

    for rel in [
        "audio/01 - One.flac",
        "audio/01 - One (alt).flac",
        "artwork/front.jpg",
        "booklet/notes.pdf",
        "lyrics/root.lrc",
        "extras/bonus.txt",
        "analysis/sonic.json",
    ] {
        assert!(out.join(rel).is_file(), "missing {rel}");
    }

    let (_, parsed) = read_manifest(&out);
    let m = parsed.manifest();
    assert_eq!(m.artwork[0].asset.path, "artwork/front.jpg");
    assert_eq!(m.booklet[0].path, "booklet/notes.pdf");
    assert_eq!(m.lyrics[0].path, "lyrics/root.lrc");
    assert_eq!(m.extras[0].path, "extras/bonus.txt");
    assert_eq!(m.analysis[0].kind, "sonic");
    assert_eq!(m.analysis[0].profile.as_deref(), Some("openl3-v1"));
    assert_eq!(m.media[0].tracks[0].representations.len(), 1);
    assert_eq!(
        m.media[0].tracks[0].representations[0].sha256,
        checksum::sha256_hex(b"alt-representation-bytes")
    );

    // The existing `.mpak` writer consumes the built directory unchanged.
    let mpak = temp.path().join("Assets.mpak");
    storage::directory::pack_directory(&out, &mpak).unwrap();
    let report = storage::mpak::verify_mpak_file(&mpak).unwrap();
    assert!(report.is_ok(), "{:?}", report.findings());
    assert_eq!(
        storage::directory::DirectoryBackend::open(&out)
            .unwrap()
            .manifest_bytes(),
        ParsedManifest::parse(&read_manifest(&out).0)
            .unwrap()
            .manifest()
            .write_canonical()
            .unwrap()
            .as_bytes()
    );
}

#[test]
fn waveform_points_derive_from_the_payload() {
    let temp = TempDir::new("waveform");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    // 20 bytes → 10 buckets.
    fs::write(root.join("one.wfm"), vec![0x40u8; 20]).unwrap();

    let mut t = track(1, "One", "audio/01 - One.flac", "one.flac");
    t.waveform = Some(DraftWaveform {
        path: "analysis/waveform/01-01.wfm".into(),
        source: "one.wfm".into(),
    });
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![t],
    }]);

    let out = temp.path().join("Wave.mpack");
    let outcome = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    let (_, parsed) = read_manifest(&out);
    let waveform = parsed.manifest().media[0].tracks[0]
        .waveform
        .as_ref()
        .unwrap();
    assert_eq!(waveform.points, 10);
    assert_eq!(waveform.sha256, checksum::sha256_hex(&[0x40u8; 20]));

    // An odd payload length is rejected.
    fs::write(root.join("odd.wfm"), vec![0u8; 7]).unwrap();
    let mut bad = track(1, "One", "audio/01 - One.flac", "one.flac");
    bad.waveform = Some(DraftWaveform {
        path: "analysis/waveform/01-01.wfm".into(),
        source: "odd.wfm".into(),
    });
    let bad_draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![bad],
    }]);
    let err = build_directory(
        &bad_draft,
        &root,
        &temp.path().join("Bad.mpack"),
        &BuildOptions::omitting_loudness(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("even payload length"), "{err}");
}

#[test]
fn track_lyrics_are_attached() {
    let temp = TempDir::new("lyrics");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    let lrc = "[ti:Song]\n[00:01.00]first line\n[00:04.00]second line\n";
    fs::write(root.join("one.lrc"), lrc).unwrap();

    let mut t = track(1, "One", "audio/01 - One.flac", "one.flac");
    t.lyrics
        .push(DraftLyrics::new("lyrics/one.lrc", "one.lrc", Some("en")));
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![t],
    }]);

    let out = temp.path().join("Lyrics.mpack");
    let outcome = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    let (_, parsed) = read_manifest(&out);
    let refs = &parsed.manifest().media[0].tracks[0].lyrics;
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].path, "lyrics/one.lrc");
    assert_eq!(refs[0].lang.as_deref(), Some("en"));
    assert_eq!(refs[0].sha256, checksum::sha256_hex(lrc.as_bytes()));
    assert_eq!(
        fs::read(out.join("lyrics/one.lrc")).unwrap(),
        lrc.as_bytes()
    );
    // The core lyrics parser accepts what the builder attached.
    assert!(musicpack_core::lyrics::parse(&fs::read(out.join("lyrics/one.lrc")).unwrap()).is_ok());
}

// ---------------------------------------------------------------------
// invalid input, path safety, sources, atomicity
// ---------------------------------------------------------------------

fn expect_error(draft: &AuthoringDraft, root: &Path, out: &Path, needle: &str) {
    let err = build_directory(draft, root, out, &BuildOptions::omitting_loudness()).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "expected error containing {needle:?}, got {err}"
    );
    assert!(!out.exists(), "failed build must not publish {out:?}");
}

#[test]
fn malformed_drafts_are_rejected() {
    let temp = TempDir::new("invalid");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    let out = temp.path().join("Bad.mpack");

    let good_track = || track(1, "One", "audio/01 - One.flac", "one.flac");

    // Empty media.
    let mut d = draft_with(Vec::new());
    expect_error(&d, &root, &out, "media must be a non-empty array");

    // No album artists.
    d = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![good_track()],
    }]);
    d.album.artists.clear();
    expect_error(&d, &root, &out, "album artists must not be empty");

    // Empty title.
    let mut t = good_track();
    t.title = String::new();
    d = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![t],
    }]);
    expect_error(&d, &root, &out, "title must not be empty");

    // Duplicate track number.
    d = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![
            track(1, "One", "audio/01 - One.flac", "one.flac"),
            track(1, "Two", "audio/02 - Two.flac", "one.flac"),
        ],
    }]);
    expect_error(&d, &root, &out, "duplicate track number 1");

    // Duplicate disc number.
    d = draft_with(vec![
        DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![track(1, "One", "audio/01 - One.flac", "one.flac")],
        },
        DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![track(1, "Two", "audio/02 - Two.flac", "one.flac")],
        },
    ]);
    expect_error(&d, &root, &out, "duplicate disc number 1");

    // Duplicate package-relative path.
    d = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![
            track(1, "One", "audio/same.flac", "one.flac"),
            track(2, "Two", "audio/same.flac", "one.flac"),
        ],
    }]);
    expect_error(&d, &root, &out, "duplicate asset path");

    // Invalid lyrics language.
    let mut t = good_track();
    t.lyrics.push(DraftLyrics {
        path: "lyrics/one.lrc".into(),
        source: "one.lrc".into(),
        lang: Some(String::new()),
    });
    d = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![t],
    }]);
    expect_error(&d, &root, &out, "lang");
}

#[test]
fn package_paths_are_validated() {
    let temp = TempDir::new("paths");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();

    for bad in [
        "../x", "/abs/x", "a//b", "a/./b", "a/../b", "a/", "a\\b", "a:b",
    ] {
        let draft = draft_with(vec![DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![track(1, "One", bad, "one.flac")],
        }]);
        let out = temp.path().join("Bad.mpack");
        let err =
            build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap_err();
        assert!(
            matches!(err, Error::Path(_)),
            "path {bad:?} should be a typed path error, got {err:?}"
        );
        assert!(!out.exists());
    }
}

#[test]
fn sources_must_stay_inside_the_source_root() {
    let temp = TempDir::new("escape");
    let root = source_root(&temp);
    let outside = temp.path().join("outside.flac");
    fs::copy(fixture("flac-mono-44k.flac"), &outside).unwrap();

    for bad in ["../outside.flac", "/etc/hosts", ""] {
        let draft = draft_with(vec![DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![track(1, "One", "audio/01.flac", bad)],
        }]);
        let err = build_directory(
            &draft,
            &root,
            &temp.path().join("Bad.mpack"),
            &BuildOptions::omitting_loudness(),
        )
        .unwrap_err();
        assert!(
            matches!(err, Error::Invalid { .. }),
            "source {bad:?} should be rejected, got {err:?}"
        );
    }

    // A missing in-root source is a typed Missing error.
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![track(1, "One", "audio/01.flac", "nope.flac")],
    }]);
    let err = build_directory(
        &draft,
        &root,
        &temp.path().join("Bad.mpack"),
        &BuildOptions::omitting_loudness(),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }), "{err:?}");
}

#[test]
fn failure_leaves_no_package_or_staging_behind() {
    let temp = TempDir::new("atomic");
    let parent = temp.path().join("dest");
    fs::create_dir_all(&parent).unwrap();
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    fs::write(root.join("odd.wfm"), vec![1u8; 7]).unwrap();

    // The build fails after the audio has been copied (odd waveform).
    let mut t = track(1, "One", "audio/01 - One.flac", "one.flac");
    t.waveform = Some(DraftWaveform {
        path: "analysis/waveform/01-01.wfm".into(),
        source: "odd.wfm".into(),
    });
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![t],
    }]);
    let out = parent.join("Nope.mpack");
    let err = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap_err();
    assert!(err.to_string().contains("even payload length"), "{err}");

    assert!(!out.exists());
    let leftovers: Vec<String> = fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(leftovers.is_empty(), "staging leftovers: {leftovers:?}");
}

#[test]
fn existing_destination_is_refused() {
    let temp = TempDir::new("exists");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    let out = temp.path().join("Taken.mpack");
    fs::create_dir_all(&out).unwrap();
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![track(1, "One", "audio/01.flac", "one.flac")],
    }]);
    let err = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}

// ---------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------

#[test]
fn identity_anchors_on_musicbrainz_ids() {
    let temp = TempDir::new("identity");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();

    let mut draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![track(1, "One", "audio/01.flac", "one.flac")],
    }]);
    draft.identifiers = Some(Identifiers {
        musicbrainz_release_group_id: Some("12345678-1234-1234-1234-1234567890ab".into()),
        musicbrainz_release_id: Some("22345678-1234-1234-1234-1234567890ab".into()),
        barcode: None,
    });

    let out = temp.path().join("Id.mpack");
    let outcome = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();
    assert_eq!(outcome.group_key, "mb:12345678-1234-1234-1234-1234567890ab");
    assert_eq!(
        outcome.release_key,
        "mb:22345678-1234-1234-1234-1234567890ab"
    );
}

// ---------------------------------------------------------------------
// loudness mode
// ---------------------------------------------------------------------

#[test]
fn loudness_can_be_omitted() {
    let temp = TempDir::new("no-loudness");
    let root = source_root(&temp);
    fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![track(1, "One", "audio/01.flac", "one.flac")],
    }]);
    let out = temp.path().join("Quiet.mpack");
    let outcome = build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();
    assert!(outcome.report.is_ok());
    assert!(outcome.manifest.media[0].tracks[0].loudness.is_none());
    assert!(outcome.manifest.media[0].tracks[0].duration.is_none());
    assert!(outcome.manifest.loudness.is_none());
    let _ = LoudnessMode::Measure;
}

// ---------------------------------------------------------------------
// C compatibility (skips when the reference CLI is not built)
// ---------------------------------------------------------------------

fn reference_cli_or_skip() -> Option<PathBuf> {
    match support::reference_cli() {
        Some(cli) => Some(cli),
        None => {
            eprintln!(
                "note: reference CLI not built; C compatibility test skipped. \
                 Set MUSICPACK_REF_CLI or build ../musicpack."
            );
            None
        }
    }
}

#[test]
fn rust_built_package_is_accepted_by_the_c_verifier() {
    let Some(cli) = reference_cli_or_skip() else {
        return;
    };
    let temp = TempDir::new("cverify");
    let root = source_root(&temp);
    fs::copy(fixture("flac16-44k.flac"), root.join("song.flac")).unwrap();
    fs::write(root.join("front.jpg"), b"artwork").unwrap();

    let mut t = track(1, "One", "audio/01 - One.flac", "song.flac");
    t.lyrics
        .push(DraftLyrics::new("lyrics/one.lrc", "one.lrc", Some("en")));
    fs::write(root.join("one.lrc"), "[00:01.00]line\n").unwrap();
    let draft = AuthoringDraft {
        artwork: vec![DraftArtwork {
            role: "front".into(),
            asset: DraftAsset::new("artwork/front.jpg", "front.jpg"),
        }],
        ..draft_with(vec![DraftDisc {
            number: 1,
            format: None,
            title: None,
            tracks: vec![t],
        }])
    };
    let out = temp.path().join("Rust.mpack");
    build_directory(&draft, &root, &out, &BuildOptions::omitting_loudness()).unwrap();

    let result = Command::new(&cli).arg("verify").arg(&out).output().unwrap();
    assert!(
        result.status.success(),
        "C verify rejected the Rust-built package:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn c_builder_and_rust_builder_agree_byte_for_byte() {
    let Some(cli) = reference_cli_or_skip() else {
        return;
    };
    let temp = TempDir::new("cdiff");
    let root = source_root(&temp);
    fs::copy(fixture("flac16-44k.flac"), root.join("song.flac")).unwrap();

    // C build-draft for the simplest album: one track, loudness disabled,
    // waveform disabled. This is the exact package the Rust builder must
    // reproduce.
    let draft_json = format!(
        "{{\"schema\":\"musicpack-draft\",\"version\":1,\"sourceRoot\":{root},\
          \"album\":{{\"title\":\"Test Album\",\"artists\":[{{\"name\":\"Test Artist\"}}],\"releaseType\":\"album\"}},\
          \"media\":[{{\"disc\":1,\"tracks\":[{{\"track\":1,\"title\":\"One\",\"audioPath\":\"song.flac\"}}]}}],\
          \"artwork\":[],\"booklet\":[],\"lyrics\":[],\"extras\":[],\
          \"waveformAnalysis\":{{\"status\":\"disabled\"}}}}",
        root = json_string(&root.to_string_lossy()),
    );
    let draft_path = temp.path().join("draft.json");
    fs::write(&draft_path, draft_json).unwrap();
    let c_out = temp.path().join("C.mpack");
    let result = Command::new(&cli)
        .arg("build-draft")
        .arg("--draft")
        .arg(&draft_path)
        .arg("-o")
        .arg(&c_out)
        .arg("--no-loudness")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "C build-draft failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let draft = draft_with(vec![DraftDisc {
        number: 1,
        format: None,
        title: None,
        tracks: vec![track(1, "One", "audio/01 - One.flac", "song.flac")],
    }]);
    let rust_out = temp.path().join("Rust.mpack");
    build_directory(&draft, &root, &rust_out, &BuildOptions::omitting_loudness()).unwrap();

    let (rust_manifest, _) = read_manifest(&rust_out);
    let (c_manifest, _) = read_manifest(&c_out);
    assert_eq!(
        String::from_utf8_lossy(&rust_manifest),
        String::from_utf8_lossy(&c_manifest),
        "Rust and C manifests must be byte-identical"
    );
    // The audio member is byte-identical too.
    assert_eq!(
        fs::read(rust_out.join("audio/01 - One.flac")).unwrap(),
        fs::read(c_out.join("audio/01 - One.flac")).unwrap()
    );
    // Both packages verify with the core verifier.
    let rust_report = verify_directory(&rust_out).unwrap();
    let c_report = verify_directory(&c_out).unwrap();
    assert!(rust_report.is_ok() && c_report.is_ok());
}

/// Minimal JSON string escaping for the temp paths used above.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
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
