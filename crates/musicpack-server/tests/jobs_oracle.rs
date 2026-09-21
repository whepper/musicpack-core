//! Library job oracle: `POST /api/v1/library/scan|verify` and
//! `GET /api/v1/library/status` must behave like the legacy C server —
//! 202 + fresh snapshot on start, a single-slot job running on its own
//! database connection, live/final counters in the status snapshot, and
//! identical database effects (verify verdicts persisted per package).
//!
//! Fixture: one healthy package, one with an unreferenced extra file
//! (verify → warning), one with a missing audio file (verify →
//! checksum-failed), one malformed (invalid — excluded from jobs).
//! The setup scan is lightweight (statuses `unverified`), so the verify
//! job observably upgrades every verdict. Requires
//! `MUSICPACK_LEGACY_SERVER`; without it the C side is skipped with a
//! notice and Rust-side assertions still run.

mod util;

use std::path::PathBuf;

use musicpack_core::format::checksum;
use musicpack_server::ingest::scan;
use musicpack_server::store::sqlite::SqliteStore;

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("jobs-{0}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(dir: &std::path::Path, rel: &str, content: &[u8]) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    checksum::sha256_hex(content)
}

fn real_mpc() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/musepack/sine44-q5.mpc"
    ))
    .unwrap()
}

/// good (verify→valid) + warn (unreferenced extra → warning) + thin
/// (missing audio → checksum-failed) + broken (invalid, excluded).
fn build_library(lib: &std::path::Path) {
    let good = lib.join("good.mpack");
    let mpc = write_file(&good, "audio/01.mpc", &real_mpc());
    let wfm = write_file(&good, "waveform/01.wfm", &[0x80u8; 20]);
    std::fs::write(
        good.join("manifest.json"),
        format!(
            concat!(
                r#"{{"format":"musicpack","version":1,"#,
                r#""album":{{"title":"Jobs","artists":[{{"name":"Jobs Artist"}}]}},"#,
                r#""media":[{{"disc":1,"tracks":[{{"track":1,"title":"T","#,
                r#""audio":{{"path":"audio/01.mpc","sha256":"{mpc}"}},
                "waveform":{{"version":1,"path":"waveform/01.wfm","sha256":"{wfm}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":10}}}}]}}]}}"#
            ),
            mpc = mpc,
            wfm = wfm,
        )
        .replace('\n', ""),
    )
    .unwrap();

    let warn = lib.join("warn.mpack");
    let wmpc = write_file(&warn, "audio/01.mpc", &real_mpc());
    std::fs::write(warn.join("unreferenced.txt"), b"extra bytes").unwrap();
    std::fs::write(
        warn.join("manifest.json"),
        format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"Warn","artists":[{{"name":"Jobs Artist"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"W","audio":{{"path":"audio/01.mpc","sha256":"{wmpc}"}}}}]}}]}}"#
        ),
    )
    .unwrap();

    let thin = lib.join("thin.mpack");
    std::fs::create_dir_all(thin.join("audio")).unwrap();
    std::fs::write(
        thin.join("manifest.json"),
        r#"{"format":"musicpack","version":1,"album":{"title":"Thin","artists":[{"name":"Jobs Artist"}]},"media":[{"disc":1,"tracks":[{"track":1,"title":"T","audio":{"path":"audio/01.mpc","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}]}]}"#,
    )
    .unwrap();

    let broken = lib.join("broken.mpack");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("manifest.json"), b"{not json").unwrap();
}

struct Env {
    pair: util::Pair,
    token: String,
    db_r: PathBuf,
    db_c: PathBuf,
}

fn setup(name: &str) -> Env {
    let root = test_root(name);
    let lib = root.join("lib");
    build_library(&lib);
    let db = root.join("base.db");
    {
        let mut store = SqliteStore::open(&db).unwrap();
        let result = scan(&mut store, &lib, false).unwrap();
        assert_eq!(result.total, 4, "fixture shape changed");
    }
    // One bearer token, hashed into the shared base before the copies.
    let secret = musicpack_server::tokens::generate_secret().unwrap();
    let hash = musicpack_server::tokens::hash_secret(&secret);
    {
        let mut store = SqliteStore::open(&db).unwrap();
        use musicpack_server::store::Store;
        store.create_token("Jobs", &hash).unwrap();
    }
    for suffix in ["-wal", "-shm"] {
        assert!(
            !root.join(format!("base.db{suffix}")).exists(),
            "WAL sidecar"
        );
    }
    let db_r = root.join("r.db");
    let db_c = root.join("c.db");
    std::fs::copy(&db, &db_r).unwrap();
    std::fs::copy(&db, &db_c).unwrap();
    let pair = util::spawn_pair(&lib, &db_r, Some(&db_c), &[]);
    Env {
        pair,
        token: secret,
        db_r,
        db_c,
    }
}

