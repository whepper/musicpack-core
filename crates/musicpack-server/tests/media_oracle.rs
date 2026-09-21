//! Media byte oracle: the Rust byte layer must answer like the legacy C
//! server — same status, same headers, same bytes — for audio,
//! representations, waveforms and assets.
//!
//! Method: one fixture library (real MPC + FLAC bytes, valid JPEG magic,
//! hostile-named assets, an extras row, a missing-audio package) is
//! ingested once with a verifying Rust scan; the database file is copied
//! and both servers serve their own copy (`--no-scan`). IDs are learned
//! from SQLite after the scan, so the matrix addresses real rows.
//!
//! Compared per request: status, the byte-contract headers
//! (`content-type`, `content-range`, `etag`, `accept-ranges`,
//! `content-length`, `content-disposition`, `cache-control`,
//! `x-content-type-options`, `content-security-policy`) and the full
//! body bytes. `date`/`connection` are transport artifacts.
//!
//! Requires `MUSICPACK_LEGACY_SERVER` (built C `musicpack-server`);
//! without it the C side is skipped with a notice and the Rust-side
//! assertions still run. Media is GET/HEAD only (plus one POST probe),
//! so the raw socket client suffices — the MHD POST-body stall from
//! stage 4 does not apply.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use musicpack_core::format::checksum;
use musicpack_server::ingest::scan;
use musicpack_server::store::sqlite::SqliteStore;

// ---- tiny HTTP client ----------------------------------------------------

struct Resp {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn request(port: u16, raw: &[u8]) -> Resp {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_nodelay(true)
        .map_err(|e| format!("nodelay: {e}"))
        .unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    // Head and body as separate writes (curl's delivery pattern).
    match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => {
            let head_end = i + 4;
            stream.write_all(&raw[..head_end]).unwrap();
            stream.write_all(&raw[head_end..]).unwrap();
        }
        None => stream.write_all(raw).unwrap(),
    }
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no header block in response");
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').unwrap();
        // Last value wins here (single-valued byte headers); the oracle
        // compares single values.
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Resp {
        status,
        headers,
        body: buf[head_end + 4..].to_vec(),
    }
}

fn media_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    token: &str,
) -> Resp {
    let mut raw =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("\r\n");
    request(port, raw.as_bytes())
}

// ---- fixture -------------------------------------------------------------

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("media-{0}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(dir: &Path, rel: &str, content: &[u8]) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    checksum::sha256_hex(content)
}

fn real_mpc48() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/musepack/sine44-q5-48s.mpc"
    ))
    .unwrap()
}

fn real_flac() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/reference/audio/flac16-44k.flac"
    ))
    .unwrap()
}

/// A minimal valid JPEG (magic only — serving checks leading bytes, the
/// client never decodes in these tests).
fn jpeg_bytes() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    bytes.extend_from_slice(b"JFIF fake but magic-valid\x00");
    bytes.extend(vec![0u8; 2048]);
    bytes
}

/// Rich package: 48 s Musepack track (multi-block) + FLAC track with a
/// FLAC representation, waveform on track 1, real + fake + SVG artwork,
/// booklet PDF, extras row (unservable), all hashes valid.
fn build_media(lib: &Path) -> PathBuf {
    let dir = lib.join("media.mpack");
    let mpc = write_file(&dir, "audio/01.mpc", &real_mpc48());
    let flac = write_file(&dir, "audio/02.flac", &real_flac());
    let rep = write_file(&dir, "audio/01-rep.flac", &real_flac());
    let front = write_file(&dir, "artwork/front.jpg", &jpeg_bytes());
    let fake = write_file(&dir, "artwork/fake.jpg", b"definitely not a jpeg");
    let svg = write_file(&dir, "artwork/vector.svg", b"<svg></svg>");
    // Space/parenthesis filename with non-magic bytes: mislabeled, so
    // attachment with the name preserved verbatim.
    let spaced = write_file(
        &dir,
        "artwork/my photo (1).jpg",
        b"definitely not a jpeg either",
    );
    let book = write_file(&dir, "booklet/booklet.pdf", b"%PDF-fake");
    let extra = write_file(&dir, "extras/notes.txt", b"extra-notes");
    // 20-byte payload with points=10 (payload rules: len == points * 2).
    let wfm = write_file(&dir, "waveform/01.wfm", &[0x80u8; 20]);
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Media Album","artists":[{{"name":"Media Artist"}}]}},"#,
            r#""artwork":["#,
            r#"{{"role":"front","path":"artwork/front.jpg","sha256":"{front}"}},"#,
            r#"{{"role":"back","path":"artwork/fake.jpg","sha256":"{fake}"}},"#,
            r#"{{"role":"alt","path":"artwork/vector.svg","sha256":"{svg}"}},"#,
            r#"{{"role":"spaced","path":"artwork/my photo (1).jpg","sha256":"{spaced}"}}],"#,
            r#""booklet":[{{"path":"booklet/booklet.pdf","sha256":"{book}"}}],"#,
            r#""extras":[{{"path":"extras/notes.txt","sha256":"{extra}"}}],"#,
            r#""media":[{{"disc":1,"tracks":["#,
            r#"{{"track":1,"title":"Long","audio":{{"path":"audio/01.mpc","sha256":"{mpc}"}},"waveform":{{"version":1,"path":"waveform/01.wfm","sha256":"{wfm}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":10}},"representations":[{{"path":"audio/01-rep.flac","sha256":"{rep}","label":"FLAC 16/44","codec":"flac"}}]}},"#,
            r#"{{"track":2,"title":"Short","audio":{{"path":"audio/02.flac","sha256":"{flac}"}}}}]}}]}}"#,
        ),
        front = front,
        fake = fake,
        svg = svg,
        spaced = spaced,
        book = book,
        extra = extra,
        mpc = mpc,
        flac = flac,
        rep = rep,
        wfm = wfm,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

