// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

//! R4.5 graceful-shutdown tests.
//!
//! The server is std-only (ADR 0001) and signal-free (ADR 0009), so R4.5 adds
//! an in-process drain token plus an opt-in `--shutdown-file` trigger. These
//! tests pin the deterministic behaviour:
//!
//! - a cancelled scan/verify stops at a package boundary and never runs the
//!   "unavailable" sweep (which would orphan not-yet-processed packages);
//! - a shutdown cancels the background job, releases the single slot and is
//!   not reported as a failure;
//! - `serve_with_shutdown` stops accepting and waits for in-flight
//!   connections before returning;
//! - the real binary drains on the shutdown file and exits 0.

mod util;

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use musicpack_core::format::checksum;
use musicpack_server::config::Config;
use musicpack_server::http::routes::Context;
use musicpack_server::jobs::{self, JobKind};
use musicpack_server::shutdown::Shutdown;
use musicpack_server::store::sqlite::SqliteStore;

fn test_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shutdown-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mpc_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/musepack/sine44-q5.mpc"
    ))
    .unwrap()
}

/// Writes a minimal valid one-track package under `lib/<name>`. The album
/// title derives from `name` so packages are not fingerprint-identical
/// (identical fingerprints are mirrors/moves in the ingestion model).
fn build_package(lib: &Path, name: &str) -> PathBuf {
    let dir = lib.join(name);
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    let bytes = mpc_bytes();
    std::fs::write(dir.join("audio/01.mpc"), &bytes).unwrap();
    let sha = checksum::sha256_hex(&bytes);
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Shutdown Test {title}","artists":[{{"name":"Tester"}}],"releaseType":"album"}},"#,
            r#""media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"audio/01.mpc","sha256":"{sha}"}}}}]}}]}}"#
        ),
        title = name.trim_end_matches(".mpack"),
        sha = sha
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

fn open_store(db: &Path) -> SqliteStore {
    SqliteStore::open(db).expect("open store")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn package_status(db: &Path, name: &str) -> (String, String) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT status, verify_status FROM packages WHERE path LIKE ?1",
        rusqlite::params![format!("%{name}")],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .unwrap()
}

// ---------------------------------------------------------------------
// cancellation semantics
// ---------------------------------------------------------------------