fn authed(env: &Env) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), format!("Bearer {}", env.token))]
}

fn get(env: &Env, legacy: bool, path: &str) -> util::Resp {
    let port = if legacy {
        env.pair.legacy.as_ref().unwrap().port
    } else {
        env.pair.rust.port
    };
    let binding = authed(env);
    let headers = binding
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect::<Vec<_>>();
    util::raw_request(port, "GET", path, &headers)
}

fn post(env: &Env, legacy: bool, path: &str) -> util::Resp {
    let port = if legacy {
        env.pair.legacy.as_ref().unwrap().port
    } else {
        env.pair.rust.port
    };
    let binding = authed(env);
    let headers = binding
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect::<Vec<_>>();
    util::raw_request(port, "POST", path, &headers)
}

/// Splits the status snapshot into its two blocks (the JSON shape is
/// `{"scan":{…},"verify":{…}}`).
fn split_blocks(body: &str) -> (&str, &str) {
    body.split_once(",\"verify\":").unwrap_or((body, ""))
}

fn is_iso_stamp(stamp: &str) -> bool {
    stamp.len() == 20
        && stamp.as_bytes()[4] == b'-'
        && stamp.as_bytes()[7] == b'-'
        && stamp.as_bytes()[10] == b'T'
        && stamp.as_bytes()[13] == b':'
        && stamp.as_bytes()[16] == b':'
        && stamp.ends_with('Z')
}