/// Package with a missing audio file (checksum-failed → invisible).
fn build_thin(lib: &Path) {
    let dir = lib.join("thin.mpack");
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"format":"musicpack","version":1,"album":{"title":"Thin","artists":[{"name":"Zed"}]},"media":[{"disc":1,"tracks":[{"track":1,"title":"T","audio":{"path":"audio/01.mpc","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}]}]}"#,
    )
    .unwrap();
}

fn build_broken(lib: &Path) {
    let dir = lib.join("broken.mpack");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), b"{not json").unwrap();
}

// ---- learned IDs ---------------------------------------------------------

struct Ids {
    track1: i64,
    track2: i64,
    variant1: i64,
    thin_track: i64,
    front_art: i64,
    fake_art: i64,
    svg_art: i64,
    spaced_art: i64,
    booklet: i64,
    extras: i64,
    audio1_sha: String,
    audio1_size: i64,
    wfm_sha: String,
}

fn learn_ids(db: &Path) -> Ids {
    let conn = rusqlite::Connection::open(db).unwrap();
    let track = |disc: i64, n: i64, pkg: &str| {
        conn.query_row(
            "SELECT t.id FROM tracks t
             JOIN media me ON me.id = t.media_id
             JOIN releases r ON r.id = me.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE me.disc_number = ?1 AND t.track_number = ?2 AND p.path LIKE ?3",
            rusqlite::params![disc, n, format!("%{pkg}")],
            |row| row.get(0),
        )
        .unwrap()
    };
    let asset = |kind: &str, role: &str| {
        conn.query_row(
            "SELECT a.id FROM assets a
             JOIN releases r ON r.id = a.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE a.kind = ?1 AND COALESCE(a.role, '') = ?2 AND p.path LIKE '%media.mpack'",
            rusqlite::params![kind, role],
            |row| row.get(0),
        )
        .unwrap()
    };
    let asset_path = |rel: &str| {
        conn.query_row(
            "SELECT a.id FROM assets a
             JOIN releases r ON r.id = a.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE a.relative_path = ?1 AND p.path LIKE '%media.mpack'",
            rusqlite::params![rel],
            |row| row.get(0),
        )
        .unwrap()
    };
    let track1: i64 = track(1, 1, "media.mpack");
    let (variant1, audio1_sha, audio1_size, wfm_sha): (i64, String, i64, String) = (
        conn.query_row(
            "SELECT id FROM audio_variants WHERE track_id = ?1 ORDER BY position, id",
            rusqlite::params![track1],
            |row| row.get(0),
        )
        .unwrap(),
        conn.query_row(
            "SELECT sha256 FROM audio_objects WHERE track_id = ?1",
            rusqlite::params![track1],
            |row| row.get(0),
        )
        .unwrap(),
        real_mpc48().len() as i64,
        conn.query_row(
            "SELECT sha256 FROM track_waveforms WHERE track_id = ?1",
            rusqlite::params![track1],
            |row| row.get(0),
        )
        .unwrap(),
    );
    Ids {
        track1,
        track2: track(1, 2, "media.mpack"),
        variant1,
        thin_track: track(1, 1, "thin.mpack"),
        front_art: asset("artwork", "front"),
        fake_art: asset("artwork", "back"),
        svg_art: asset("artwork", "alt"),
        spaced_art: asset_path("artwork/my photo (1).jpg"),
        booklet: asset_path("booklet/booklet.pdf"),
        extras: asset_path("extras/notes.txt"),
        audio1_sha,
        audio1_size,
        wfm_sha,
    }
}

// ---- servers -------------------------------------------------------------

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_ready(port: u16) {
    for _ in 0..100 {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::Write;
            let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            if s.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\n\r\n")
                .is_ok()
            {
                use std::io::Read;
                let mut buf = [0u8; 512];
                if s.read(&mut buf).unwrap_or(0) > 0 {
                    return;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("server on port {port} never became ready");
}

fn clean_env(cmd: &mut Command) {
    cmd.env_remove("MUSICPACK_LIBRARY");
    cmd.env_remove("MUSICPACK_DATABASE");
    cmd.env_remove("MUSICPACK_LISTEN");
    cmd.env_remove("MUSICPACK_PORT");
    cmd.env_remove("MUSICPACK_LOG");
}

struct Fixture {
    lib: PathBuf,
    port_c: Option<u16>,
    port_r: u16,
    child_c: Option<Child>,
    child_r: Child,
    token: String,
    ids: Ids,
    audio1: Vec<u8>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child_r.kill();
        if let Some(child) = self.child_c.as_mut() {
            let _ = child.kill();
        }
    }
}

fn setup() -> Fixture {
    static SETUP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SETUP_LOCK.lock().unwrap();
    setup_inner()
}

fn setup_inner() -> Fixture {
    const RUST_BIN: &str = env!("CARGO_BIN_EXE_musicpack-server");
    let root = test_root("oracle");
    let lib = root.join("lib");
    build_media(&lib);
    build_thin(&lib);
    build_broken(&lib);
    let db = root.join("base.db");
    {
        let mut store = SqliteStore::open(&db).unwrap();
        let result = scan(&mut store, &lib, true).unwrap();
        assert_eq!(
            (result.total, result.added, result.invalid),
            (3, 2, 1),
            "fixture shape changed"
        );
    }
    let token = {
        let mut store = SqliteStore::open(&db).unwrap();
        use musicpack_server::store::Store;
        let secret = musicpack_server::tokens::generate_secret().unwrap();
        let hash = musicpack_server::tokens::hash_secret(&secret);
        store.create_token("Media", &hash).unwrap();
        secret
    };
    let ids = learn_ids(&db);
    let audio1 = real_mpc48();
    assert_eq!(audio1.len() as i64, ids.audio1_size);
    for suffix in ["-wal", "-shm"] {
        assert!(
            !root.join(format!("base.db{suffix}")).exists(),
            "WAL sidecar would make copies inconsistent"
        );
    }
    let db_c = root.join("c.db");
    let db_r = root.join("r.db");
    std::fs::copy(&db, &db_c).unwrap();
    std::fs::copy(&db, &db_r).unwrap();

    let port_r = free_port();
    let mut cmd_r = Command::new(RUST_BIN);
    cmd_r.args([
        "serve",
        "--library",
        lib.to_str().unwrap(),
        "--database",
        db_r.to_str().unwrap(),
        "--listen",
        "127.0.0.1",
        "--port",
        &port_r.to_string(),
        "--no-scan",
    ]);
    clean_env(&mut cmd_r);
    cmd_r.stdout(Stdio::null()).stderr(Stdio::null());
    let child_r = cmd_r.spawn().expect("failed to start the Rust server");
    wait_ready(port_r);

    let (port_c, child_c) = match std::env::var_os("MUSICPACK_LEGACY_SERVER") {
        Some(bin) => {
            let port_c = free_port();
            let mut cmd_c = Command::new(bin);
            cmd_c.args([
                "serve",
                "--library",
                lib.to_str().unwrap(),
                "--database",
                db_c.to_str().unwrap(),
                "--listen",
                "127.0.0.1",
                "--port",
                &port_c.to_string(),
                "--no-scan",
            ]);
            clean_env(&mut cmd_c);
            cmd_c.stdout(Stdio::null()).stderr(Stdio::null());
            let child_c = cmd_c.spawn().expect("failed to start the C server");
            wait_ready(port_c);
            (Some(port_c), Some(child_c))
        }
        None => {
            eprintln!("notice: MUSICPACK_LEGACY_SERVER is not set; C comparison skipped");
            (None, None)
        }
    };
    Fixture {
        lib,
        port_c,
        port_r,
        child_c,
        child_r,
        token,
        ids,
        audio1,
    }
}

// ---- comparison ----------------------------------------------------------

/// Byte-contract headers: everything the C emits on byte responses that
/// is contract (single values; `set-cookie` never appears here).
fn byte_headers(resp: &Resp) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for key in [
        "content-type",
        "content-range",
        "etag",
        "accept-ranges",
        "content-length",
        "content-disposition",
        "cache-control",
        "x-content-type-options",
        "content-security-policy",
    ] {
        if let Some(value) = resp.headers.get(key) {
            map.insert(key.to_string(), value.clone());
        }
    }
    map
}

fn compare_media(fx: &Fixture, label: &str, method: &str, path: &str, headers: &[(&str, &str)]) {
    let Some(port_c) = fx.port_c else { return };
    let c = media_request(port_c, method, path, headers, &fx.token);
    let r = media_request(fx.port_r, method, path, headers, &fx.token);
    assert_eq!(
        r.status, c.status,
        "{label}: status differs ({method} {path}): Rust={} C={}",
        r.status, c.status
    );
    assert_eq!(
        byte_headers(&r),
        byte_headers(&c),
        "{label}: headers differ ({method} {path})\nRust: {:?}\nC:    {:?}",
        byte_headers(&r),
        byte_headers(&c)
    );
    assert_eq!(
        r.body,
        c.body,
        "{label}: body differs ({method} {path}, Rust {} bytes, C {} bytes)",
        r.body.len(),
        c.body.len()
    );
}

/// Authenticated request against either server (token from the fixture).
fn authed_get(fx: &Fixture, port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
    media_request(port, method, path, headers, &fx.token)
}

// ---- audio matrix ----------------------------------------------------------

#[test]
fn audio_full_get_head_and_post() {
    let fx = setup();
    let t1 = fx.ids.track1;
    let audio = |p: &str| format!("/api/v1/tracks/{t1}/audio{p}");
    for (label, method, path) in [
        ("full", "GET", audio("")),
        ("head", "HEAD", audio("")),
        ("post-bytes", "POST", audio("")),
    ] {
        compare_media(&fx, label, method, path.as_str(), &[]);
    }
    // Rust-side pins (identical on C by the comparisons above).
    let r = authed_get(&fx, fx.port_r, "GET", &audio(""), &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, fx.audio1, "full body must be the file bytes");
    assert_eq!(
        r.headers["etag"],
        format!("\"{}\"", fx.ids.audio1_sha),
        "strong content-sha ETag"
    );
    assert_eq!(r.headers["content-type"], "audio/musepack");
    assert_eq!(r.headers["accept-ranges"], "bytes");
    assert_eq!(r.headers["content-length"], fx.audio1.len().to_string());
    assert!(
        !r.headers.contains_key("content-disposition"),
        "audio serves inline"
    );
    assert_eq!(
        r.headers["cache-control"],
        "private, max-age=0, must-revalidate"
    );
    let h = authed_get(&fx, fx.port_r, "HEAD", &audio(""), &[]);
    assert_eq!(h.status, 200);
    assert!(h.body.is_empty(), "HEAD carries no body");
    assert_eq!(
        h.headers.get("content-length"),
        r.headers.get("content-length"),
        "HEAD mirrors GET length"
    );
}

#[test]
fn audio_ranges() {
    let fx = setup();
    let t1 = fx.ids.track1;
    let s = fx.audio1.len();
    let audio = |p: &str| format!("/api/v1/tracks/{t1}/audio{p}");
    let _ = audio;
    // First block, middle block, EOF-ending, suffix, one-byte.
    for (label, range) in [
        ("first-64k", format!("bytes=0-{}", 64 * 1024 - 1)),
        ("middle", "bytes=65536-131071".to_string()),
        ("eof-ending", format!("bytes={}-", s - 1000)),
        ("suffix", "bytes=-100".to_string()),
        ("one-byte", "bytes=0-0".to_string()),
        ("clamped-end", format!("bytes={}-{}", s - 10, s + 9999)),
        ("head-range", "bytes=0-99".to_string()),
    ] {
        compare_media(
            &fx,
            label,
            "GET",
            &format!("/api/v1/tracks/{t1}/audio"),
            &[("Range", range.as_str())],
        );
    }
    compare_media(
        &fx,
        "head-with-range",
        "HEAD",
        &format!("/api/v1/tracks/{t1}/audio"),
        &[("Range", "bytes=0-99")],
    );
    // Rust-side pins.
    let r = authed_get(
        &fx,
        fx.port_r,
        "GET",
        &format!("/api/v1/tracks/{t1}/audio"),
        &[("Range", "bytes=0-99")],
    );
    assert_eq!(r.status, 206);
    assert_eq!(r.body, fx.audio1[..100]);
    assert_eq!(
        r.headers["content-range"],
        format!("bytes 0-99/{s}"),
        "exact Content-Range shape"
    );
    assert_eq!(r.headers["content-length"], "100");
}

#[test]
fn audio_unsatisfiable_and_malformed_ranges() {
    let fx = setup();
    let t1 = fx.ids.track1;
    let s = fx.audio1.len();
    let path = format!("/api/v1/tracks/{t1}/audio");
    for (label, range) in [
        ("past-eof", format!("bytes={s}-")),
        ("far-past-eof", "bytes=99999999-".to_string()),
        ("garbage", "bytes=xyz".to_string()),
        ("reversed", "bytes=5-2".to_string()),
        ("multi", "bytes=0-1,2-3".to_string()),
        ("unit", "items=0-1".to_string()),
        ("case", "Bytes=0-1".to_string()),
        ("suffix-zero", "bytes=-0".to_string()),
        ("trailing-space", "bytes=0-1 ".to_string()),
        ("overflow", "bytes=0-99999999999999999999999".to_string()),
    ] {
        compare_media(&fx, label, "GET", &path, &[("Range", range.as_str())]);
    }
    // Rust-side pins: shared 416 shape, empty body.
    let r = authed_get(
        &fx,
        fx.port_r,
        "GET",
        &path,
        &[("Range", "bytes=99999999-")],
    );
    assert_eq!(r.status, 416);
    assert!(r.body.is_empty());
    assert_eq!(
        r.headers["content-range"],
        format!("bytes */{s}"),
        "416 carries the total"
    );
    assert_eq!(r.headers["accept-ranges"], "bytes");
    assert!(!r.headers.contains_key("etag"), "416 carries no ETag");
    assert!(
        !r.headers.contains_key("content-type"),
        "416 carries no Content-Type"
    );
}

#[test]
fn audio_conditionals() {
    let fx = setup();
    let t1 = fx.ids.track1;
    let path = format!("/api/v1/tracks/{t1}/audio");
    let etag = format!("\"{}\"", fx.ids.audio1_sha);
    for (label, headers) in [
        ("inm-match", vec![("If-None-Match", etag.as_str())]),
        (
            "inm-match-with-range",
            vec![("If-None-Match", etag.as_str()), ("Range", "bytes=0-99")],
        ),
        ("inm-miss", vec![("If-None-Match", "\"other\"")]),
        ("inm-star", vec![("If-None-Match", "*")]),
        (
            "if-range-match",
            vec![("Range", "bytes=0-99"), ("If-Range", etag.as_str())],
        ),
        (
            "if-range-miss",
            vec![("Range", "bytes=0-99"), ("If-Range", "\"stale\"")],
        ),
        (
            "if-range-malformed",
            vec![("Range", "bytes=0-99"), ("If-Range", "garbage")],
        ),
        (
            "if-range-malformed-unquoted",
            vec![("Range", "bytes=0-99"), ("If-Range", "abc123")],
        ),
    ] {
        compare_media(&fx, label, "GET", &path, &headers);
    }
    // Rust-side pins.
    let get = |headers: &[(&str, &str)]| authed_get(&fx, fx.port_r, "GET", &path, headers);
    let r = get(&[("If-None-Match", &etag)]);
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());
    assert_eq!(r.headers["etag"], etag);
    assert!(
        !r.headers.contains_key("content-type"),
        "304 is header-minimal"
    );
    assert!(
        !r.headers.contains_key("accept-ranges"),
        "304 is header-minimal"
    );
    let r = get(&[("Range", "bytes=0-99"), ("If-Range", "\"stale\"")]);
    assert_eq!(r.status, 200, "stale If-Range serves the full object");
    assert_eq!(r.body.len(), fx.audio1.len());
}

#[test]
fn audio_unknown_and_unavailable() {
    let fx = setup();
    for (label, path, status, message) in [
        (
            "missing-track",
            "/api/v1/tracks/999999/audio",
            404,
            "Track not found",
        ),
        (
            "malformed-id",
            "/api/v1/tracks/abc/audio",
            400,
            "malformed track id",
        ),
        (
            "unavailable-track",
            &format!("/api/v1/tracks/{}/audio", fx.ids.thin_track),
            404,
            "Track not found",
        ),
    ] {
        compare_media(&fx, label, "GET", path, &[]);
        let r = authed_get(&fx, fx.port_r, "GET", path, &[]);
        assert_eq!(r.status, status, "{label}");
        let text = String::from_utf8(r.body).unwrap();
        assert!(text.contains(message), "{label}: {text}");
    }
    // Unauthenticated media is 401 on both.
    if let Some(port_c) = fx.port_c {
        for port in [port_c, fx.port_r] {
            let r = media_request(
                port,
                "GET",
                &format!("/api/v1/tracks/{}/audio", fx.ids.track1),
                &[],
                "",
            );
            assert_eq!(r.status, 401, "port {port}");
        }
    }
}

// ---- representations -------------------------------------------------------

#[test]
fn representation_matrix() {
    let fx = setup();
    let (t1, v1) = (fx.ids.track1, fx.ids.variant1);
    let rep = |t: i64, v: i64, p: &str| format!("/api/v1/tracks/{t}/representations/{v}/audio{p}");
    for (label, path) in [
        ("full", rep(t1, v1, "")),
        ("wrong-track", rep(fx.ids.track2, v1, "")),
        ("missing-variant", rep(t1, 999999, "")),
    ] {
        compare_media(&fx, label, "GET", &path, &[]);
    }
    compare_media(&fx, "head", "HEAD", &rep(t1, v1, ""), &[]);
    compare_media(
        &fx,
        "range",
        "GET",
        &rep(t1, v1, ""),
        &[("Range", "bytes=0-99")],
    );
    compare_media(
        &fx,
        "malformed-rid",
        "GET",
        &format!("/api/v1/tracks/{t1}/representations/xyz/audio"),
        &[],
    );
    // Rust-side pins: ETag + 304 + body identity with the FLAC bytes.
    let r = authed_get(&fx, fx.port_r, "GET", &rep(t1, v1, ""), &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, real_flac());
    assert_eq!(r.headers["content-type"], "audio/flac");
    let etag = r.headers["etag"].clone();
    let r = authed_get(
        &fx,
        fx.port_r,
        "GET",
        &rep(t1, v1, ""),
        &[("If-None-Match", &etag)],
    );
    assert_eq!(r.status, 304);
}

// ---- waveforms -------------------------------------------------------------

#[test]
fn waveform_matrix() {
    let fx = setup();
    let (t1, t2) = (fx.ids.track1, fx.ids.track2);
    for (label, method, path, headers) in [
        (
            "get",
            "GET",
            format!("/api/v1/tracks/{t1}/waveform"),
            vec![],
        ),
        (
            "head",
            "HEAD",
            format!("/api/v1/tracks/{t1}/waveform"),
            vec![],
        ),
        (
            "range",
            "GET",
            format!("/api/v1/tracks/{t1}/waveform"),
            vec![("Range", "bytes=0-9")],
        ),
        (
            "suffix",
            "GET",
            format!("/api/v1/tracks/{t1}/waveform"),
            vec![("Range", "bytes=-5")],
        ),
        (
            "past-eof",
            "GET",
            format!("/api/v1/tracks/{t1}/waveform"),
            vec![("Range", "bytes=99999-")],
        ),
        (
            "missing",
            "GET",
            format!("/api/v1/tracks/{t2}/waveform"),
            vec![],
        ),
        (
            "malformed",
            "GET",
            "/api/v1/tracks/xyz/waveform".to_string(),
            vec![],
        ),
    ] {
        compare_media(&fx, label, method, &path, &headers);
    }
    // Rust-side pins: exact MIME, ETag, 20-byte body, ranged slice.
    let get = |headers: &[(&str, &str)]| {
        authed_get(
            &fx,
            fx.port_r,
            "GET",
            &format!("/api/v1/tracks/{t1}/waveform"),
            headers,
        )
    };
    let r = get(&[]);
    assert_eq!(r.status, 200);
    // The C serves the MIME through its 48-byte `mime[48]` field, so the
    // 48-char waveform type arrives truncated (see `c_mime_truncate`).
    assert_eq!(
        r.headers["content-type"],
        "application/vnd.musicpack.waveform-v1+octet-str"
    );
    assert_eq!(r.body, vec![0x80u8; 20]);
    assert_eq!(r.headers["etag"], format!("\"{}\"", fx.ids.wfm_sha));
    let r = get(&[("Range", "bytes=0-9")]);
    assert_eq!(r.status, 206);
    assert_eq!(r.body, vec![0x80u8; 10]);
    assert_eq!(r.headers["content-range"], "bytes 0-9/20");
}

// ---- assets ------------------------------------------------------------------

#[test]
fn asset_matrix() {
    let fx = setup();
    let ids = &fx.ids;
    for (label, id) in [
        ("front", ids.front_art),
        ("fake", ids.fake_art),
        ("svg", ids.svg_art),
        ("spaced", ids.spaced_art),
        ("booklet", ids.booklet),
    ] {
        let path = format!("/api/v1/assets/{id}");
        compare_media(&fx, label, "GET", &path, &[]);
        compare_media(&fx, &format!("{label}-head"), "HEAD", &path, &[]);
    }
    compare_media(
        &fx,
        "extras-404",
        "GET",
        &format!("/api/v1/assets/{}", ids.extras),
        &[],
    );
    compare_media(&fx, "missing", "GET", "/api/v1/assets/999999", &[]);
    compare_media(&fx, "malformed", "GET", "/api/v1/assets/xyz", &[]);
    compare_media(
        &fx,
        "front-304",
        "GET",
        &format!("/api/v1/assets/{}", ids.front_art),
        &[("If-None-Match", "\"bogus\"")],
    );
    // Rust-side pins: inline vs attachment discipline.
    let get = |id: i64| authed_get(&fx, fx.port_r, "GET", &format!("/api/v1/assets/{id}"), &[]);
    let r = get(ids.front_art);
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "image/jpeg");
    assert!(
        !r.headers.contains_key("content-disposition"),
        "magic-valid JPEG serves inline"
    );
    assert_eq!(r.body, jpeg_bytes());
    let r = get(ids.fake_art);
    assert_eq!(
        r.headers["content-disposition"], "attachment; filename=\"fake.jpg\"",
        "mislabeled JPEG forces attachment"
    );
    let r = get(ids.svg_art);
    assert_eq!(r.headers["content-type"], "image/svg+xml");
    assert!(
        r.headers["content-disposition"].starts_with("attachment;"),
        "SVG forces attachment"
    );
    let r = get(ids.spaced_art);
    assert_eq!(
        r.headers["content-disposition"], "attachment; filename=\"my photo (1).jpg\"",
        "spaces and parens survive sanitization verbatim"
    );
}

// ---- stale records + filesystem edges ----------------------------------------

#[test]
fn stale_record_serves_503_on_both() {
    let fx = setup();
    // Delete the audio file after the scan: the DB row is stale. Both
    // servers must answer 503 without leaking the path. The shared
    // library directory backs both servers' files, so one deletion
    // exercises both (no rescan — the row stays stale, like production).
    std::fs::remove_file(fx.lib.join("media.mpack/audio/01.mpc")).unwrap();
    let path = format!("/api/v1/tracks/{}/audio", fx.ids.track1);
    if let Some(port_c) = fx.port_c {
        for port in [port_c, fx.port_r] {
            let r = media_request(port, "GET", &path, &[], &fx.token);
            assert_eq!(r.status, 503, "port {port}");
            let text = String::from_utf8(r.body).unwrap();
            assert!(text.contains("source file missing"), "port {port}: {text}");
            assert!(
                !text.contains("media.mpack"),
                "port {port}: path leaked: {text}"
            );
        }
    } else {
        let r = authed_get(&fx, fx.port_r, "GET", &path, &[]);
        assert_eq!(r.status, 503);
    }
    // Untouched siblings still serve (the failure is per-object).
    let r = authed_get(
        &fx,
        fx.port_r,
        "GET",
        &format!("/api/v1/tracks/{}/audio", fx.ids.track2),
        &[],
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.body, real_flac());
}

#[test]
fn filesystem_edges_are_503_without_path_leaks() {
    use musicpack_server::media::{MediaError, open};
    use musicpack_server::store::MediaRef;
    let fx = setup();
    let dir = test_root("fsedges");
    std::fs::create_dir_all(dir.join("pkg/audio")).unwrap();
    let target = dir.join("pkg/audio/01.mpc");
    std::fs::write(&target, b"0123456789abcdef").unwrap();
    let base = MediaRef {
        id: 1,
        release_id: 1,
        package_path: dir.join("pkg").to_string_lossy().into_owned(),
        relative_path: "audio/01.mpc".into(),
        mime: "audio/musepack".into(),
        codec: "musepack".into(),
        status: "valid".into(),
        sha256: None,
    };
    // Baseline opens.
    assert!(open(&base).is_ok());
    // Symlinked final component → missing.
    std::os::unix::fs::symlink("01.mpc", dir.join("pkg/audio/link.mpc")).unwrap();
    let mut link = base.clone();
    link.relative_path = "audio/link.mpc".into();
    assert!(matches!(
        open(&link),
        Err(MediaError::Unavailable("source file missing"))
    ));
    // Directory in place of the file → not regular.
    std::fs::create_dir_all(dir.join("pkg/audio/sub")).unwrap();
    let mut sub = base.clone();
    sub.relative_path = "audio/sub".into();
    assert!(matches!(
        open(&sub),
        Err(MediaError::Unavailable("source file is not a regular file"))
    ));
    // Missing file → missing.
    let mut gone = base.clone();
    gone.relative_path = "audio/nope.mpc".into();
    assert!(matches!(
        open(&gone),
        Err(MediaError::Unavailable("source file missing"))
    ));
    // Traversal attempts never resolve (canonical manifest rules).
    for rel in ["../x", "/abs", "a/../b", "", "audio/"] {
        let mut hostile = base.clone();
        hostile.relative_path = rel.into();
        assert!(
            matches!(
                open(&hostile),
                Err(MediaError::Unavailable("audio object not found")),
            ),
            "{rel:?}"
        );
    }
    // Oversized request head closes the connection (bounded parsing).
    let port = fx.port_r;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    use std::io::Write;
    stream
        .write_all(format!("GET /api/v1/tracks/{}//audio HTTP/1.1\r\n", fx.ids.track1).as_bytes())
        .unwrap();
    stream.write_all(b"X-Pad: ").unwrap();
    stream.write_all(&vec![b'a'; 40 * 1024]).unwrap();
    stream.write_all(b"\r\n\r\n").unwrap();
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    assert!(
        buf.is_empty(),
        "oversized headers must close the connection without a response"
    );
}

// Traversal shapes are rejected without serving: shape-first dispatch (like
// the C) 404s unknown shapes even with non-numeric ids, while `%2F`
// decodes to `/` before routing on both (MHD unescapes too), so
// `1%2faudio` serves exactly like `1/audio`.
#[test]
fn dot_segment_traversal_is_rejected() {
    let fx = setup();
    for (label, path) in [
        ("dotdot-shape", "/api/v1/tracks/../1/audio"),
        ("encoded-dotdot", "/api/v1/tracks/..%2f1%2faudio"),
        ("above-root", "/api/v1/../api/v1/health"),
        ("bad-id-bad-shape", "/api/v1/tracks/abc/bogus"),
        ("dot-asset", "/api/v1/assets/%2e%2e/x"),
        ("encoded-slash-audio", "/api/v1/tracks/1%2faudio"),
    ] {
        compare_media(&fx, label, "GET", path, &[]);
    }
    // Rust-side pins: nothing serves bytes here except the encoded-slash
    // alias, which is byte-identical to the plain audio route.
    let r = authed_get(&fx, fx.port_r, "GET", "/api/v1/tracks/1%2faudio", &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, fx.audio1);
    for path in [
        "/api/v1/tracks/../1/audio",
        "/api/v1/tracks/..%2f1%2faudio",
        "/api/v1/tracks/abc/bogus",
    ] {
        let r = authed_get(&fx, fx.port_r, "GET", path, &[]);
        assert_eq!(r.status, 404, "{path}");
    }
}

#[test]
fn oversized_body_closes_the_connection() {
    let fx = setup();
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", fx.port_r)).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    use std::io::{Read, Write};
    stream
        .write_all(b"POST /api/v1/session HTTP/1.1\r\nHost: x\r\nContent-Length: 5000\r\n\r\n")
        .unwrap();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    assert!(
        buf.is_empty(),
        "oversized bodies must close the connection without a response"
    );
}

// ---- web block-reader contract -------------------------------------------------
//
// Replicates `web/app/public/networker.js` validation byte-for-byte: 64 KiB
// block-aligned `Range` fetches, `206` required (a `200` is fatal), the
// `Content-Range` regex, start==base, and declared-length==body-length.
// Reassembled blocks must equal the file.

#[test]
fn web_block_reader_contract() {
    let fx = setup();
    let t1 = fx.ids.track1;
    let total = fx.audio1.len() as u64;
    assert!(total > 3 * 64 * 1024, "fixture must span several blocks");
    let mut assembled = Vec::with_capacity(fx.audio1.len());
    let mut base: u64 = 0;
    while base < total {
        let end = (base + 64 * 1024 - 1).min(total - 1);
        let r = media_request(
            fx.port_r,
            "GET",
            &format!("/api/v1/tracks/{t1}/audio"),
            &[("Range", &format!("bytes={base}-{end}"))],
            &fx.token,
        );
        assert_eq!(r.status, 206, "block at {base}");
        // `if (res.status === 206)` + Content-Range regex + start check.
        let cr = r.headers.get("content-range").expect("Content-Range");
        let body: Vec<&str> = cr
            .strip_prefix("bytes ")
            .expect("bytes unit")
            .split(['-', '/'])
            .collect();
        assert_eq!(body.len(), 3);
        let (start, last, count): (u64, u64, u64) = (
            body[0].parse().unwrap(),
            body[1].parse().unwrap(),
            body[2].parse().unwrap(),
        );
        assert_eq!(start, base, "range start mismatch");
        assert_eq!(count, total, "total mismatch");
        assert_eq!(
            last - start + 1,
            r.body.len() as u64,
            "truncated 206: declared != body"
        );
        assembled.extend_from_slice(&r.body);
        base = last + 1;
    }
    assert_eq!(
        assembled, fx.audio1,
        "reassembled blocks must equal the file"
    );
}

// ---- concurrency ---------------------------------------------------------------
//
// Mixed parallel load: full + ranged media GETs with simultaneous JSON API
// traffic. Every response must be byte-correct (proves no wedging, no
// cross-talk, and — by design — that the store lock is only held for
// metadata resolution, never across file streaming).

#[test]
fn concurrent_media_and_api_requests() {
    let fx = setup();
    // Share via channels (the fixture owns children killed on drop; wait
    // for completion inside the test instead).
    struct Shared {
        port_r: u16,
        token: String,
        track1: i64,
        audio1: Vec<u8>,
    }
    let shared = std::sync::Arc::new(Shared {
        port_r: fx.port_r,
        token: fx.token.clone(),
        track1: fx.ids.track1,
        audio1: fx.audio1.clone(),
    });
    // Hold the fixture (and its child processes) alive for the test.
    let mut threads = Vec::new();
    for worker in 0..8 {
        let shared = std::sync::Arc::clone(&shared);
        threads.push(std::thread::spawn(move || {
            for round in 0..4 {
                let token = shared.token.clone();
                // Full audio GET.
                let r = media_request(
                    shared.port_r,
                    "GET",
                    &format!("/api/v1/tracks/{}/audio", shared.track1),
                    &[],
                    &token,
                );
                assert_eq!(r.status, 200, "worker {worker} round {round}");
                assert_eq!(r.body, shared.audio1);
                // Ranged GET.
                let base = (worker as u64 * 9973 + round as u64 * 65536)
                    % (shared.audio1.len() as u64 - 2000);
                let r = media_request(
                    shared.port_r,
                    "GET",
                    &format!("/api/v1/tracks/{}/audio", shared.track1),
                    &[("Range", &format!("bytes={}-{}", base, base + 999))],
                    &token,
                );
                assert_eq!(r.status, 206, "worker {worker} round {round}");
                assert_eq!(
                    &r.body[..],
                    &shared.audio1[base as usize..base as usize + 1000]
                );
                // JSON API interleaved (must stay responsive mid-stream).
                let r = media_request(shared.port_r, "GET", "/api/v1/albums", &[], &token);
                assert_eq!(r.status, 200, "worker {worker} round {round}");
                assert!(r.body.starts_with(b"{\"albums\":"), "worker {worker}");
            }
        }));
    }
    for thread in threads {
        thread.join().expect("worker panicked");
    }
    // `fx` drops here, terminating both servers.
}
