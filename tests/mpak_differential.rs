//! MPAK differential harness: container writing/reading vs the reference CLI.
//!
//! For each valid corpus case:
//!
//! 1. the reference CLI packs the directory bundle;
//! 2. the Rust packer packs the same bundle;
//! 3. both containers must be **byte-identical**;
//! 4. the Rust reader/verifier must accept the reference container;
//! 5. the reference CLI must `verify` and `unpack` the Rust container, and
//!    the unpacked tree must byte-match the source bundle.
//!
//! This is skipped with a notice when no reference binary is built (the
//! committed-fixture byte-identity test in `tests/mpak_compat.rs` always
//! runs). Unix-only: the directory source is the POSIX adapter.

#![cfg(unix)]

mod support;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use support::{reference_cli, write_corpus};

fn reference_pack_ok(cli: &Path, dir: &Path, out: &Path) -> bool {
    Command::new(cli)
        .arg("pack")
        .arg(dir)
        .arg(out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Recursively compares two directory trees (regular files only).
fn trees_match(a: &Path, b: &Path) -> Result<(), String> {
    let mut stack = vec![(a.to_path_buf(), b.to_path_buf())];
    while let Some((left, right)) = stack.pop() {
        let left_entries = fs::read_dir(&left).map_err(|e| format!("{left:?}: {e}"))?;
        let mut names: Vec<String> = left_entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let l = left.join(&name);
            let r = right.join(&name);
            let lmeta = fs::symlink_metadata(&l).map_err(|e| e.to_string())?;
            if lmeta.is_dir() {
                stack.push((l, r));
            } else if lmeta.is_file() {
                let lb = fs::read(&l).map_err(|e| e.to_string())?;
                let rb = fs::read(&r).map_err(|_| format!("missing in unpacked tree: {r:?}"))?;
                if lb != rb {
                    return Err(format!("content differs: {}", l.display()));
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn mpak_write_read_matches_the_reference_cli() {
    let Some(cli) = reference_cli() else {
        eprintln!(
            "note: reference CLI not built; MPAK differential run skipped \
             (committed-fixture byte-identity still runs in tests/mpak_compat.rs)"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "musicpack-core-mpak-differential-{}",
        std::process::id()
    ));
    let cases = write_corpus(&root).expect("corpus builds");

    // Valid cases with varied member shapes: minimal, waveforms,
    // representations and the full/complete manifest.
    let selected = [
        "minimal",
        "complete",
        "with-waveform",
        "with-representations",
    ];
    let mut checked = 0usize;

    for case in selected {
        assert!(
            cases.iter().any(|c| c.name == case),
            "corpus case '{case}' missing"
        );
        let dir = root.join(format!("{case}.mpack"));
        let reference_mpak = root.join(format!("{case}.reference.mpak"));
        assert!(
            reference_pack_ok(&cli, &dir, &reference_mpak),
            "{case}: reference pack failed"
        );

        // Rust pack must be byte-identical.
        let mut rust_bytes = Vec::new();
        musicpack_core::storage::directory::pack_directory_to_writer(&dir, &mut rust_bytes)
            .unwrap_or_else(|e| panic!("{case}: rust pack failed: {e}"));
        let reference_bytes = fs::read(&reference_mpak).expect("reference container");
        assert_eq!(
            rust_bytes, reference_bytes,
            "{case}: Rust container must be byte-identical to the reference pack"
        );

        // Rust reader/verifier accepts the reference container.
        let report = musicpack_core::storage::mpak::verify_mpak(Arc::new(
            musicpack_core::format::mpak::MemorySource::new(reference_bytes.clone()),
        ))
        .unwrap_or_else(|e| panic!("{case}: rust verify failed to open: {e}"));
        assert!(
            report.is_ok(),
            "{case}: rust verification of the reference container failed: {:?}",
            report
                .findings()
                .iter()
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>()
        );

        // Reference CLI verifies and unpacks the Rust container.
        let rust_mpak = root.join(format!("{case}.rust.mpak"));
        fs::write(&rust_mpak, &rust_bytes).expect("write rust container");
        let verify = Command::new(&cli)
            .arg("verify")
            .arg(&rust_mpak)
            .status()
            .expect("reference verify runs");
        assert!(
            verify.success(),
            "{case}: reference verify of Rust container failed"
        );

        let unpack_dir = root.join(format!("{case}.unpacked"));
        let unpack = Command::new(&cli)
            .arg("unpack")
            .arg(&rust_mpak)
            .arg(&unpack_dir)
            .status()
            .expect("reference unpack runs");
        assert!(
            unpack.success(),
            "{case}: reference unpack of Rust container failed"
        );
        trees_match(&dir, &unpack_dir).unwrap_or_else(|e| {
            panic!("{case}: unpacked tree differs from the source bundle: {e}")
        });

        checked += 1;
    }

    assert_eq!(checked, selected.len());
    eprintln!(
        "mpak differential: {checked} cases — Rust pack byte-identical, reference verify/unpack OK"
    );
    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn both_implementations_reject_packing_an_invalid_bundle() {
    let Some(cli) = reference_cli() else {
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "musicpack-core-mpak-diff-invalid-{}",
        std::process::id()
    ));
    let cases = write_corpus(&root).expect("corpus builds");
    assert!(cases.iter().any(|c| c.name == "checksum-mismatch"));
    let dir = root.join("checksum-mismatch.mpack");

    // The reference refuses to pack an unverified bundle...
    let reference_out = root.join("invalid.reference.mpak");
    assert!(!reference_pack_ok(&cli, &dir, &reference_out));

    // ...and so does the Rust packer.
    let mut bytes = Vec::new();
    assert!(
        musicpack_core::storage::directory::pack_directory_to_writer(&dir, &mut bytes).is_err(),
        "Rust must refuse to pack an unverified bundle"
    );
    let _ = fs::remove_dir_all(&root);
}
