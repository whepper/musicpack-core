//! The `.mpack` v1 conformance corpus — regression gate for manifest
//! parsing *and* verification.
//!
//! The corpus is the authoritative behavioural spec from the reference
//! repository (72 cases: 6 valid / 57 invalid-manifest / 9 invalid-verify;
//! `waveform-points-mismatch` + `symlink-escape` included). This test:
//!
//! 1. materialises the corpus via the Rust port in `tests/support` and
//!    — when the reference checkout + Python are available — byte-compares
//!    it against the authoritative generator (the port must never drift);
//! 2. asserts parse outcomes per group on every platform;
//! 3. on unix, runs the full directory-bundle verification and asserts the
//!    authoritative outcome per group (valid accepted; the other groups
//!    rejected — at parse time or by verification);
//! 4. expects **zero gaps**.

mod support;

use std::path::PathBuf;

use support::{Group, ManifestCase, write_corpus};

fn corpus_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "musicpack-core-conformance-{}-{name}",
        std::process::id()
    ))
}

fn manifest_bytes(root: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(root.join(format!("{name}.mpack")).join("manifest.json"))
        .expect("manifest present")
}

/// Byte-compares the Rust-generated corpus against the authoritative
/// Python generator when both are available.
#[test]
fn corpus_port_matches_the_authoritative_generator() {
    if support::reference_generator().is_none() {
        eprintln!(
            "note: reference repository not found; corpus byte-comparison skipped \
             (set MUSICPACK_REFERENCE_DIR to enable)"
        );
        return;
    }
    let py_dir = corpus_dir("generator").join("py");
    match support::run_reference_generator(&py_dir) {
        // Python 3 unavailable: skip (the Rust port is still exercised by
        // the parse test below).
        None => {
            eprintln!("note: python3 unavailable; corpus byte-comparison skipped");
            return;
        }
        Some(Err(e)) => panic!("reference generator failed: {e}"),
        Some(Ok(())) => {}
    }

    let rust_dir = corpus_dir("generator").join("rust");
    write_corpus(&rust_dir).expect("rust corpus builds");

    // Compare directory listings and every manifest byte.
    let mut py_cases: Vec<String> = std::fs::read_dir(&py_dir)
        .expect("py corpus")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    py_cases.sort();
    let mut rust_cases: Vec<String> = std::fs::read_dir(&rust_dir)
        .expect("rust corpus")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    rust_cases.sort();
    assert_eq!(rust_cases, py_cases, "corpus case sets diverge");

    for case in rust_cases {
        let py_bytes = std::fs::read(py_dir.join(&case).join("manifest.json")).unwrap();
        let rust_bytes = std::fs::read(rust_dir.join(&case).join("manifest.json")).unwrap();
        assert_eq!(
            rust_bytes, py_bytes,
            "{case}: corpus port diverges from the authoritative generator"
        );
    }
}

#[test]
fn outcomes_match_the_authoritative_expectations() {
    let root = corpus_dir("outcomes");
    let cases = write_corpus(&root).expect("corpus builds");
    assert_eq!(cases.len(), 72, "the corpus has 72 cases");

    let mut failures = Vec::new();

    for case in &cases {
        let bytes = match &case.manifest {
            ManifestCase::Raw(raw) if raw.is_empty() => {
                // symlink-escape: same expectation as the other
                // invalid-verify cases; the manifest is well-formed.
                manifest_bytes(&root, "minimal")
            }
            ManifestCase::Raw(raw) => raw.clone(),
            ManifestCase::Generated(_) => manifest_bytes(&root, case.name),
        };
        let parse_result = musicpack_core::format::manifest::ParsedManifest::parse(&bytes);

        // Parse-level expectations (all platforms).
        match case.group {
            Group::Valid => {
                if let Err(e) = parse_result {
                    failures.push(format!("valid {}: unexpectedly rejected: {e}", case.name));
                }
            }
            Group::InvalidManifest if case.name != "waveform-points-mismatch" => {
                if parse_result.is_ok() {
                    failures.push(format!(
                        "invalid manifest {}: unexpectedly accepted",
                        case.name
                    ));
                }
            }
            Group::InvalidManifest => {
                // Rejected at the file level, not the manifest level.
                if let Err(e) = parse_result {
                    failures.push(format!(
                        "{}: should parse (rejection is file-level): {e}",
                        case.name
                    ));
                }
            }
            Group::InvalidVerify => {
                if let Err(e) = parse_result {
                    failures.push(format!(
                        "invalid verify {}: manifest should parse: {e}",
                        case.name
                    ));
                }
            }
        }

        // Verification-level expectations (the directory backend is the
        // reference adapter; unix only, like the reference's POSIX checks).
        #[cfg(unix)]
        {
            let pkg = root.join(format!("{}.mpack", case.name));
            let verified = musicpack_core::storage::directory::verify_directory(&pkg);
            match case.group {
                Group::Valid => match verified {
                    Ok(report) if report.is_ok() => {}
                    Ok(report) => failures.push(format!(
                        "valid {}: verify errors: {:?}",
                        case.name,
                        report
                            .findings()
                            .iter()
                            .map(|f| f.message.as_str())
                            .collect::<Vec<_>>()
                    )),
                    Err(e) => {
                        failures.push(format!("valid {}: verify failed to run: {e}", case.name))
                    }
                },
                Group::InvalidManifest | Group::InvalidVerify => {
                    let rejected = match verified {
                        Err(_) => true, // package open / parse failure
                        Ok(report) => !report.is_ok(),
                    };
                    if !rejected {
                        failures.push(format!(
                            "{}: verification unexpectedly accepted the package",
                            case.name
                        ));
                    }
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "conformance failures:\n{}",
        failures.join("\n")
    );
    #[cfg(unix)]
    eprintln!("conformance: 72 cases — parse + full verification, zero gaps");
    #[cfg(not(unix))]
    eprintln!("conformance: 72 cases — parse-level only (directory verification is unix-only)");

    let _ = std::fs::remove_dir_all(&root);
}
