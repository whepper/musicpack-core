//! MPAK reference compatibility: reading a reference-packed container and
//! byte-identical writing.
//!
//! Committed fixtures (produced by the reference CLI, unmodified):
//!
//! - `fixtures/reference/mpak-source/` — a tiny directory bundle whose
//!   members exercise the real `INDX`/`MANF`/`DATA`/`TAIL` layout;
//! - `fixtures/reference/reference-small.mpak` — `musicpack pack` output
//!   for exactly that bundle.
//!
//! Regeneration (from a checkout with the reference CLI built; never
//! modify the committed fixtures in place without regenerating both):
//!
//! ```sh
//! musicpack pack fixtures/reference/mpak-source fixtures/reference/reference-small.mpak
//! ```
//!
//! The writer byte-identity assertion below is the strongest available
//! compatibility proof: it compares Rust output against the reference
//! implementation's own bytes without needing the CLI at test time.

use std::path::PathBuf;
use std::sync::Arc;

use musicpack_core::format::mpak::{MemorySource, MpakReader};
use musicpack_core::storage::mpak::{MpakBackend, verify_mpak};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference")
}

fn reference_container() -> Vec<u8> {
    std::fs::read(fixture_dir().join("reference-small.mpak")).expect("committed container fixture")
}

fn source_package() -> PathBuf {
    fixture_dir().join("mpak-source")
}

fn scan(bytes: Vec<u8>) -> MpakReader {
    musicpack_core::format::mpak::scan(&MemorySource::new(bytes), false).expect("scans")
}

#[test]
fn reads_the_reference_container() {
    let bytes = reference_container();
    let reader = scan(bytes.clone());
    assert_eq!(reader.file_size(), bytes.len() as u64);
    assert_eq!(reader.minor(), 0);
    assert!(!reader.reserved_nonzero());
    assert!(!reader.resynced());
    assert_eq!(reader.manifest_count(), 1);
    assert!(reader.indx_present());
    assert!(
        reader.indx_valid(),
        "reference INDX reconciles with the scan"
    );
    assert!(reader.tail_present());
    assert!(!reader.tail_malformed());
    assert_eq!(reader.skipped_members(), 0);
    assert_eq!(reader.duplicate_members(), 0);

    // Members: audio, artwork, extras (DATA order = pack order).
    let paths: Vec<&str> = reader.members().iter().map(|m| m.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["audio/01.bin", "artwork/front.bin", "extras/notes.txt"]
    );

    // INDX entries carry the manifest's hashes (equal to the source file
    // digests, since the reference pack only packs verified packages).
    for member in reader.members() {
        let expected = musicpack_core::format::checksum::sha256_hex(
            &std::fs::read(source_package().join(&member.path)).expect("source member"),
        );
        let declared =
            musicpack_core::format::checksum::sha256_hex_to_bytes(&expected).expect("valid hex");
        assert_eq!(
            reader.indx_sha256(&member.path),
            Some(&declared),
            "INDX hash for {}",
            member.path
        );
    }

    // TAIL self-consistency.
    assert_eq!(reader.tail_total_size(), bytes.len() as u64);
    assert_eq!(reader.tail_objects(), reader.members().len() as u32);
    assert_eq!(
        reader.tail_indx_offset(),
        musicpack_core::format::mpak::HEADER_LEN as u64
    );
}

#[test]
fn reference_container_verifies_through_the_shared_verifier() {
    let report =
        verify_mpak(Arc::new(MemorySource::new(reference_container()))).expect("opens and parses");
    assert!(
        report.is_ok(),
        "reference container must verify cleanly: {:?}",
        report
            .findings()
            .iter()
            .map(|f| f.message.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(report.errors(), 0);
    assert_eq!(report.warnings(), 0);
}

#[test]
fn backend_exposes_members_to_the_verifier() {
    use musicpack_core::storage::PackageBackend;

    let backend =
        MpakBackend::open(Arc::new(MemorySource::new(reference_container()))).expect("opens");
    let opened = backend.open_asset("audio/01.bin").expect("member");
    let on_disk_len = std::fs::metadata(source_package().join("audio/01.bin"))
        .unwrap()
        .len();
    assert_eq!(opened.len, on_disk_len);
    let listed = backend.list_files();
    assert_eq!(
        listed,
        vec!["audio/01.bin", "artwork/front.bin", "extras/notes.txt"]
    );
    assert!(backend.meta_files().is_empty());
    assert!(backend.object_id("audio/01.bin").is_none());
    assert!(backend.open_asset("missing.bin").is_err());

    // Extraction matches the source package's bytes.
    let extracted = backend.read_member("audio/01.bin", 1024).unwrap();
    let on_disk = std::fs::read(source_package().join("audio/01.bin")).unwrap();
    assert_eq!(extracted, on_disk);
}

#[cfg(unix)]
#[test]
fn rust_pack_matches_the_reference_bytes() {
    let mut rust_bytes = Vec::new();
    musicpack_core::storage::directory::pack_directory_to_writer(
        &source_package(),
        &mut rust_bytes,
    )
    .expect("packs the reference source bundle");
    let reference_bytes = reference_container();
    assert_eq!(
        rust_bytes.len(),
        reference_bytes.len(),
        "container size diverges from the reference pack"
    );
    assert_eq!(
        rust_bytes, reference_bytes,
        "Rust MPAK output must be byte-identical to the reference pack"
    );
}

#[cfg(unix)]
#[test]
fn rust_pack_is_deterministic_across_calls() {
    let mut a = Vec::new();
    let mut b = Vec::new();
    musicpack_core::storage::directory::pack_directory_to_writer(&source_package(), &mut a)
        .expect("pack a");
    musicpack_core::storage::directory::pack_directory_to_writer(&source_package(), &mut b)
        .expect("pack b");
    assert_eq!(a, b, "separate pack invocations must be byte-identical");
}

#[cfg(unix)]
#[test]
fn rust_pack_round_trips_through_the_reader() {
    let mut bytes = Vec::new();
    musicpack_core::storage::directory::pack_directory_to_writer(&source_package(), &mut bytes)
        .expect("pack");
    // Read back and verify.
    let report = verify_mpak(Arc::new(MemorySource::new(bytes.clone()))).expect("opens");
    assert!(report.is_ok(), "{:?}", report.findings());
    // And the members match the source files.
    let backend = MpakBackend::open(Arc::new(MemorySource::new(bytes))).expect("opens");
    for (path, file) in [
        ("audio/01.bin", "audio/01.bin"),
        ("artwork/front.bin", "artwork/front.bin"),
        ("extras/notes.txt", "extras/notes.txt"),
    ] {
        let from_container = backend.read_member(path, 1 << 20).unwrap();
        let from_disk = std::fs::read(source_package().join(file)).unwrap();
        assert_eq!(from_container, from_disk, "{path}");
    }
}
