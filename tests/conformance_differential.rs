//! Differential harness: the Rust implementation vs the reference CLI.
//!
//! Runs the authoritative corpus through both implementations and compares
//! the accept/reject decisions:
//!
//! ```text
//!                 same corpus
//!                     │
//!             ┌───────┴────────┐
//!             ▼                ▼
//!    reference `musicpack`   musicpack-core
//!      info / verify          parse + verify (directory backend)
//!             │                │
//!             └───────┬────────┘
//!                     ▼
//!              compare outcomes
//! ```
//!
//! The reference CLI's `info` also runs verification (it reports integrity),
//! so for every case both `info` and `verify` must agree with the Rust
//! outcome. The reference binary is discovered via `support::reference_cli`:
//! `$MUSICPACK_REF_CLI`, else `<reference>/build/core/musicpack/musicpack`,
//! where the reference directory is `$MUSICPACK_REFERENCE_DIR` or the
//! sibling `../musicpack`. To build it once:
//!
//! ```sh
//! cmake -S ../musicpack -B ../musicpack/build \
//!       -DMPC_BUILD_TESTS=OFF -DMPC_BUILD_SERVER=OFF \
//!       -DMPC_BUILD_MPCGAIN=OFF -DMPC_BUILD_MPCCHAP=OFF
//! cmake --build ../musicpack/build --target musicpack_cmd -j
//! ```
//!
//! When no reference binary is available this test skips with a notice —
//! the primary corpus gate (`tests/conformance_corpus.rs`) always runs.
//! The reference checkout is never modified: the corpus is generated into
//! a per-process temporary directory.
//!
//! The Rust verifier's directory backend is unix-only (the reference's
//! POSIX hardening: link-count rejection, inode dedup, containment), so the
//! comparison runs on unix; Windows semantics genuinely differ there and a
//! Windows adapter is future work.

mod support;

use std::process::Command;

use support::write_corpus;

#[test]
fn differential_against_the_reference_cli() {
    let Some(cli) = support::reference_cli() else {
        eprintln!(
            "note: reference CLI not built; differential run skipped. \
             Build it with: cmake -S ../musicpack -B ../musicpack/build \
             -DMPC_BUILD_TESTS=OFF -DMPC_BUILD_SERVER=OFF -DMPC_BUILD_MPCGAIN=OFF \
             -DMPC_BUILD_MPCCHAP=OFF && cmake --build ../musicpack/build \
             --target musicpack_cmd -j   (or set MUSICPACK_REF_CLI)"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "musicpack-core-differential-{}",
        std::process::id()
    ));
    let cases = write_corpus(&root).expect("corpus builds");
    assert_eq!(cases.len(), 72);

    #[cfg(unix)]
    {
        let mut mismatches = Vec::new();
        for case in &cases {
            let pkg = root.join(format!("{}.mpack", case.name));

            let rust_ok = musicpack_core::storage::directory::verify_directory(&pkg)
                .map(|report| report.is_ok())
                .unwrap_or(false);

            for verb in ["info", "verify"] {
                let output = Command::new(&cli)
                    .arg(verb)
                    .arg(&pkg)
                    .output()
                    .unwrap_or_else(|e| panic!("reference CLI failed to run: {e}"));
                let reference_ok = output.status.success();
                if reference_ok != rust_ok {
                    mismatches.push(format!(
                        "{} ({verb}): reference {} but Rust {}",
                        case.name,
                        if reference_ok { "accepts" } else { "rejects" },
                        if rust_ok { "accepts" } else { "rejects" }
                    ));
                }
            }
        }

        assert!(
            mismatches.is_empty(),
            "{} cases compared (info + verify); divergences:\n{}",
            cases.len(),
            mismatches.join("\n")
        );
        eprintln!(
            "differential: {} cases x (info, verify) match the reference CLI — zero gaps",
            cases.len()
        );
    }

    #[cfg(not(unix))]
    {
        let _ = cli;
        eprintln!(
            "note: directory verification is unix-only; differential verification \
             skipped on this platform (the reference's Windows checks differ)"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
