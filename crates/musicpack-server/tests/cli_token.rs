//! End-to-end CLI tests: the `musicpack-server` binary's token surface,
//! `scan`/`verify`, and the `serve` lifecycle (stage 4: `serve` is live).

use std::path::PathBuf;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_musicpack-server");

fn temp_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cli-{0}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Runs the binary with a clean environment (no `MUSICPACK_*` leakage from
/// the developer's shell) and a per-test database.
fn run(args: &[&str], database: &PathBuf) -> (std::process::ExitStatus, String, String) {
    let output = Command::new(BINARY)
        .args(args)
        .arg("--database")
        .arg(database)
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_DATABASE")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .output()
        .unwrap();
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn token_create_list_revoke_round_trip() {
    let db = temp_db("roundtrip.db");

    // create: prints the secret exactly once in the reference's format.
    let (status, stdout, stderr) = run(&["token", "create", "--name", "Web"], &db);
    assert!(status.success(), "stderr: {stderr}");
    assert!(
        stdout.starts_with("Token created: Web (id 1)\n\n"),
        "unexpected create banner: {stdout}"
    );
    assert!(
        stdout.ends_with("This token will not be shown again.\n"),
        "unexpected create trailer: {stdout}"
    );
    let secret = stdout
        .lines()
        .find(|l| l.starts_with("mpk_"))
        .expect("exactly-once secret line missing")
        .to_string();
    assert_eq!(secret.len(), 4 + 43, "base64url body must be 43 chars");
    assert!(
        !stderr.contains(&secret),
        "the secret must never reach stderr"
    );

    // list: the reference's header + row + footer.
    let (status, stdout, stderr) = run(&["token", "list"], &db);
    assert!(status.success(), "stderr: {stderr}");
    let mut lines = stdout.lines();
    let header = lines.next().unwrap();
    // Derived from the C printf("%-4s  %-20s  %-22s  %-22s  %s") with the
    // reference's header words (verified character-by-character); identical
    // to `token list` on legacy output.
    assert_eq!(
        header,
        "id    name                  created                 last used               status"
    );
    let row = lines.next().unwrap();
    assert!(
        row.contains("   1") && row.contains("Web") && row.contains("active"),
        "unexpected row: {row}"
    );
    assert_eq!(lines.next(), Some("1 token(s)"));
    assert_eq!(lines.next(), None);

    // revoke: the reference's messages; a second revoke reports not-found.
    let (status, stdout, _) = run(&["token", "revoke", "1"], &db);
    assert!(status.success());
    assert_eq!(stdout.trim_end(), "Token 1 revoked.");
    let (status, stdout, _) = run(&["token", "revoke", "1"], &db);
    assert!(status.success(), "double revoke still exits 0 (reference)");
    assert_eq!(stdout.trim_end(), "Token 1 not found or already revoked.");

    // list reflects the revocation.
    let (_, stdout, _) = run(&["token", "list"], &db);
    assert!(stdout.contains("revoked") && stdout.contains("1 token(s)"));
}

#[test]
fn token_create_without_name_defaults_to_unnamed() {
    let db = temp_db("unnamed.db");
    let (status, stdout, stderr) = run(&["token", "create"], &db);
    assert!(status.success(), "stderr: {stderr}");
    assert!(stdout.starts_with("Token created: unnamed (id 1)"));
}

#[test]
fn scan_and_verify_run_end_to_end() {
    use std::process::Command;
    // `scan` on an empty library succeeds with the reference summary.
    let db = temp_db("scan-empty.db");
    let lib = db.parent().unwrap().join("empty-lib");
    std::fs::create_dir_all(&lib).unwrap();
    let output = Command::new(BINARY)
        .args(["scan", "--library"])
        .arg(&lib)
        .arg("--database")
        .arg(&db)
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_DATABASE")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scan: 0 packages (0 added, 0 updated, 0 moved, 0 removed, 0 invalid)"),
        "{stdout}"
    );
}

#[test]
fn serve_starts_and_answers_health() {
    // `serve` is live since stage 4: it binds, serves the read-only API,
    // and blocks until killed. Spawn it on a loopback port, poll health
    // until ready, then terminate the child.
    use std::io::{Read, Write};

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cli-{0}-serve", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("serve.db");
    // `--no-scan` requires an existing database (like the reference), so
    // create one first via any store-opening command.
    let (status, _, stderr) = run(&["token", "create", "--name", "Serve"], &db);
    assert!(status.success(), "stderr: {stderr}");
    // Pick a free loopback port by binding `:0` first.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(BINARY)
        .args([
            "serve",
            "--database",
            db.to_str().unwrap(),
            "--listen",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--no-scan",
        ])
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_DATABASE")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let health = b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    let mut ready = false;
    for _ in 0..150 {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            if stream.write_all(health).is_ok() {
                let mut buf = Vec::new();
                if stream.read_to_end(&mut buf).is_ok()
                    && buf.starts_with(b"HTTP/1.1 200 OK")
                    && String::from_utf8_lossy(&buf).contains(r#""status":"ok""#)
                {
                    ready = true;
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(ready, "serve did not answer health on 127.0.0.1:{port}");
}

#[test]
fn usage_outcomes_match_the_reference_exit_codes() {
    let db = temp_db("usage.db");
    // help/version exit 0.
    let (status, _, _) = run(&["help"], &db);
    assert_eq!(status.code(), Some(0));
    let (status, stdout, _) = run(&["version"], &db);
    assert_eq!(status.code(), Some(0));
    assert!(stdout.starts_with("musicpack-server "));

    // Unknown command and unknown token subcommand exit 2.
    let (status, _, stderr) = run(&["frobnicate"], &db);
    assert_eq!(status.code(), Some(2));
    assert!(stderr.contains("unknown command 'frobnicate'"));
    let (status, _, stderr) = run(&["token", "explode"], &db);
    assert_eq!(status.code(), Some(2));
    assert!(stderr.contains("unknown token subcommand 'explode'"));

    // Invalid port exits 2.
    let (status, _, stderr) = run(&["serve", "--port", "0"], &db);
    assert_eq!(status.code(), Some(2));
    assert!(stderr.contains("invalid port 0"));
}

#[test]
fn environment_configuration_is_honored() {
    let db = temp_db("env.db");
    let dir = db.parent().unwrap().join("env-override.db");
    let output = Command::new(BINARY)
        .args(["token", "list"])
        .env("MUSICPACK_DATABASE", &dir)
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0 token(s)"), "{stdout}");
    assert!(dir.is_file(), "MUSICPACK_DATABASE must select the database");
}

#[test]
fn flags_override_environment_values() {
    // R4.5 config audit: precedence is flags > environment > defaults.
    let root = temp_db("precedence.db").parent().unwrap().to_path_buf();
    let env_db = root.join("precedence-env.db");
    let flag_db = root.join("precedence-flag.db");
    let _ = std::fs::remove_file(&env_db);
    let _ = std::fs::remove_file(&flag_db);
    let output = Command::new(BINARY)
        .args(["token", "list", "--database", flag_db.to_str().unwrap()])
        .env("MUSICPACK_DATABASE", &env_db)
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .env_remove("MUSICPACK_SHUTDOWN_FILE")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(flag_db.is_file(), "the flag selects the database");
    assert!(
        !env_db.exists(),
        "the environment value must lose to an explicit flag"
    );
}