/// Waits until the status snapshot reports no running job (both blocks
/// idle), then returns the final body.
fn wait_done(env: &Env, legacy: bool, _block: &str) -> String {
    for _ in 0..300 {
        let body = get(env, legacy, "/api/v1/library/status").text();
        if !body.contains("\"running\":1") {
            return body;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!(
        "job on {} never finished",
        if legacy { "C" } else { "Rust" }
    );
}

fn compare_job_counters(_env: &Env, final_r: &str, final_c: &str, keys: &[&str]) {
    for key in keys {
        assert_eq!(
            util::json_int(final_r, key),
            util::json_int(final_c, key),
            "counter {key} differs between implementations"
        );
    }
}

#[test]
fn scan_job_lifecycle_and_counters_match_on_both() {
    let env = setup("scan");
    // Unauthenticated job APIs are behind the auth gate on both.
    for server in [false, true] {
        if server && env.pair.legacy.is_none() {
            continue;
        }
        let r = {
            let port = if server {
                env.pair.legacy.as_ref().unwrap().port
            } else {
                env.pair.rust.port
            };
            util::raw_request(port, "POST", "/api/v1/library/scan", &[])
        };
        assert_eq!(r.status, 401, "unauthenticated scan must 401");
    }
    // Start on both.
    let r202 = post(&env, false, "/api/v1/library/scan");
    assert_eq!(r202.status, 202, "scan start");
    assert!(
        r202.text().starts_with("{\"scan\":{"),
        "202 carries the snapshot"
    );
    // The start response is a complete status snapshot (same renderer as
    // GET /library/status).
    assert_eq!(
        r202.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    let c202 = env
        .pair
        .legacy
        .is_some()
        .then(|| post(&env, true, "/api/v1/library/scan"));
    if let Some(c) = &c202 {
        assert_eq!(c.status, 202, "legacy scan start");
    }
    // Wait for both to finish, then compare finals.
    let final_r = wait_done(&env, false, "scan");
    assert_eq!(util::json_int(&final_r, "failed"), 0, "scan job succeeded");
    let (scan_block, _) = split_blocks(&final_r);
    assert!(
        is_iso_stamp(util::json_str(scan_block, "finishedAt")),
        "finishedAt stamped: {}",
        util::json_str(scan_block, "finishedAt")
    );
    if c202.is_some() {
        let final_c = wait_done(&env, true, "scan");
        compare_job_counters(
            &env,
            &final_r,
            &final_c,
            &[
                "packagesScanned",
                "added",
                "updated",
                "removed",
                "invalid",
                "failed",
            ],
        );
    }
    // The slot is released: a second start succeeds on both.
    assert_eq!(post(&env, false, "/api/v1/library/scan").status, 202);
    if env.pair.legacy.is_some() {
        assert_eq!(post(&env, true, "/api/v1/library/scan").status, 202);
        wait_done(&env, false, "scan");
        wait_done(&env, true, "scan");
    }
}

#[test]
fn verify_job_updates_statuses_identically() {
    let env = setup("verify");
    let r202 = post(&env, false, "/api/v1/library/verify");
    assert_eq!(r202.status, 202, "verify start");
    let has_legacy = env.pair.legacy.is_some();
    if has_legacy {
        assert_eq!(post(&env, true, "/api/v1/library/verify").status, 202);
    }
    let final_r = wait_done(&env, false, "verify");
    // Rust pins: 3 candidates (broken is invalid and excluded), one each
    // passed / warnings / failed, and the job itself succeeded.
    assert_eq!(util::json_int(&final_r, "packagesVerified"), 3);
    assert_eq!(util::json_int(&final_r, "passed"), 1);
    assert_eq!(util::json_int(&final_r, "warnings"), 1);
    let (scan_block, verify_block) = split_blocks(&final_r);
    assert_eq!(util::json_int(verify_block, "failed"), 1);
    assert_eq!(util::json_int(verify_block, "jobFailed"), 0);
    assert!(is_iso_stamp(util::json_str(verify_block, "finishedAt")));
    assert!(scan_block.contains("\"running\":0"), "scan block idle");
    if has_legacy {
        let final_c = wait_done(&env, true, "verify");
        compare_job_counters(
            &env,
            &final_r,
            &final_c,
            &[
                "packagesVerified",
                "passed",
                "warnings",
                "failed",
                "jobFailed",
            ],
        );
    }
    // Database effects: the verdict columns must match row-for-row.
    let dump = |db: &std::path::Path| {
        let conn = rusqlite::Connection::open(db).unwrap();
        let mut stmt = conn
            .prepare("SELECT id, status, verify_status FROM packages ORDER BY id")
            .unwrap();
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows
    };
    let rows_r = dump(&env.db_r);
    assert_eq!(rows_r.len(), 4);
    if has_legacy {
        let rows_c = dump(&env.db_c);
        assert_eq!(rows_r, rows_c, "verify verdicts differ between databases");
    }
    // Independent Rust pins (order by id == fixture creation order is not
    // guaranteed; assert the multiset of verdicts instead).
    let mut verdicts: Vec<(String, String)> = rows_r
        .iter()
        .map(|(_, s, v)| (s.clone(), v.clone()))
        .collect();
    verdicts.sort();
    let expected: Vec<(String, String)> = [
        ("checksum-failed", "checksum-failed"),
        ("invalid", "unverified"),
        ("valid", "valid"),
        ("warning", "warning"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(verdicts, expected, "verify upgraded every verdict");
}

#[test]
fn status_endpoint_is_authed_and_shape_stable() {
    let env = setup("status");
    // Unauthenticated status is 401 on both (the C auth gate precedes it).
    let port = env.pair.rust.port;
    let r = util::raw_request(port, "GET", "/api/v1/library/status", &[]);
    assert_eq!(r.status, 401);
    if let Some(c) = env.pair.legacy.as_ref() {
        let l = util::raw_request(c.port, "GET", "/api/v1/library/status", &[]);
        assert_eq!(l.status, 401);
    }
    // Authenticated idle snapshot: zeros and empty stamps (also compared
    // byte-for-byte against the C by api_oracle).
    let body = get(&env, false, "/api/v1/library/status").text();
    assert_eq!(
        body,
        r#"{"scan":{"running":0,"startedAt":"","finishedAt":"","packagesScanned":0,"added":0,"updated":0,"removed":0,"invalid":0,"failed":0},"verify":{"running":0,"startedAt":"","finishedAt":"","packagesVerified":0,"passed":0,"warnings":0,"failed":0,"jobFailed":0}}"#
    );
    // Method discipline: GET on the start endpoints 405s on both.
    assert_eq!(get(&env, false, "/api/v1/library/scan").status, 405);
    assert_eq!(get(&env, false, "/api/v1/library/verify").status, 405);
    if let Some(c) = env.pair.legacy.as_ref() {
        let headers = authed(&env);
        let hdr: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            util::raw_request(c.port, "GET", "/api/v1/library/scan", &hdr).status,
            405
        );
    }
}
