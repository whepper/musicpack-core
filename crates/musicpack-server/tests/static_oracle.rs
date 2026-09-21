//! Static hosting oracle: with `--static-dir`, the Rust server must
//! answer exactly like the legacy C server — same files, same MIMEs,
//! `Cache-Control: no-cache`, the cross-origin isolation pair, the SPA
//! fallback rules (GET-only, extension-less), the bare static 404, and
//! the strict `/api/` reservation.
//!
//! Fixture: a small static tree (html/js/css/json/ico/ts/woff2/svg,
//! a nested asset, an extension-less file) plus traversal and symlink
//! probes. Compared per request: status, contract headers and full body
//! bytes. Requires `MUSICPACK_LEGACY_SERVER`; without it the C side is
//! skipped with a notice and Rust-side assertions still run.

mod util;

use std::path::{Path, PathBuf};

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("static-{0}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type StaticFiles = Vec<(&'static str, &'static [u8])>;

/// The static tree (served verbatim, so the bytes are the expectation).
fn build_static(root: &Path) -> StaticFiles {
    std::fs::create_dir_all(root.join("assets")).unwrap();
    let files: StaticFiles = vec![
        (
            "index.html",
            b"<!doctype html><html><body>musicpack shell</body></html>\n",
        ),
        ("app.js", b"console.log('musicpack');\n"),
        ("style.css", b"body { color: #f1eee7 }\n"),
        ("assets/app.css", b".deep { color: gold }\n"),
        ("data.json", b"{\"build\":\"oracle\"}\n"),
        ("favicon.ico", b"\x00\x00\x01\x00\x01\x00"),
        (
            "worklet.ts",
            b"// worklet source served as text/javascript\n",
        ),
        ("font.woff2", b"wOF2fake"),
        ("logo.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        ("README", b"extension-less file\n"),
    ];
    for (rel, bytes) in &files {
        std::fs::write(root.join(rel), bytes).unwrap();
    }
    files
}

struct Env {
    pair: util::Pair,
    files: StaticFiles,
    static_root: PathBuf,
}

fn setup(name: &str) -> Env {
    let root = test_root(name);
    let files = build_static(&root);
    let static_string = root.to_str().unwrap().to_string();
    let db_r = root.join("r.db");
    let db_c = root.join("c.db");
    // Any valid database works; byte serving itself is covered by
    // media_oracle. Create one empty database and copy it per server.
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    {
        let store = musicpack_server::store::sqlite::SqliteStore::open(&db_r).unwrap();
        use musicpack_server::store::Store;
        store.health_schema_version();
    }
    std::fs::copy(&db_r, &db_c).unwrap();
    let pair = util::spawn_pair(&lib, &db_r, Some(&db_c), &["--static-dir", &static_string]);
    Env {
        pair,
        files,
        static_root: root,
    }
}

fn body_of(files: &StaticFiles, rel: &str) -> &'static [u8] {
    files
        .iter()
        .find(|(name, _)| *name == rel)
        .map(|(_, bytes)| *bytes)
        .unwrap()
}

/// Compares a 200 file response: status, full contract header set
/// (content-type/cache-control/COOP/COEP) and exact body bytes.
fn compare_file(env: &Env, label: &str, method: &str, path: &str, expect_rel: &str) {
    let r = util::raw_request(env.pair.rust.port, method, path, &[]);
    assert_eq!(r.status, 200, "{label}: status");
    assert_eq!(r.body, body_of(&env.files, expect_rel), "{label}: body");
    // The isolation pair and no-cache are part of the contract on every
    // static 200.
    assert_eq!(
        r.header("cross-origin-opener-policy"),
        Some("same-origin"),
        "{label}"
    );
    assert_eq!(
        r.header("cross-origin-embedder-policy"),
        Some("require-corp"),
        "{label}"
    );
    assert_eq!(r.header("cache-control"), Some("no-cache"), "{label}");
    if let Some(c) = env.pair.legacy.as_ref() {
        let l = util::raw_request(c.port, method, path, &[]);
        assert_eq!(l.status, 200, "{label}: legacy status");
        assert_eq!(l.body, r.body, "{label}: legacy body differs");
        let mut rust_headers = r.contract_headers();
        let mut legacy_headers = l.contract_headers();
        rust_headers.sort();
        legacy_headers.sort();
        assert_eq!(
            rust_headers, legacy_headers,
            "{label}: legacy headers differ"
        );
    }
}

/// Compares a rejection: status, exact 404 envelope bytes, and the bare
/// header set (Content-Type only — no no-store, no isolation headers).
fn compare_rejection(env: &Env, label: &str, method: &str, path: &str, expect_status: u16) {
    let r = util::raw_request(env.pair.rust.port, method, path, &[]);
    assert_eq!(r.status, expect_status, "{label}: status");
    assert!(
        !r.has_header("cross-origin-opener-policy"),
        "{label}: no COOP on static errors"
    );
    if expect_status == 404 && method != "HEAD" {
        // HEAD responses declare Content-Length but never carry a body.
        assert_eq!(
            r.text(),
            r#"{"error":{"code":"not_found","message":"Not found"}}"#,
            "{label}: envelope"
        );
    }
    if expect_status == 404 {
        assert_eq!(
            r.contract_headers(),
            vec![(
                "content-type".into(),
                "application/json; charset=utf-8".into()
            )],
            "{label}: static 404 carries Content-Type only"
        );
    }
    if let Some(c) = env.pair.legacy.as_ref() {
        let l = util::raw_request(c.port, method, path, &[]);
        assert_eq!(l.status, r.status, "{label}: legacy status");
        if method != "HEAD" {
            assert_eq!(l.body, r.body, "{label}: legacy body differs");
        }
        assert_eq!(
            l.contract_headers(),
            r.contract_headers(),
            "{label}: legacy headers"
        );
    }
}

#[test]
fn static_files_serve_with_contract_headers() {
    let env = setup("files");
    compare_file(&env, "root", "GET", "/", "index.html");
    compare_file(&env, "index", "GET", "/index.html", "index.html");
    compare_file(&env, "js", "GET", "/app.js", "app.js");
    compare_file(&env, "css", "GET", "/style.css", "style.css");
    compare_file(&env, "nested", "GET", "/assets/app.css", "assets/app.css");
    compare_file(&env, "json", "GET", "/data.json", "data.json");
    compare_file(&env, "ico", "GET", "/favicon.ico", "favicon.ico");
    compare_file(&env, "ts", "GET", "/worklet.ts", "worklet.ts");
    compare_file(&env, "woff2", "GET", "/font.woff2", "font.woff2");
    compare_file(&env, "svg", "GET", "/logo.svg", "logo.svg");
    compare_file(&env, "noext", "GET", "/README", "README");
    // MIME spot checks (the shared mp_mime_for_path table).
    let r = util::raw_request(env.pair.rust.port, "GET", "/app.js", &[]);
    assert_eq!(r.header("content-type"), Some("text/javascript"));
    let r = util::raw_request(env.pair.rust.port, "GET", "/logo.svg", &[]);
    assert_eq!(r.header("content-type"), Some("image/svg+xml"));
    let r = util::raw_request(env.pair.rust.port, "GET", "/README", &[]);
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
}

#[test]
fn spa_fallback_and_head_semantics() {
    let env = setup("spa");
    compare_file(&env, "deep-route", "GET", "/albums/2", "index.html");
    compare_file(
        &env,
        "deep-nested",
        "GET",
        "/artists/7/tracks",
        "index.html",
    );
    // HEAD of a real file: headers only.
    let r = util::raw_request(env.pair.rust.port, "HEAD", "/app.js", &[]);
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    assert_eq!(r.header("content-type"), Some("text/javascript"));
    // HEAD of an unknown extension-less path: the fallback is GET-only
    // (the C `strcmp(method, "GET")`).
    compare_rejection(&env, "head-deep", "HEAD", "/albums/2", 404);
    // POST of an existing file serves it (the C ignores the method on the
    // file branch).
    compare_file(&env, "post-file", "POST", "/app.js", "app.js");
    // POST of an unknown path: not GET → no fallback.
    compare_rejection(&env, "post-unknown", "POST", "/albums/2", 404);
}

#[test]
fn missing_and_traversal_shapes() {
    let env = setup("missing");
    compare_rejection(&env, "missing-asset", "GET", "/missing.png", 404);
    compare_rejection(&env, "missing-nested", "GET", "/assets/nope.css", 404);
    compare_rejection(&env, "dotdot", "GET", "/../secret.txt", 404);
    compare_rejection(&env, "encoded-dotdot", "GET", "/..%2fsecret.txt", 404);
    compare_rejection(&env, "dot-segment", "GET", "/a/../index.html", 404);
    compare_rejection(&env, "dotdot-bare", "GET", "/..", 404);
    compare_rejection(&env, "dot-file", "GET", "/./index.html", 404);
    // Rust pin: nothing was served for the traversal shapes.
    for path in ["/../secret.txt", "/..%2fsecret.txt"] {
        let r = util::raw_request(env.pair.rust.port, "GET", path, &[]);
        assert_ne!(r.body, b"secret".to_vec(), "{path} must not leak");
    }
}

#[cfg(unix)]
#[test]
fn symlinked_static_files_are_rejected_on_both() {
    let mut env = setup("symlink");
    let outside = test_root("symlink-out");
    std::fs::write(outside.join("secret.js"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.js"), env.static_root.join("leak.js")).unwrap();
    compare_rejection(&env, "symlink", "GET", "/leak.js", 404);
    let _ = &mut env; // servers live until the end of the test
}

#[test]
fn the_api_prefix_is_reserved_and_unauthenticated_static_serves() {
    let env = setup("gate");
    // /api/… never reaches the static handler: health answers as the API
    // (JSON, no isolation headers), unknown API paths use the API's 404
    // envelope.
    let r = util::raw_request(env.pair.rust.port, "GET", "/api/v1/health", &[]);
    assert_eq!(r.status, 200);
    assert!(
        r.header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    assert!(!r.has_header("cross-origin-opener-policy"));
    let r = util::raw_request(env.pair.rust.port, "GET", "/api/v1/nope", &[]);
    // The auth gate precedes routing (C dispatch order): unknown API
    // paths still require credentials first.
    assert_eq!(r.status, 401);
    assert_eq!(
        r.text(),
        r#"{"error":{"code":"unauthorized","message":"missing, invalid, expired or revoked credentials"}}"#
    );
    // Static files need no authentication at all.
    compare_file(&env, "unauth-static", "GET", "/app.js", "app.js");
    if let Some(c) = env.pair.legacy.as_ref() {
        let l = util::raw_request(c.port, "GET", "/api/v1/nope", &[]);
        assert_eq!(l.status, 401, "api auth gate parity");
        assert_eq!(l.text(), r.text(), "api 401 envelope parity");
    }
}
