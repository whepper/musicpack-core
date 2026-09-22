//! CLI integration tests: command surface, exit codes, pack/unpack round
//! trips, and the reference repository's hostile-package replay through the
//! real binary.
//!
//! The reference `tests/run_mpack_hostile.sh` cases are reproduced here
//! (FIFO asset, directory at an asset path, symlink escape, FIFO manifest,
//! hard link, oversized sparse file) with the same observable assertions:
//! a non-zero exit and, for the size case, an `exceeds` message.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use musicpack_core::format::checksum;

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_musicpack")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("the musicpack binary runs")
}

fn code(output: &Output) -> i32 {
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
    let dir = std::env::temp_dir().join(format!("musicpack-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Writes a minimal, fully valid one-track package into `root`.
fn write_min_package(root: &Path) {
    write_package_with(
        root,
        &[("audio/01.bin", b"one")],
        &[("extras/notes.txt", b"notes")],
    );
}

/// Writes a valid package with the given primary audio and extras.
fn write_package_with(root: &Path, audio: &[(&str, &[u8])], extras: &[(&str, &[u8])]) {
    fs::create_dir_all(root).unwrap();
    let mut tracks = Vec::new();
    for (i, (path, bytes)) in audio.iter().enumerate() {
        fs::create_dir_all(root.join(Path::new(path).parent().unwrap())).unwrap();
        fs::write(root.join(path), bytes).unwrap();
        tracks.push(format!(
            r#"{{"track": {n}, "title": "t", "audio": {{"path": "{path}", "sha256": "{sha}"}}}}"#,
            n = i + 1,
            sha = checksum::sha256_hex(bytes)
        ));
    }
    let mut extras_json = String::new();
    if !extras.is_empty() {
        let entries: Vec<String> = extras
            .iter()
            .map(|(path, bytes)| {
                fs::create_dir_all(root.join(Path::new(path).parent().unwrap())).unwrap();
                fs::write(root.join(path), bytes).unwrap();
                format!(
                    r#"{{"path": "{path}", "sha256": "{sha}"}}"#,
                    sha = checksum::sha256_hex(bytes)
                )
            })
            .collect();
        extras_json = format!(r#","extras":[{}]"#, entries.join(","));
    }
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"CLI Test","artists":[{{"name":"Tester"}}]}},"media":[{{"disc":1,"tracks":[{}]}}]{}}}"#,
        tracks.join(","),
        extras_json
    );
    fs::write(root.join("manifest.json"), manifest).unwrap();
}

// ---------------------------------------------------------------------
// command surface
// ---------------------------------------------------------------------

#[test]
fn usage_errors_exit_two() {
    assert_eq!(code(&run(&[])), 2);
    assert_eq!(code(&run(&["unknown"])), 2);
    assert_eq!(code(&run(&["info"])), 2);
    assert_eq!(code(&run(&["pack", "only-one"])), 2);
}

#[test]
fn missing_package_exits_one() {
    let missing = temp_dir("missing");
    let output = run(&["verify", missing.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("cannot open package"));
}

#[test]
fn malformed_manifest_is_rejected() {
    let dir = temp_dir("malformed");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("manifest.json"), b"{\"format\": ").unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    cleanup(&dir);
}

