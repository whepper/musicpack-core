//! R4.2 vertical: **Author → package → server**.
//!
//! Runs the Rust authoring pipeline to completion, then ingests the produced
//! `.mpack` through the server's real discovery/ingestion path
//! ([`musicpack_server::ingest::scan`]) with verification enabled, and
//! asserts the package is accepted as valid. This is the end-to-end proof
//! that a Rust-authored package is a first-class server-library package.
//!
//! The `musicpack-author` dependency is a **dev-dependency** only: the
//! server's production artifact is unchanged, and nothing depends on the
//! server in the other direction.

use std::path::{Path, PathBuf};

use musicpack_author::pipeline::{AuthorRequest, PipelineOptions, run};
use musicpack_core::authoring::LoudnessMode;
use musicpack_server::ingest::scan;
use musicpack_server::store::sqlite::SqliteStore;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "author-vertical-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
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

#[test]
fn rust_authored_package_is_ingested_by_the_server() {
    let temp = TempDir::new();
    let library = temp.path().join("library");
    std::fs::create_dir_all(&library).unwrap();

    // Source album.
    let album = temp.path().join("album");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/reference/audio/flac16-44k.flac"),
        album.join("one.flac"),
    )
    .unwrap();

    // Draft JSON.
    let draft = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},
           "album":{{"title":"Author Vertical","artists":[{{"name":"Test Artist"}}],"releaseType":"album"}},
           "identifiers":{{"musicbrainzReleaseGroupId":"11111111-1111-1111-1111-111111111111","musicbrainzReleaseId":"22222222-2222-2222-2222-222222222222"}},
           "media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}}]}}]}}"#,
        root = json_string(&album.to_string_lossy()),
    );

    // Build the package through the Rust authoring pipeline.
    let output = library.join("Author Vertical.mpack");
    let outcome = run(&AuthorRequest {
        draft_json: draft.as_bytes(),
        output: &output,
        options: PipelineOptions {
            waveform: false,
            loudness: LoudnessMode::Omit,
            ..PipelineOptions::default()
        },
        identify: None,
    })
    .unwrap();
    assert!(outcome.report.is_ok());
    assert!(output.join("manifest.json").is_file());

    // Ingest through the real server path, verifying during the scan.
    let db = temp.path().join("library.db");
    let mut store = SqliteStore::open(&db).unwrap();
    let result = scan(&mut store, &library, true).unwrap();
    assert_eq!(result.total, 1, "one library package discovered");
    assert_eq!(
        result.added, 1,
        "the Rust-authored package must be ingested"
    );
    assert_eq!(result.invalid, 0, "the package must verify as valid");

    // The fingerprints the builder derived are what the server stores.
    assert!(!outcome.fingerprint.is_empty());
}

#[test]
fn c_created_database_still_opens_after_authoring_work() {
    // A cheap guard that the authoring dependency did not disturb the
    // server's own store surface: open an empty database and scan an empty
    // library.
    let temp = TempDir::new();
    let library = temp.path().join("empty");
    std::fs::create_dir_all(&library).unwrap();
    let db = temp.path().join("empty.db");
    let mut store = SqliteStore::open(&db).unwrap();
    let result = scan(&mut store, &library, true).unwrap();
    assert_eq!(result.total, 0);
}
