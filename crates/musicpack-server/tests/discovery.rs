//! Filesystem discovery tests: the walk rules, budgets, safety properties
//! and determinism of [`musicpack_server::discover`], plus an optional
//! cross-check against the legacy C scanner.
//!
//! The synthetic library trees are built at test runtime in the cargo temp
//! dir (no commit weight, no platform-specific fixtures on disk). Symlink
//! and permission cases are `#[cfg(unix)]`, matching the repository's
//! convention for filesystem-semantics tests.

// `HashSet` is only used by the unix-only symlink test below.
#[cfg(unix)]
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use musicpack_server::discover::{
    CandidateBody, DiscoverError, InvalidReason, PackageCandidate, discover,
};

const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("discover-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn minimal_manifest(title: &str, artist: &str) -> String {
    format!(
        "{{\"format\":\"musicpack\",\"version\":1,\"album\":{{\"title\":\"{title}\",\"artists\":[{{\"name\":\"{artist}\"}}]}},\"media\":[{{\"disc\":1,\"tracks\":[{{\"track\":1,\"title\":\"T\",\"audio\":{{\"path\":\"audio/01.mpc\",\"sha256\":\"{EMPTY_SHA}\"}}}}]}}]}}"
    )
}

fn write_package(dir: &Path, manifest: &str) {
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    std::fs::write(dir.join("audio/01.mpc"), b"x").unwrap();
}

/// Builds the standard mixed tree; returns the root. Layout:
/// `alpha.mpack`, `beta.mpack` (valid), `nested/inner.mpack` (valid),
/// `plain/` (files only), `empty.mpack/` (no manifest), `bad.mpack/`
/// (malformed JSON), `archive.mpak` (file), `notes.txt` (file),
/// `UPPER.MPACK/` (directory, not a package).
fn mixed_tree() -> PathBuf {
    let root = test_root("mixed");
    write_package(
        &root.join("alpha.mpack"),
        &minimal_manifest("Alpha", "Alice"),
    );
    write_package(&root.join("beta.mpack"), &minimal_manifest("Beta", "Bob"));
    write_package(
        &root.join("nested").join("inner.mpack"),
        &minimal_manifest("Inner", "Ivy"),
    );
    std::fs::create_dir_all(root.join("plain")).unwrap();
    std::fs::write(root.join("plain/file.txt"), b"plain").unwrap();
    std::fs::create_dir_all(root.join("empty.mpack")).unwrap();
    std::fs::create_dir_all(root.join("bad.mpack")).unwrap();
    std::fs::write(root.join("bad.mpack/manifest.json"), b"{nope").unwrap();
    std::fs::write(root.join("archive.mpak"), b"mpak").unwrap();
    std::fs::write(root.join("notes.txt"), b"notes").unwrap();
    std::fs::create_dir_all(root.join("UPPER.MPACK")).unwrap();
    root
}

