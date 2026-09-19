//! CLI differential harness: exit-code and output compatibility against the
//! reference `musicpack` tool (when it is built).
//!
//! Machine-readable behaviour is what matters: for every case both tools'
//! exit statuses must agree, packed containers must be byte-identical, and
//! each tool must accept the other's containers. Human-readable text is
//! compared only where it is a documented compatibility surface (the
//! `checksum mismatch` / `missing file` / `exceeds` vocabulary).
//!
//! Discovery mirrors the core harness: `$MUSICPACK_REF_CLI`, else the
//! sibling reference checkout's build output. The reference repository is
//! never modified.
//!
//! Unix-only: every case exercises the directory-bundle adapter.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use musicpack_core::format::checksum;

fn rust_bin() -> &'static str {
    env!("CARGO_BIN_EXE_musicpack")
}

fn reference_cli() -> Option<PathBuf> {
    if let Ok(cli) = std::env::var("MUSICPACK_REF_CLI") {
        let path = PathBuf::from(cli);
        return if path.is_file() { Some(path) } else { None };
    }
    let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../musicpack");
    let cli = reference.join("build/core/musicpack/musicpack");
    if cli.is_file() { Some(cli) } else { None }
}

fn run(bin: &Path, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{bin:?} failed to run: {e}"))
}

fn exit(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("musicpack-cli-diff-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn write_package(root: &Path, tamper_asset: bool, unreferenced: bool) {
    fs::create_dir_all(root.join("audio")).unwrap();
    let content: &[u8] = b"audio-bytes";
    fs::write(root.join("audio/01.bin"), content).unwrap();
    if tamper_asset {
        fs::write(root.join("audio/01.bin"), b"tampered!!").unwrap();
    }
    if unreferenced {
        fs::create_dir_all(root.join("extras")).unwrap();
        fs::write(root.join("extras/loose.txt"), b"x").unwrap();
    }
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"Diff","artists":[{{"name":"T"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"t","audio":{{"path":"audio/01.bin","sha256":"{sha}"}}}}]}}]}}"#,
        sha = checksum::sha256_hex(content)
    );
    fs::write(root.join("manifest.json"), manifest).unwrap();
}

/// Compares both tools' exit codes for a command, asserting agreement and
/// returning the shared status.
fn compare(cli: &Path, case: &str, args_rust: &[&str], args_ref: &[&str]) -> i32 {
    let rust = run(Path::new(rust_bin()), args_rust);
    let reference = run(cli, args_ref);
    assert_eq!(
        exit(&rust),
        exit(&reference),
        "{case}: exit codes diverge\n  rust: {}\n  ref:  {}",
        text(&rust),
        text(&reference)
    );
    exit(&rust)
}

#[cfg(unix)]
#[test]
fn cli_behaviour_matches_the_reference() {
    let Some(cli) = reference_cli() else {
        eprintln!(
            "note: reference CLI not built; CLI differential skipped \
             (set MUSICPACK_REF_CLI or build ../musicpack)"
        );
        return;
    };
    let root = temp_dir("cases");
    fs::create_dir_all(&root).unwrap();

    // Valid package: info and verify both succeed.
    let valid = root.join("valid.mpack");
    write_package(&valid, false, false);
    assert_eq!(
        compare(
            &cli,
            "valid verify",
            &["verify", valid.to_str().unwrap()],
            &["verify", valid.to_str().unwrap()]
        ),
        0
    );
    assert_eq!(
        compare(
            &cli,
            "valid info",
            &["info", valid.to_str().unwrap()],
            &["info", valid.to_str().unwrap()]
        ),
        0
    );

    // Warnings are not failures.
    let warned = root.join("warned.mpack");
    write_package(&warned, false, true);
    assert_eq!(
        compare(
            &cli,
            "warn verify",
            &["verify", warned.to_str().unwrap()],
            &["verify", warned.to_str().unwrap()]
        ),
        0
    );

    // Verification failure.
    let bad = root.join("bad.mpack");
    write_package(&bad, true, false);
    assert_eq!(
        compare(
            &cli,
            "checksum verify",
            &["verify", bad.to_str().unwrap()],
            &["verify", bad.to_str().unwrap()]
        ),
        1
    );
    let rust = run(Path::new(rust_bin()), &["verify", bad.to_str().unwrap()]);
    assert!(text(&rust).contains("checksum mismatch"));

    // Malformed manifest.
    let malformed = root.join("malformed.mpack");
    fs::create_dir_all(&malformed).unwrap();
    fs::write(malformed.join("manifest.json"), b"{").unwrap();
    assert_eq!(
        compare(
            &cli,
            "malformed verify",
            &["verify", malformed.to_str().unwrap()],
            &["verify", malformed.to_str().unwrap()]
        ),
        1
    );

    // Pack: byte-identical outputs, and each tool accepts the other's.
    let rust_mpak = root.join("rust.mpak");
    let ref_mpak = root.join("ref.mpak");
    assert_eq!(
        compare(
            &cli,
            "pack",
            &["pack", valid.to_str().unwrap(), rust_mpak.to_str().unwrap()],
            &["pack", valid.to_str().unwrap(), ref_mpak.to_str().unwrap()]
        ),
        0
    );
    let rust_bytes = fs::read(&rust_mpak).unwrap();
    let ref_bytes = fs::read(&ref_mpak).unwrap();
    assert_eq!(
        rust_bytes, ref_bytes,
        "CLI pack output must be byte-identical"
    );

    // Cross-verification of packed containers.
    assert_eq!(
        compare(
            &cli,
            "verify rust mpak",
            &["verify", rust_mpak.to_str().unwrap()],
            &["verify", rust_mpak.to_str().unwrap()]
        ),
        0
    );
    assert_eq!(
        compare(
            &cli,
            "verify ref mpak",
            &["verify", ref_mpak.to_str().unwrap()],
            &["verify", ref_mpak.to_str().unwrap()]
        ),
        0
    );

    // Unpack both containers and compare the trees.
    let rust_unpack = root.join("unpack-rust");
    let ref_unpack = root.join("unpack-ref");
    assert_eq!(
        compare(
            &cli,
            "unpack ref mpak",
            &[
                "unpack",
                ref_mpak.to_str().unwrap(),
                rust_unpack.to_str().unwrap()
            ],
            &[
                "unpack",
                ref_mpak.to_str().unwrap(),
                ref_unpack.to_str().unwrap()
            ]
        ),
        0
    );
    assert_eq!(
        fs::read(rust_unpack.join("manifest.json")).unwrap(),
        fs::read(ref_unpack.join("manifest.json")).unwrap()
    );
    assert_eq!(
        fs::read(rust_unpack.join("audio/01.bin")).unwrap(),
        fs::read(ref_unpack.join("audio/01.bin")).unwrap()
    );

    // Corrupted container: both must agree (non-zero) and neither crashes.
    let mut corrupt = rust_bytes.clone();
    let halfway = corrupt.len() / 2;
    corrupt[halfway] ^= 0xFF;
    let corrupt_path = root.join("corrupt.mpak");
    fs::write(&corrupt_path, &corrupt).unwrap();
    let shared = compare(
        &cli,
        "corrupt verify",
        &["verify", corrupt_path.to_str().unwrap()],
        &["verify", corrupt_path.to_str().unwrap()],
    );
    assert_ne!(shared, 0, "a corrupted container must not verify");
    assert!(shared < 128, "must fail cleanly, not crash");

    fs::remove_dir_all(&root).ok();
}
