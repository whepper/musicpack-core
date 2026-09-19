//! Focused verification tests: checksums, filesystem hostility, containment,
//! budgets, ordering, and report classification.
//!
//! The filesystem cases mirror the reference repository's hostile-package
//! suite (`tests/run_mpack_hostile.sh`) and pin the reference CLI's observed
//! behaviour (verified empirically against a built reference binary):
//!
//! | Object at a referenced path | Reference outcome |
//! |-----------------------------|-------------------|
//! | regular file, correct digest | verified |
//! | regular file, wrong digest | `checksum mismatch` |
//! | absent | `missing file` |
//! | directory | `missing file` |
//! | FIFO / socket / device | `missing file` (never blocks) |
//! | final-component symlink (target inside or outside) | `missing file` / `unsafe path` |
//! | intermediate directory symlink inside the root | accepted |
//! | intermediate directory symlink outside the root | `unsafe path` |
//! | hard link (`nlink > 1`) | `missing file` |
//! | > 8 GiB | `exceeds …-byte file limit` (before hashing) |
//!
//! All filesystem cases are `#[cfg(unix)]`: the directory adapter implements
//! the POSIX hardening (the reference's Windows checks genuinely differ).

mod support;

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::{Path, PathBuf};

use musicpack_core::format::checksum;
#[cfg(unix)]
use musicpack_core::storage::directory::{DirectoryBackend, verify_directory};
#[cfg(unix)]
use musicpack_core::validation::Severity;

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

/// A fresh package directory under the process temp dir.
#[cfg(unix)]
fn package_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "musicpack-core-verify-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Writes a minimal one-track manifest referencing `rel`.
#[cfg(unix)]
fn write_manifest(dir: &Path, rel: &str, digest: &str) {
    let json = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"{rel}","sha256":"{digest}"}}}}]}}]}}"#
    );
    fs::write(dir.join("manifest.json"), json).unwrap();
}

/// Creates a package whose single asset is `rel` with `bytes`, declaring
/// the correct digest. Returns (dir, digest).
#[cfg(unix)]
fn package_with_asset(name: &str, rel: &str, bytes: &[u8]) -> (PathBuf, String) {
    let dir = package_dir(name);
    let digest = checksum::sha256_hex(bytes);
    write_manifest(&dir, rel, &digest);
    let full = dir.join(rel);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(&full, bytes).unwrap();
    (dir, digest)
}

#[cfg(unix)]
fn findings_of(report: &musicpack_core::validation::Report) -> Vec<String> {
    report
        .findings()
        .iter()
        .map(|f| format!("{:?}: {}", f.severity, f.message))
        .collect()
}

// ---------------------------------------------------------------------
// SHA-256 vectors and oracle agreement
// ---------------------------------------------------------------------

#[test]
fn sha256_matches_the_independent_oracle() {
    // The library uses RustCrypto `sha2`; tests/support carries an
    // independent implementation used to build corpus digests. They must
    // agree on vectors and on arbitrary data.
    assert_eq!(checksum::sha256_hex(b""), support::sha256_hex(b""));
    assert_eq!(checksum::sha256_hex(b"abc"), support::sha256_hex(b"abc"));
    assert_eq!(
        checksum::sha256_hex(b"one"),
        "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed"
    );
    for len in [1usize, 55, 56, 63, 64, 65, 127, 128, 1000, 65536] {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        assert_eq!(
            checksum::sha256_hex(&data),
            support::sha256_hex(&data),
            "len {len}"
        );
    }
}