#[test]
fn cancelled_scan_stops_before_the_sweep_and_keeps_committed_state() {
    let root = test_root("scan-cancel");
    let lib = root.join("lib");
    let db = root.join("library.db");
    std::fs::create_dir_all(&lib).unwrap();
    // A package that exists in the database but not on disk afterwards: a
    // completed scan would sweep it to `unavailable`.
    build_package(&lib, "gone.mpack");
    {
        let mut store = open_store(&db);
        musicpack_server::ingest::scan(&mut store, &lib, false).unwrap();
    }
    assert_eq!(package_status(&db, "gone.mpack").0, "valid");

    // Replace it on disk with a different package, then scan with a callback
    // that cancels immediately.
    std::fs::remove_dir_all(lib.join("gone.mpack")).unwrap();
    build_package(&lib, "fresh.mpack");
    let mut store = open_store(&db);
    let mut calls = 0usize;
    let result = musicpack_server::ingest::scan_with_progress(&mut store, &lib, false, &mut |_| {
        calls += 1;
        false // cancel at the first boundary
    })
    .unwrap();
    assert!(result.cancelled, "scan reports the clean cancel");
    assert_eq!(calls, 1, "stopped at the first package boundary");
    assert_eq!(result.removed, 0, "the sweep did not run");
    // The absent package was NOT swept to unavailable, and the fresh one was
    // ingested (committed prefix retained).
    assert_eq!(
        package_status(&db, "gone.mpack").0,
        "valid",
        "a cancelled scan must not orphan not-yet-processed packages"
    );
    assert_eq!(package_status(&db, "fresh.mpack").0, "valid");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_verify_stops_at_a_boundary() {
    let root = test_root("verify-cancel");
    let lib = root.join("lib");
    let db = root.join("library.db");
    std::fs::create_dir_all(&lib).unwrap();
    build_package(&lib, "a.mpack");
    build_package(&lib, "b.mpack");
    {
        let mut store = open_store(&db);
        musicpack_server::ingest::scan(&mut store, &lib, false).unwrap();
    }

    let mut store = open_store(&db);
    let mut calls = 0usize;
    let result = musicpack_server::ingest::verify_library(&mut store, &lib, &mut |_| {
        calls += 1;
        false
    })
    .unwrap();
    assert!(result.cancelled, "verify reports the clean cancel");
    assert_eq!(calls, 1, "stopped after the first verdict, not the second");
    assert_eq!(result.total, 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shutdown_cancels_the_job_and_releases_the_slot_without_failing() {
    let root = test_root("job-cancel");
    let lib = root.join("lib");
    let db = root.join("library.db");
    std::fs::create_dir_all(&lib).unwrap();
    build_package(&lib, "a.mpack");
    build_package(&lib, "b.mpack");

    let mut cfg = Config::defaults();
    cfg.library = lib.to_str().unwrap().into();
    cfg.database = db.to_str().unwrap().into();

    let jobs = jobs::shared();
    let shutdown = Shutdown::new();
    // Request before starting: the worker's first progress check cancels.
    shutdown.request();
    assert!(jobs::start(&jobs, &cfg, JobKind::Scan, shutdown.clone()));

    // The slot must be released promptly (drain window), and a cancelled job
    // is not a failure.
    let deadline = Instant::now() + Duration::from_secs(10);
    while jobs::lock(&jobs).running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = jobs::lock(&jobs).clone();
    assert!(!st.running, "cancelled job released the slot");
    assert_eq!(st.failed, 0, "a shutdown cancel is not a failure");
    assert!(jobs::wait_idle(&jobs, Duration::from_secs(1)));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------
// connection drain
// ---------------------------------------------------------------------

#[test]
fn serve_drains_an_in_flight_connection_before_returning() {
    let root = test_root("drain");
    let db = root.join("library.db");
    let store = open_store(&db);
    let mut cfg = Config::defaults();
    cfg.database = db.to_str().unwrap().into();
    cfg.library = root.join("lib").to_str().unwrap().into();
    let shutdown = Shutdown::new();
    let ctx = Arc::new(Context {
        config: cfg,
        store: Mutex::new(store),
        jobs: jobs::shared(),
        shutdown: shutdown.clone(),
    });

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_shutdown = shutdown.clone();
    let server = std::thread::spawn(move || {
        musicpack_server::http::serve_with_shutdown(listener, ctx, server_shutdown)
    });

    // A normal request completes while the server is accepting.
    util::wait_ready(port);

    // Hold a connection open mid-request (no terminating blank line): its
    // handler thread is parked in `read`, so it is in-flight work.
    let mut held = TcpStream::connect(("127.0.0.1", port)).unwrap();
    held.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(shutdown.in_flight(), 1, "the held connection is tracked");

    // Ask for shutdown: the accept loop stops, but the drain must wait.
    shutdown.request();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !server.is_finished(),
        "serve waits for the in-flight request"
    );

    // Releasing the client lets the handler finish; the drain then completes.
    drop(held);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !server.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(server.is_finished(), "serve returned after the drain");
    server.join().unwrap().unwrap();
    assert_eq!(shutdown.in_flight(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------
// the real binary: --shutdown-file drains and exits 0
// ---------------------------------------------------------------------

#[test]
fn binary_drains_on_the_shutdown_file_and_exits_zero() {
    use std::process::{Command, Stdio};

    let root = test_root("binary");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let stop = root.join("stop.flag");
    std::fs::create_dir_all(&lib).unwrap();

    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_musicpack-server"))
        .args([
            "serve",
            "--library",
            lib.to_str().unwrap(),
            "--database",
            db.to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--shutdown-file",
            stop.to_str().unwrap(),
        ])
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_DATABASE")
        .env_remove("MUSICPACK_PORT")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    util::wait_ready(port);

    // Trigger the graceful stop.
    std::fs::write(&stop, b"").unwrap();

    // Bounded wait for a clean exit.
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("server did not exit after the shutdown file appeared");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "graceful shutdown exits 0: {status:?}");
    assert!(!stop.exists(), "the trigger file is consumed");
    let _ = std::fs::remove_dir_all(&root);
}