fn names(candidates: &[PackageCandidate], root: &Path) -> Vec<String> {
    candidates
        .iter()
        .map(|c| {
            c.path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn discovers_exactly_the_package_directories() {
    let root = mixed_tree();
    let found = discover(&root).unwrap();
    // Sorted by path. UPPER.MPACK is absent (wrong case: recursed, not a
    // package); plain/, archive.mpak and notes.txt are absent (not `.mpack`
    // directories); empty.mpack and bad.mpack are present but invalid.
    assert_eq!(
        names(&found, &root),
        vec![
            "alpha.mpack",
            "bad.mpack",
            "beta.mpack",
            "empty.mpack",
            "nested/inner.mpack",
        ]
    );
}

#[test]
fn valid_packages_carry_identity_and_invalid_ones_carry_reasons() {
    let root = mixed_tree();
    let found = discover(&root).unwrap();
    for name in ["alpha.mpack", "beta.mpack", "nested/inner.mpack"] {
        let candidate = found.iter().find(|c| c.path.ends_with(name)).unwrap();
        assert_eq!(candidate.manifest_sha256.len(), 64);
        match &candidate.body {
            CandidateBody::Valid {
                fingerprint,
                group_key,
                release_key,
                ..
            } => {
                assert_eq!(fingerprint.len(), 64);
                assert!(group_key.starts_with("h:"));
                assert!(release_key.starts_with("h:"));
            }
            CandidateBody::Invalid(reason) => panic!("{name} should be valid: {reason:?}"),
        }
    }
    let empty = found
        .iter()
        .find(|c| c.path.ends_with("empty.mpack"))
        .unwrap();
    assert!(matches!(
        empty.body,
        CandidateBody::Invalid(InvalidReason::UnreadableManifest(_))
    ));
    let bad = found
        .iter()
        .find(|c| c.path.ends_with("bad.mpack"))
        .unwrap();
    assert!(matches!(
        bad.body,
        CandidateBody::Invalid(InvalidReason::InvalidManifest(_))
    ));
}

#[test]
fn results_are_sorted_by_path() {
    let root = mixed_tree();
    let found = discover(&root).unwrap();
    let mut sorted = found.clone();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(
        found
            .iter()
            .zip(sorted.iter())
            .all(|(a, b)| a.path == b.path),
        "discovery must return candidates in path order"
    );
}

#[test]
fn root_ending_in_mpack_is_enumerated_not_reported() {
    // The reference opens the root and walks its children; a root that
    // itself looks like a package is never a candidate.
    let outer = test_root("mpack-root");
    let root = outer.join("library.mpack");
    write_package(&root.join("inner.mpack"), &minimal_manifest("I", "I"));
    let found = discover(&root).unwrap();
    assert_eq!(names(&found, &root), vec!["inner.mpack"]);
}

#[test]
fn mpack_contents_are_never_descended_into() {
    let root = test_root("no-descend");
    write_package(&root.join("outer.mpack"), &minimal_manifest("O", "O"));
    // A nested .mpack inside a package must not be discovered.
    write_package(
        &root.join("outer.mpack").join("sneaky.mpack"),
        &minimal_manifest("S", "S"),
    );
    let found = discover(&root).unwrap();
    assert_eq!(names(&found, &root), vec!["outer.mpack"]);
}

#[test]
fn depth_limit_fails_the_whole_discovery() {
    // 63 nested dirs + package: fine. 64 nested dirs: the 64th directory
    // is entered at depth 64 and fails the traversal.
    for (label, depth, ok) in [("shallow", 63, true), ("deep", 64, false)] {
        let root = test_root(&format!("depth-{label}"));
        let mut dir = root.clone();
        for i in 0..depth {
            dir = dir.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&dir).unwrap();
        write_package(&dir.join("pkg.mpack"), &minimal_manifest("D", "D"));
        let result = discover(&root);
        if ok {
            assert!(result.is_ok(), "{label} should discover");
            assert_eq!(result.unwrap().len(), 1);
        } else {
            assert!(
                matches!(result, Err(DiscoverError::IncompleteTraversal { .. })),
                "{label} should fail closed, got {result:?}"
            );
        }
    }
}

#[test]
fn object_budget_fails_closed() {
    let root = test_root("objects");
    for i in 0..100_001 {
        std::fs::write(root.join(format!("f{i}")), b"").unwrap();
    }
    assert!(
        matches!(discover(&root), Err(DiscoverError::BudgetExceeded { .. })),
        "the 100,001st object must abort discovery"
    );
}

#[test]
fn package_budget_fails_closed() {
    let root = test_root("packages");
    for i in 0..10_001 {
        write_package(
            &root.join(format!("p{i:05}.mpack")),
            &minimal_manifest("P", "P"),
        );
    }
    assert!(
        matches!(discover(&root), Err(DiscoverError::BudgetExceeded { .. })),
        "the 10,001st package must abort discovery"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_followed() {
    use std::os::unix::fs::symlink;
    let root = mixed_tree();
    // A symlinked package, a symlinked directory (with a package behind
    // it), and a symlink loop must all be invisible to the walk.
    symlink(root.join("alpha.mpack"), root.join("link.mpack")).unwrap();
    symlink(root.join("nested"), root.join("dirlink")).unwrap();
    symlink(root.join("loop-b"), root.join("loop-a")).unwrap();
    symlink(root.join("loop-a"), root.join("loop-b")).unwrap();
    symlink("/nonexistent-target-xyz", root.join("dangling")).unwrap();
    let found = discover(&root).unwrap();
    let set: HashSet<String> = names(&found, &root).into_iter().collect();
    assert!(!set.contains("link.mpack"), "symlinked package skipped");
    assert!(!set.contains("dirlink"), "symlinked dir not descended");
    assert!(!set.contains("dirlink/inner.mpack"));
    assert!(!set.contains("loop-a"));
    assert!(!set.contains("dangling"));
    assert_eq!(set.len(), 5, "only the real packages remain");
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_names_do_not_break_the_walk() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let root = test_root("nonutf8");
    write_package(&root.join("ok.mpack"), &minimal_manifest("O", "O"));
    let weird = OsStr::from_bytes(b"we\xffird.mpack");
    std::fs::create_dir(root.join(weird)).unwrap();
    let found = discover(&root).unwrap();
    // The non-UTF8 `.mpack` dir is still matched by the byte-exact suffix
    // rule (and reported invalid: no manifest inside).
    assert_eq!(found.len(), 2);
}

#[cfg(unix)]
#[test]
fn unreadable_subtree_fails_closed() {
    let root = mixed_tree();
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    let mut perms = std::fs::metadata(&locked).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&locked, perms.clone()).unwrap();
    // Remove all permissions (readonly() only clears write on unix? set
    // mode 0 explicitly for a guaranteed-unreadable dir).
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = discover(&root);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(result, Err(DiscoverError::IncompleteTraversal { .. })),
        "an unreadable subtree must fail the discovery, got {result:?}"
    );
}

#[test]
fn legacy_scanner_discovers_the_same_package_set() {
    // Cross-check: the C scanner must ingest exactly the valid packages
    // the Rust discovery reports (invalid ones land in `invalid` rows).
    // Requires MUSICPACK_LEGACY_SERVER (the repository's differential-test
    // convention); skips with a notice otherwise.
    let Some(binary) = std::env::var_os("MUSICPACK_LEGACY_SERVER") else {
        eprintln!(
            "notice: MUSICPACK_LEGACY_SERVER is not set; the discovery \
             cross-check against the C scanner was skipped."
        );
        return;
    };
    let root = mixed_tree();
    let found = discover(&root).unwrap();

    let db = root.join("crosscheck.db");
    let output = std::process::Command::new(&binary)
        .arg("scan")
        .arg("--library")
        .arg(&root)
        .arg("--database")
        .arg(&db)
        .env_remove("MUSICPACK_LOG")
        .output()
        .expect("failed to run the legacy server binary");
    assert!(output.status.success());

    // Packages table paths are absolute; compare the relative sets of
    // successfully ingested (non-invalid) packages against the valid Rust
    // candidates.
    let db_text = std::process::Command::new("sqlite3")
        .arg(&db)
        .arg("SELECT path FROM packages WHERE status != 'invalid' ORDER BY path;")
        .output()
        .expect("sqlite3 CLI is required for the cross-check")
        .stdout;
    let db_text = String::from_utf8(db_text).unwrap();
    let mut c_paths: Vec<String> = db_text
        .lines()
        .map(|p| {
            Path::new(p)
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    c_paths.sort();
    let mut rust_paths: Vec<String> = found
        .iter()
        .filter(|c| matches!(c.body, CandidateBody::Valid { .. }))
        .map(|c| {
            c.path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    rust_paths.sort();
    // The C scan also descends UPPER.MPACK (empty, no package) — same set.
    // Note: the C database file itself lives under root but is a file, so
    // it never interferes with discovery in either implementation.
    assert_eq!(
        rust_paths, c_paths,
        "Rust and C must agree on the valid set"
    );
}