// ---------------------------------------------------------------------
// info / verify
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn info_reports_the_manifest_and_exits_zero() {
    let dir = temp_dir("info");
    write_min_package(&dir);
    let output = run(&["info", dir.to_str().unwrap()]);
    let out = text(&output);
    assert_eq!(code(&output), 0, "{out}");
    assert!(out.contains("Album: CLI Test"), "{out}");
    assert!(out.contains("Artist: Tester"), "{out}");
    assert!(out.contains("Integrity: OK"), "{out}");
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn verify_clean_package_exits_zero() {
    let dir = temp_dir("verify-ok");
    write_min_package(&dir);
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 0);
    assert!(text(&output).contains("0 error(s), 0 warning(s)"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn verify_checksum_mismatch_exits_one_with_the_vocabulary() {
    let dir = temp_dir("verify-mismatch");
    write_min_package(&dir);
    fs::write(dir.join("audio/01.bin"), b"changed").unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("checksum mismatch"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn verify_warnings_do_not_fail() {
    let dir = temp_dir("verify-warn");
    write_min_package(&dir);
    fs::write(dir.join("extras/unreferenced.txt"), b"x").unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    let out = text(&output);
    assert_eq!(code(&output), 0, "{out}");
    assert!(out.contains("unreferenced file"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn verify_json_shape() {
    let dir = temp_dir("verify-json");
    write_min_package(&dir);
    let output = run(&["verify", dir.to_str().unwrap(), "--json"]);
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(code(&output), 0);
    assert!(out.contains("\"ok\": true"), "{out}");
    assert!(out.contains("\"errors\": []"), "{out}");
    assert!(out.contains("\"warnings\": []"), "{out}");
    cleanup(&dir);
}

// ---------------------------------------------------------------------
// pack / unpack
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn pack_is_deterministic_and_unpack_round_trips() {
    let root = temp_dir("pack");
    let dir = root.join("pkg");
    write_min_package(&dir);
    let out_a = root.join("a.mpak");
    let out_b = root.join("b.mpak");
    let a = run(&["pack", dir.to_str().unwrap(), out_a.to_str().unwrap()]);
    let b = run(&["pack", dir.to_str().unwrap(), out_b.to_str().unwrap()]);
    assert_eq!(code(&a), 0, "{}", text(&a));
    assert_eq!(code(&b), 0, "{}", text(&b));
    let bytes_a = fs::read(&out_a).unwrap();
    let bytes_b = fs::read(&out_b).unwrap();
    assert_eq!(bytes_a, bytes_b, "packing must be deterministic");

    // The packed container verifies on its own.
    let verify = run(&["verify", out_a.to_str().unwrap()]);
    assert_eq!(code(&verify), 0, "{}", text(&verify));

    // Unpack and compare trees.
    let unpacked = root.join("unpacked");
    let unpack = run(&[
        "unpack",
        out_a.to_str().unwrap(),
        unpacked.to_str().unwrap(),
    ]);
    assert_eq!(code(&unpack), 0, "{}", text(&unpack));
    for file in ["manifest.json", "audio/01.bin", "extras/notes.txt"] {
        assert_eq!(
            fs::read(dir.join(file)).unwrap(),
            fs::read(unpacked.join(file)).unwrap(),
            "{file}"
        );
    }
    cleanup(&root);
}

#[test]
fn pack_refuses_existing_output_and_invalid_bundles() {
    let root = temp_dir("pack-refuse");
    let dir = root.join("pkg");
    write_min_package(&dir);
    let out = root.join("exists.mpak");
    fs::write(&out, b"occupied").unwrap();
    let output = run(&["pack", dir.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("already exists"));

    // An unverified bundle must not pack (the core gates packing).
    fs::write(dir.join("audio/01.bin"), b"tampered").unwrap();
    let out2 = root.join("bad.mpak");
    let output = run(&["pack", dir.to_str().unwrap(), out2.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(!out2.exists());
    cleanup(&root);
}

#[test]
fn unpack_reference_container_round_trips() {
    // Portable: the committed reference container (no directory backend).
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/reference-small.mpak");
    let dest = temp_dir("unpack-fixture");
    let output = run(&["unpack", fixture.to_str().unwrap(), dest.to_str().unwrap()]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert!(dest.join("manifest.json").is_file());
    assert_eq!(
        fs::read(dest.join("audio/01.bin")).unwrap(),
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/reference/mpak-source/audio/01.bin")
        )
        .unwrap()
    );
    cleanup(&dest);
}

#[test]
fn unpack_refuses_existing_destination() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/reference-small.mpak");
    let dest = temp_dir("unpack-exists");
    fs::create_dir_all(&dest).unwrap();
    let output = run(&["unpack", fixture.to_str().unwrap(), dest.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("already exists"));
    cleanup(&dest);
}

// ---------------------------------------------------------------------
// hostile replay (reference tests/run_mpack_hostile.sh)
// ---------------------------------------------------------------------

#[cfg(unix)]
fn hostile_manifest(path: &str, sha: &str) -> String {
    format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"T","audio":{{"path":"{path}","sha256":"{sha}"}}}}]}}]}}"#
    )
}

#[cfg(unix)]
#[test]
fn hostile_fifo_asset_is_rejected_without_blocking() {
    let dir = temp_dir("hostile-fifo");
    fs::create_dir_all(dir.join("audio")).unwrap();
    fs::write(
        dir.join("manifest.json"),
        hostile_manifest("audio/01.mpc", &"a".repeat(64)),
    )
    .unwrap();
    let Ok(status) = Command::new("mkfifo")
        .arg(dir.join("audio/01.mpc"))
        .status()
    else {
        eprintln!("note: mkfifo unavailable; FIFO case skipped");
        cleanup(&dir);
        return;
    };
    assert!(status.success());
    // Must return promptly with a failure (a read-open would hang forever).
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("missing file"));
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hostile_directory_asset_is_rejected() {
    let dir = temp_dir("hostile-dir");
    fs::create_dir_all(dir.join("audio/01.mpc")).unwrap();
    fs::write(
        dir.join("manifest.json"),
        hostile_manifest("audio/01.mpc", &"a".repeat(64)),
    )
    .unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hostile_symlink_escape_is_rejected() {
    let dir = temp_dir("hostile-symlink");
    fs::create_dir_all(dir.join("audio")).unwrap();
    let outside = dir.join("../outside.bin");
    fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("audio/01.mpc")).unwrap();
    fs::write(
        dir.join("manifest.json"),
        hostile_manifest("audio/01.mpc", &checksum::sha256_hex(b"outside")),
    )
    .unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("unsafe path"));
    let _ = fs::remove_file(&outside);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hostile_fifo_manifest_fails_to_open_without_blocking() {
    let dir = temp_dir("hostile-fifo-manifest");
    fs::create_dir_all(&dir).unwrap();
    let Ok(status) = Command::new("mkfifo")
        .arg(dir.join("manifest.json"))
        .status()
    else {
        eprintln!("note: mkfifo unavailable; FIFO manifest case skipped");
        cleanup(&dir);
        return;
    };
    assert!(status.success());
    let output = run(&["info", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hostile_hard_link_is_rejected() {
    let dir = temp_dir("hostile-hardlink");
    fs::create_dir_all(dir.join("audio")).unwrap();
    fs::write(dir.join("audio/01.mpc"), b"real content").unwrap();
    let alias = dir.join("../hlink_alias.mpc");
    fs::hard_link(dir.join("audio/01.mpc"), &alias).unwrap();
    // Real hash: the only reason verification may fail is the link count.
    fs::write(
        dir.join("manifest.json"),
        hostile_manifest("audio/01.mpc", &checksum::sha256_hex(b"real content")),
    )
    .unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("missing file"));
    let _ = fs::remove_file(&alias);
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn hostile_oversized_file_is_rejected_with_exceeds() {
    let dir = temp_dir("hostile-oversize");
    fs::create_dir_all(dir.join("audio")).unwrap();
    let file = fs::File::create(dir.join("audio/01.mpc")).unwrap();
    if file.set_len(9 * 1024 * 1024 * 1024).is_err() {
        eprintln!("note: sparse files unavailable; oversized case skipped");
        cleanup(&dir);
        return;
    }
    drop(file);
    fs::write(
        dir.join("manifest.json"),
        hostile_manifest("audio/01.mpc", &"a".repeat(64)),
    )
    .unwrap();
    let output = run(&["verify", dir.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(text(&output).contains("exceeds"), "{}", text(&output));
    cleanup(&dir);
}

#[test]
fn unpack_never_writes_outside_the_destination() {
    // A hand-built container whose member path attempts traversal is
    // rejected at scan time, so extraction never even starts.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MPAK");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    let mut payload = Vec::new();
    payload.extend_from_slice(&(9u16).to_be_bytes());
    payload.extend_from_slice(b"../evil.b");
    payload.extend_from_slice(b"boom");
    let container = dir_temp_container();
    let mut frame = Vec::new();
    frame.extend_from_slice(b"DATA");
    frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    let crc = musicpack_core::format::mpak::crc16_buypass(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame.extend_from_slice(&payload);
    bytes.extend_from_slice(&frame);

    let mut file = fs::File::create(&container).unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);

    let dest = temp_dir("unpack-traversal");
    let output = run(&[
        "unpack",
        container.to_str().unwrap(),
        dest.to_str().unwrap(),
    ]);
    // The invalid member is skipped (recovery-mode policy, like the
    // reference) and reported; nothing is ever written at or outside the
    // traversal path.
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(text(&output).contains("skipped"), "{}", text(&output));
    assert!(!container.parent().unwrap().join("evil.b").exists());
    if dest.exists() {
        for entry in fs::read_dir(&dest).unwrap().flatten() {
            assert_ne!(
                entry.file_name().to_string_lossy(),
                "evil.b",
                "traversal member must not be written"
            );
        }
    }
    let _ = fs::remove_file(&container);
    cleanup(&dest);
}

fn dir_temp_container() -> PathBuf {
    std::env::temp_dir().join(format!(
        "musicpack-cli-traversal-{}.mpak",
        std::process::id()
    ))
}