// ---------------------------------------------------------------------
// Filesystem verification cases
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn correct_asset_verifies() {
    let (dir, _) = package_with_asset("correct", "audio/01.bin", b"one");
    let report = verify_directory(&dir).expect("opens");
    assert!(report.is_ok(), "{:?}", findings_of(&report));
    assert_eq!(report.errors(), 0);
    assert_eq!(report.warnings(), 0);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn checksum_mismatch_is_an_error() {
    let (dir, _) = package_with_asset("mismatch", "audio/01.bin", b"one");
    fs::write(dir.join("audio/01.bin"), b"changed").unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert!(!report.is_ok());
    assert_eq!(report.errors(), 1);
    let f = &report.findings()[0];
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(f.message, "track: checksum mismatch 'audio/01.bin'");
    assert!(f.message.contains("checksum mismatch"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn missing_asset_is_an_error() {
    let dir = package_dir("missing");
    write_manifest(&dir, "audio/01.bin", &checksum::sha256_hex(b"one"));
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert_eq!(
        report.findings()[0].message,
        "track: missing file 'audio/01.bin'"
    );
    assert!(report.findings()[0].message.contains("missing file"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn directory_where_file_expected_is_missing() {
    let dir = package_dir("isdir");
    write_manifest(&dir, "audio/01.bin", &checksum::sha256_hex(b"one"));
    fs::create_dir_all(dir.join("audio/01.bin")).unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert!(report.findings()[0].message.contains("missing file"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn fifo_where_file_expected_does_not_block() {
    let dir = package_dir("fifo");
    write_manifest(&dir, "audio/01.bin", &checksum::sha256_hex(b"one"));
    fs::create_dir_all(dir.join("audio")).unwrap();
    // mkfifo via the system tool (portable across unix CI images).
    let Ok(status) = std::process::Command::new("mkfifo")
        .arg(dir.join("audio/01.bin"))
        .status()
    else {
        eprintln!("note: mkfifo unavailable; FIFO case skipped");
        cleanup(&dir);
        return;
    };
    assert!(status.success());
    // Must return promptly (a read-open of a FIFO would block forever).
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert!(report.findings()[0].message.contains("missing file"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn final_symlink_inside_is_missing() {
    // Reference: O_NOFOLLOW makes a final-component symlink look missing.
    let dir = package_dir("final-inside");
    let digest = checksum::sha256_hex(b"real");
    write_manifest(&dir, "audio/01.mpc", &digest);
    fs::create_dir_all(dir.join("audio")).unwrap();
    fs::write(dir.join("audio/target.mpc"), b"real").unwrap();
    std::os::unix::fs::symlink("target.mpc", dir.join("audio/01.mpc")).unwrap();

    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert_eq!(
        report.findings()[0].message,
        "track: missing file 'audio/01.mpc'"
    );
    // The link target is a regular file and shows up as unreferenced.
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.severity == Severity::Warning
                && f.message == "unreferenced file 'audio/target.mpc'")
    );
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn final_symlink_outside_is_unsafe() {
    let dir = package_dir("final-outside");
    let outside =
        std::env::temp_dir().join(format!("musicpack-core-outside-{}.bin", std::process::id()));
    fs::write(&outside, b"real").unwrap();
    write_manifest(&dir, "audio/01.mpc", &checksum::sha256_hex(b"real"));
    fs::create_dir_all(dir.join("audio")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("audio/01.mpc")).unwrap();

    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert_eq!(
        report.findings()[0].message,
        "track: unsafe path 'audio/01.mpc'"
    );
    assert!(report.findings()[0].message.contains("unsafe path"));
    let _ = fs::remove_file(outside);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn intermediate_symlink_inside_is_accepted() {
    // A directory symlink that resolves within the root is followed by the
    // reference (containment checks pass); the linked-to file is reported
    // as unreferenced because enumeration never follows the link.
    let dir = package_dir("inter-inside");
    write_manifest(&dir, "audio/01.mpc", &checksum::sha256_hex(b"real"));
    fs::create_dir_all(dir.join("realdir")).unwrap();
    fs::write(dir.join("realdir/01.mpc"), b"real").unwrap();
    std::os::unix::fs::symlink("realdir", dir.join("audio")).unwrap();

    let report = verify_directory(&dir).expect("opens");
    assert!(report.is_ok(), "{:?}", findings_of(&report));
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.severity == Severity::Warning
                && f.message == "unreferenced file 'realdir/01.mpc'")
    );
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn intermediate_symlink_outside_is_unsafe() {
    let dir = package_dir("inter-outside");
    let outdir = std::env::temp_dir().join(format!("musicpack-core-outdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&outdir);
    fs::create_dir_all(&outdir).unwrap();
    fs::write(outdir.join("01.mpc"), b"real").unwrap();
    write_manifest(&dir, "audio/01.mpc", &checksum::sha256_hex(b"real"));
    std::os::unix::fs::symlink(&outdir, dir.join("audio")).unwrap();

    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert!(report.findings()[0].message.contains("unsafe path"));
    let _ = fs::remove_dir_all(outdir);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn nested_symlink_chain_behaviour() {
    // audio -> mid -> realdir (inside): accepted.
    let dir = package_dir("nested-inside");
    write_manifest(&dir, "audio/01.mpc", &checksum::sha256_hex(b"real"));
    fs::create_dir_all(dir.join("realdir")).unwrap();
    fs::write(dir.join("realdir/01.mpc"), b"real").unwrap();
    std::os::unix::fs::symlink("realdir", dir.join("mid")).unwrap();
    std::os::unix::fs::symlink("mid", dir.join("audio")).unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert!(report.is_ok(), "{:?}", findings_of(&report));
    cleanup(&dir);

    // audio -> mid -> outside: unsafe.
    let dir = package_dir("nested-outside");
    let outdir =
        std::env::temp_dir().join(format!("musicpack-core-nested-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&outdir);
    fs::create_dir_all(&outdir).unwrap();
    fs::write(outdir.join("01.mpc"), b"real").unwrap();
    write_manifest(&dir, "audio/01.mpc", &checksum::sha256_hex(b"real"));
    std::os::unix::fs::symlink("mid", dir.join("audio")).unwrap();
    std::os::unix::fs::symlink(&outdir, dir.join("mid")).unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    assert!(report.findings()[0].message.contains("unsafe path"));
    let _ = fs::remove_dir_all(outdir);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hard_link_is_rejected() {
    let (dir, _) = package_with_asset("hardlink", "audio/01.bin", b"real");
    let alias =
        std::env::temp_dir().join(format!("musicpack-core-hlink-{}.bin", std::process::id()));
    let _ = fs::remove_file(&alias);
    fs::hard_link(dir.join("audio/01.bin"), &alias).unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    // `nlink > 1` makes the reference treat the object as an outside alias:
    // observed as a missing file.
    assert!(report.findings()[0].message.contains("missing file"));
    let _ = fs::remove_file(alias);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn oversized_file_is_rejected_before_hashing() {
    let dir = package_dir("oversized");
    write_manifest(&dir, "audio/01.bin", &"a".repeat(64));
    fs::create_dir_all(dir.join("audio")).unwrap();
    // Sparse 9 GiB file: rejected by the 8 GiB per-file limit without
    // reading it, so this test stays fast.
    let file = fs::File::create(dir.join("audio/01.bin")).unwrap();
    if file.set_len(9 * 1024 * 1024 * 1024).is_err() {
        eprintln!("note: sparse files unavailable; oversized case skipped");
        cleanup(&dir);
        return;
    }
    drop(file);
    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 1);
    let msg = &report.findings()[0].message;
    assert!(msg.contains("exceeds"), "{msg}");
    assert!(msg.contains("8589934592-byte file limit"), "{msg}");
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn unreferenced_files_warn_but_manifest_does_not() {
    let (dir, _) = package_with_asset("extras", "audio/01.bin", b"one");
    fs::create_dir_all(dir.join("extras")).unwrap();
    fs::write(dir.join("extras/b.txt"), b"x").unwrap();
    fs::write(dir.join("extras/a.txt"), b"x").unwrap();
    let report = verify_directory(&dir).expect("opens");
    assert!(report.is_ok(), "{:?}", findings_of(&report));
    assert_eq!(report.warnings(), 2);
    // Deterministic (sorted) order, and manifest.json is never a warning.
    assert_eq!(
        report
            .findings()
            .iter()
            .map(|f| f.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "unreferenced file 'extras/a.txt'",
            "unreferenced file 'extras/b.txt'"
        ]
    );
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn multiple_failures_have_deterministic_reference_order() {
    // Disc 1 track 1 missing, track 2 checksum mismatch; artwork missing.
    let dir = package_dir("multiple");
    let sha = checksum::sha256_hex(b"two");
    let json = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"audio/01.bin","sha256":"{sha}"}}}},{{"track":2,"title":"Two","audio":{{"path":"audio/02.bin","sha256":"{sha}"}}}}]}}],"artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{sha}"}}]}}"#
    );
    fs::write(dir.join("manifest.json"), json).unwrap();
    fs::create_dir_all(dir.join("audio")).unwrap();
    fs::write(dir.join("audio/02.bin"), b"changed").unwrap();

    let report = verify_directory(&dir).expect("opens");
    assert_eq!(report.errors(), 3);
    let messages: Vec<&str> = report
        .findings()
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert_eq!(
        messages,
        vec![
            "track: missing file 'audio/01.bin'",
            "track: checksum mismatch 'audio/02.bin'",
            "artwork: missing file 'artwork/front.jpg'",
        ]
    );
    // Running again yields identical findings.
    let again = verify_directory(&dir).expect("opens");
    assert_eq!(report, again);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn open_failures_are_errors() {
    // Missing root.
    let missing = std::env::temp_dir().join(format!(
        "musicpack-core-does-not-exist-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);
    assert!(verify_directory(&missing).is_err());

    // Manifest absent.
    let dir = package_dir("no-manifest");
    assert!(DirectoryBackend::open(&dir).is_err());
    cleanup(&dir);

    // Root is a file.
    let dir = package_dir("root-file");
    let file = dir.join("x.mpak");
    fs::write(&file, b"not a directory").unwrap();
    assert!(DirectoryBackend::open(&file).is_err());
    cleanup(&dir);
}
