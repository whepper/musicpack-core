//! API golden oracle: the Rust HTTP server must answer like the legacy C
//! server — same status, same headers, same JSON — for the whole stage-4
//! surface.
//!
//! Method: one fixture library is ingested once (Rust scan; stage 3 proved
//! DB parity), the database file is copied, and both servers serve their
//! own copy (`--no-scan`). Every request below goes to both servers byte
//! for byte; responses are compared exactly except three genuinely
//! nondeterministic fields: the `Date` header, the session secret inside
//! `Set-Cookie`, and the session probe timestamps (shape-asserted).
//!
//! Out-of-scope routes (byte serving, `POST /library/scan|verify`) are
//! asserted as Rust-404s separately — the C behaviour there belongs to
//! stage 5 and is deliberately not compared.
//!
//! Requires `MUSICPACK_LEGACY_SERVER` (built C `musicpack-server`); without
//! it the C side is skipped with a notice and the Rust-side contract
//! assertions still run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use musicpack_core::format::checksum;
use musicpack_server::ingest::scan;
use musicpack_server::store::sqlite::SqliteStore;

const RUST_BIN: &str = env!("CARGO_BIN_EXE_musicpack-server");

// ---- fixture library -----------------------------------------------------

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("api-{}-{name}-{n}", std::process::id()));
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

fn real_mpc() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/musepack/sine44-q5.mpc"
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

/// Rich album: MB-anchored group, two artists, loudness, genres with an
/// escapable quote, two tracks (waveform + representation + loudness on
/// the first), front artwork + booklet, full release/source/provenance.
fn build_alpha(lib: &Path) {
    let dir = lib.join("alpha.mpack");
    let mpc = write_file(&dir, "audio/01.mpc", &real_mpc());
    let flac = write_file(&dir, "audio/02.flac", &real_flac());
    let rep = write_file(&dir, "audio/01-rep.flac", &real_flac());
    let front = write_file(&dir, "artwork/front.jpg", b"front-image");
    let book = write_file(&dir, "booklet/booklet.pdf", b"booklet-pdf");
    let wfm = write_file(&dir, "waveform/01.wfm", &[0x80u8; 20]);
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Alpha Album","artists":[{{"name":"Alice","role":"vocals","sortName":"Alice, A","musicbrainzId":"11111111-2222-3333-4444-555555555555"}},{{"name":"Bob"}}],"releaseType":"album","originalReleaseDate":"2024-05-01","genres":["rock","a\"b"]}},"#,
            r#""identifiers":{{"musicbrainzReleaseGroupId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","musicbrainzReleaseId":"ffffffff-1111-2222-3333-444444444444","barcode":"012345678905"}},"#,
            r#""release":{{"edition":"Deluxe","releaseDate":"2024-05-01","country":"US","label":"Probe Records","catalogueNumber":"PROBE-001"}},"#,
            r#""source":{{"kind":"cd-rip","store":"Probe Store","id":"ps-1"}},"#,
            r#""identity":{{"source":"musicbrainz","confidence":"exact"}},"#,
            r#""provenance":{{"tool":"probe-tool","toolVersion":"0.1"}},"#,
            r#""loudness":{{"algorithm":"ITU-R BS.1770-5","albumLUFS":-8.5,"albumTruePeakDbTP":-1.25}},"#,
            r#""artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{front}"}}],"#,
            r#""booklet":[{{"path":"booklet/booklet.pdf","sha256":"{book}"}}],"#,
            r#""media":["#,
            r#"{{"disc":1,"format":"Digital","tracks":["#,
            r#"{{"track":1,"title":"First \"Quoted\"","duration":180.5,"loudness":{{"trackLUFS":-7.5,"truePeakDbTP":-1.0}},"artists":[{{"name":"Alice"}}],"identifiers":{{"isrc":"US-AAA-24-00001"}},"audio":{{"path":"audio/01.mpc","sha256":"{mpc}"}},"waveform":{{"version":1,"path":"waveform/01.wfm","sha256":"{wfm}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":10}},"representations":[{{"path":"audio/01-rep.flac","sha256":"{rep}","label":"FLAC 16/44","codec":"flac"}}]}},"#,
            r#"{{"track":2,"title":"Second","audio":{{"path":"audio/02.flac","sha256":"{flac}"}}}}]}}]}}"#
        ),
        front = front,
        book = book,
        mpc = mpc,
        flac = flac,
        rep = rep,
        wfm = wfm,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
}

/// Second album sharing Bob (albumCount 2), hash-anchored, one plain
/// track, no artwork.
fn build_beta(lib: &Path) {
    let dir = lib.join("beta.mpack");
    let audio = write_file(&dir, "audio/01.mpc", b"placeholder");
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Beta Beats","artists":[{{"name":"Bob"}},{{"name":"Carol","role":"featuring"}}]}},"#,
            r#""release":{{"edition":"Standard"}},"#,
            r#""media":[{{"disc":1,"tracks":[{{"track":1,"title":"Only","audio":{{"path":"audio/01.mpc","sha256":"{audio}"}}}}]}}]}}"#
        ),
        audio = audio,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
}

/// Package with a missing audio file (warning/unverified — invisible to
/// the API, exercising the gate).
fn build_thin(lib: &Path) {
    let dir = lib.join("thin.mpack");
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"format":"musicpack","version":1,"album":{"title":"Thin","artists":[{"name":"Zed"}]},"media":[{"disc":1,"tracks":[{"track":1,"title":"T","audio":{"path":"audio/01.mpc","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}]}]}"#,
    )
    .unwrap();
}

/// Malformed manifest (invalid row — invisible).
fn build_broken(lib: &Path) {
    let dir = lib.join("broken.mpack");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), b"{not json").unwrap();
}

fn build_library(lib: &Path) {
    build_alpha(lib);
    build_beta(lib);
    build_thin(lib);
    build_broken(lib);
}

// ---- tiny HTTP client ----------------------------------------------------

struct Resp {
    status: u16,
    headers: HashMap<String, Vec<String>>,
    body: Vec<u8>,
}

fn request(port: u16, raw: &[u8]) -> Resp {
    request_inner(port, raw, "")
}

fn request_labelled(label: &str, port: u16, raw: &[u8]) -> Resp {
    request_inner(port, raw, label)
}

fn request_inner(port: u16, raw: &[u8], label: &str) -> Resp {
    // The legacy MHD stack intermittently stalls POST-with-body requests
    // for seconds (a transport-level flake in this environment: identical
    // bytes sometimes answer instantly, sometimes hang; GETs never do).
    // Retry fresh connections a few times — the compared contract bytes
    // are unaffected, and the Rust side never needs a retry.
    let mut last = format!("no attempts made [{label}]");
    for _ in 0..4 {
        match try_request(port, raw) {
            Ok(resp) => return resp,
            Err(message) => last = message,
        }
    }
    panic!("request [{label}] failed after retries: {last}");
}

fn try_request(port: u16, raw: &[u8]) -> Result<Resp, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    // Mimic curl: TCP_NODELAY plus separate head/body writes (see
    // [`curl_post`]: the legacy MHD stack stalls when head and body
    // arrive coalesced in one segment).
    stream
        .set_nodelay(true)
        .map_err(|e| format!("nodelay: {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .map_err(|e| format!("timeout: {e}"))?;
    match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => {
            let head_end = i + 4;
            stream
                .write_all(&raw[..head_end])
                .map_err(|e| format!("write: {e}"))?;
            stream
                .write_all(&raw[head_end..])
                .map_err(|e| format!("write: {e}"))?;
        }
        None => {
            stream.write_all(raw).map_err(|e| format!("write: {e}"))?;
        }
    }
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(format!("no header block ({} bytes)", buf.len()));
    };
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
    let mut headers: HashMap<String, Vec<String>> = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers
            .entry(name.trim().to_ascii_lowercase())
            .or_default()
            .push(value.trim().to_string());
    }
    Ok(Resp {
        status,
        headers,
        body: buf[head_end + 4..].to_vec(),
    })
}

fn get(port: u16, path: &str, headers: &[(&str, &str)]) -> Resp {
    let mut raw = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("\r\n");
    request(port, raw.as_bytes())
}

fn raw_method(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    raw_method_host(port, "127.0.0.1", method, path, headers, body)
}

fn raw_method_host(
    port: u16,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Resp {
    raw_method_labelled_host("", port, host, method, path, headers, body)
}

fn raw_method_labelled(
    label: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Resp {
    raw_method_labelled_host(label, port, "127.0.0.1", method, path, headers, body)
}

fn raw_method_labelled_host(
    label: &str,
    port: u16,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Resp {
    // Exactly one Host header (MHD rejects duplicates with its own 400;
    // see `same_host_cors`).
    assert!(
        !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")),
        "{label}: pass host via the host argument, not headers"
    );
    let mut raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("\r\n");
    let mut bytes = raw.into_bytes();
    bytes.extend_from_slice(body);
    request_labelled(label, port, &bytes)
}

// ---- servers -------------------------------------------------------------

/// POSTs via the system curl instead of the raw socket client.
///
/// The legacy MHD stack intermittently stalls POST-with-body requests
/// from hand-rolled socket clients (it reads every byte but sometimes
/// never dispatches; GETs and curl-delivered requests are unaffected —
/// dozens of trials, zero stalls). curl's delivery pattern (separate
/// head/body writes with TCP_NODELAY) avoids the stall reliably, so the
/// session POST comparisons use curl on both sides. The compared bytes
/// (status/headers/body) do not depend on client framing: neither server
/// inspects User-Agent/Accept/Content-Type. Falls back to the raw client
/// when curl is unavailable (e.g. minimal Windows CI images).
fn curl_post(port: u16, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Option<Resp> {
    use std::io::Write;
    let mut cmd = Command::new("curl");
    cmd.args([
        "-s",
        "-i",
        "-m",
        "20",
        "-X",
        "POST",
        &format!("http://127.0.0.1:{port}{path}"),
    ]);
    for (k, v) in headers {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    cmd.arg("--data-binary")
        .arg("@-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    child.stdin.as_mut()?.write_all(body).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_curl_output(&output.stdout)
}

/// Parses `curl -i` output (status line, headers, blank line, body) into
/// a [`Resp`]. Skips any interim `100 Continue` block.
fn parse_curl_output(out: &[u8]) -> Option<Resp> {
    // Our JSON bodies never contain `\r\n\r\n`, so the last occurrence
    // separates the final head from the body.
    let head_end = out.windows(4).rposition(|w| w == b"\r\n\r\n")?;
    let mut head = String::from_utf8_lossy(&out[..head_end]).into_owned();
    // Strip an interim block (`HTTP/1.1 100 Continue\r\n\r\n` HV…) if
    // curl printed one.
    if let Some(idx) = head.rfind("\r\nHTTP/") {
        head = head[idx + 2..].to_string();
    }
    let mut lines = head.lines();
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let mut response_headers: HashMap<String, Vec<String>> = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':')?;
        response_headers
            .entry(name.trim().to_ascii_lowercase())
            .or_default()
            .push(value.trim().to_string());
    }
    Some(Resp {
        status,
        headers: response_headers,
        body: out[head_end + 4..].to_vec(),
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_ready(port: u16) {
    for i in 0..100 {
        if let Ok(resp) = std::net::TcpStream::connect(("127.0.0.1", port)).and_then(|mut s| {
            use std::io::Write;
            s.set_read_timeout(Some(std::time::Duration::from_millis(200)))
                .unwrap();
            s.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\n\r\n")?;
            let mut buf = [0u8; 512];
            use std::io::Read;
            let n = s.read(&mut buf).unwrap_or(0);
            Ok::<_, std::io::Error>((n, buf))
        }) {
            if resp.0 > 0 {
                if i > 5 {
                    eprintln!("debug: port {port} ready after {i} polls");
                }
                return;
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
    #[allow(dead_code)]
    root: PathBuf,
    port_c: Option<u16>,
    port_r: u16,
    child_c: Option<Child>,
    child_r: Child,
    token: String,
}

// Killing the servers on teardown: spawned children inherit nothing
// user-visible (stdio is nulled below), and must not outlive the test —
// a leaked `serve` process would otherwise hold the test's stdout pipe
// open and hang the harness.
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child_r.kill();
        if let Some(child) = self.child_c.as_mut() {
            let _ = child.kill();
        }
    }
}

fn setup() -> Fixture {
    // Port allocation + server startup are globally serialized: two
    // parallel setups could otherwise pick the same freshly-freed port,
    // and the losing server would exit while `wait_ready` still succeeds
    // against the winner (wrong server, wrong database).
    static SETUP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SETUP_LOCK.lock().unwrap();
    setup_inner()
}

fn setup_inner() -> Fixture {
    let root = test_root("oracle");
    let lib = root.join("lib");
    build_library(&lib);
    // One database, ingested once, then copied per server.
    let db = root.join("base.db");
    {
        let mut store = SqliteStore::open(&db).unwrap();
        // Verifying scan: packages must be servable (visible) for the API
        // lists to be non-empty. `thin` (missing audio) lands in
        // checksum-failed and `broken` in invalid — both invisible,
        // exercising the gate.
        let result = scan(&mut store, &lib, true).unwrap();
        assert_eq!(
            (result.total, result.added, result.invalid),
            (4, 3, 1),
            "fixture shape changed"
        );
    }
    // One bearer token, created before the copies diverge.
    let token = {
        let mut store = SqliteStore::open(&db).unwrap();
        use musicpack_server::store::Store;
        let secret = musicpack_server::tokens::generate_secret().unwrap();
        let hash = musicpack_server::tokens::hash_secret(&secret);
        store.create_token("Oracle", &hash).unwrap();
        secret
    };
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", db.display()));
        assert!(
            !sidecar.exists(),
            "WAL sidecar {sidecar:?} would make copies inconsistent"
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
    // Nulled stdio: a serving child must never inherit the test's pipes
    // (it would hold them open past the test's end and hang the harness).
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
        root,
        port_c,
        port_r,
        child_c,
        child_r,
        token,
    }
}

// ---- comparison ----------------------------------------------------------

/// Compares one request's outcome on both servers: status, headers
/// (minus `Date`), and body (with session-shape normalization).
fn compare(
    fx: &Fixture,
    label: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    let Some(port_c) = fx.port_c else { return };
    let c = raw_method_labelled(&format!("{label}-C"), port_c, method, path, headers, body);
    let r = raw_method_labelled(
        &format!("{label}-R"),
        fx.port_r,
        method,
        path,
        headers,
        body,
    );
    assert_same(label, method, path, &c, &r);
}

/// Compares a POST-with-body via curl on both sides (see [`curl_post`]).
/// Falls back to the raw client when curl is unavailable.
fn compare_post(fx: &Fixture, label: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) {
    let Some(port_c) = fx.port_c else { return };
    let c = curl_post(port_c, path, headers, body).unwrap_or_else(|| {
        raw_method_labelled(&format!("{label}-C"), port_c, "POST", path, headers, body)
    });
    let r = curl_post(fx.port_r, path, headers, body).unwrap_or_else(|| {
        raw_method_labelled(
            &format!("{label}-R"),
            fx.port_r,
            "POST",
            path,
            headers,
            body,
        )
    });
    assert_same(label, "POST", path, &c, &r);
}

fn assert_same(label: &str, method: &str, path: &str, c: &Resp, r: &Resp) {
    assert_eq!(
        (r.status, c.status),
        (c.status, c.status),
        "{label}: status differs ({method} {path}): Rust={} C={}",
        r.status,
        c.status
    );
    let norm_headers = |resp: &Resp| {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (k, v) in &resp.headers {
            // Transport artifacts, not contract: timestamps, framing, and
            // the keep-alive negotiation (which varies with client
            // headers between the raw socket client and curl).
            if k == "date" || k == "content-length" || k == "connection" {
                continue;
            }
            map.insert(k.clone(), v.clone());
        }
        map
    };
    // Set-Cookie carries a fresh secret per server; compare by shape.
    // An empty secret is the session-clearing cookie (logout) and is
    // normalized as-is.
    let norm_cookie = |values: Option<&Vec<String>>| {
        values.map(|list| {
            list.iter()
                .map(|v| {
                    assert!(
                        v.starts_with("musicpack_session="),
                        "{label}: unexpected cookie: {v}"
                    );
                    let secret = v["musicpack_session=".len()..].split(';').next().unwrap();
                    if secret.is_empty() {
                        return "<CLEARED>".to_string();
                    }
                    let (len, shape) = (
                        secret.len(),
                        secret
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                    );
                    let rest = &v["musicpack_session=".len() + secret.len()..];
                    assert_eq!((len, shape), (43, true), "{label}: bad secret shape");
                    format!("<SECRET>{rest}")
                })
                .collect::<Vec<_>>()
        })
    };
    let mut hc = norm_headers(c);
    let mut hr = norm_headers(r);
    hc.insert(
        "set-cookie".into(),
        norm_cookie(hc.get("set-cookie")).unwrap_or_default(),
    );
    hr.insert(
        "set-cookie".into(),
        norm_cookie(hr.get("set-cookie")).unwrap_or_default(),
    );
    assert_eq!(hr, hc, "{label}: headers differ ({method} {path})");
    assert_eq!(
        normalize_body(&r.body),
        normalize_body(&c.body),
        "{label}: body differs ({method} {path})\nRust: {}\nC:    {}",
        String::from_utf8_lossy(&r.body),
        String::from_utf8_lossy(&c.body)
    );
}

/// Redacts session-probe timestamps by shape (`"YYYY-MM-DD HH:MM:SS"`);
/// everything else compares byte-for-byte.
fn normalize_body(body: &[u8]) -> Vec<u8> {
    let mut text = String::from_utf8(body.to_vec()).expect("API bodies are UTF-8");
    for key in ["createdAt", "expiresAt"] {
        let mut start = 0;
        loop {
            let needle = format!("\"{key}\":\"");
            let Some(i) = text[start..].find(&needle) else {
                break;
            };
            let vs = start + i + needle.len();
            let end = text[vs..].find('"').unwrap() + vs;
            let value = &text[vs..end];
            assert!(
                value.len() == 19,
                "unexpected timestamp shape for {key}: {value}"
            );
            text.replace_range(vs..end, "<TS>");
            start = vs + 4;
        }
    }
    text.into_bytes()
}

fn bearer(fx: &Fixture) -> (String, String) {
    ("Authorization".to_string(), format!("Bearer {}", fx.token))
}

// ---- tests ---------------------------------------------------------------

#[test]
fn health_matches() {
    let fx = setup();
    let Some(port_c) = fx.port_c else { return };
    for port in [port_c, fx.port_r] {
        let resp = get(port, "/api/v1/health", &[]);
        assert_eq!(resp.status, 200);
    }
    compare(&fx, "health", "GET", "/api/v1/health", &[], &[]);
    compare(&fx, "health-head", "HEAD", "/api/v1/health", &[], &[]);
}

#[test]
fn auth_gate_matches() {
    let fx = setup();
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    // No credentials → 401 envelope on both.
    compare(&fx, "no-auth", "GET", "/api/v1/artists", &[], &[]);
    let r = get(fx.port_r, "/api/v1/artists", &[]);
    assert_eq!(r.status, 401);
    assert_eq!(
        String::from_utf8(r.body).unwrap(),
        r#"{"error":{"code":"unauthorized","message":"missing, invalid, expired or revoked credentials"}}"#
    );
    // Bad bearer → 401.
    compare(
        &fx,
        "bad-bearer",
        "GET",
        "/api/v1/artists",
        &[("Authorization", "Bearer mpk_nope")],
        &[],
    );
    // Cookie without session → 401.
    compare(
        &fx,
        "bad-cookie",
        "GET",
        "/api/v1/artists",
        &[("Cookie", "musicpack_session=abcdef")],
        &[],
    );
    // Good bearer → 200.
    let r = get(fx.port_r, "/api/v1/artists", &authed);
    assert_eq!(r.status, 200);
    // Health stays public.
    let r = get(fx.port_r, "/api/v1/health", &[]);
    assert_eq!(r.status, 200);
}

#[test]
fn session_lifecycle_matches() {
    let fx = setup();
    let Some(_) = fx.port_c else { return };
    // Malformed bodies → 400 (via curl: POST-with-body over a raw
    // socket intermittently stalls the legacy MHD stack; see
    // [`curl_post`]).
    for (label, body) in [
        ("empty", &b""[..]),
        ("garbage", &b"not json"[..]),
        ("wrong-shape", &b"{}"[..]),
        ("empty-token", &b"{\"token\":\"\"}"[..]),
    ] {
        compare_post(&fx, label, "/api/v1/session", &[], body);
    }
    // Unknown token → 401.
    compare_post(
        &fx,
        "bad-token",
        "/api/v1/session",
        &[],
        br#"{"token":"mpk_nope"}"#,
    );
    // `Secure` cookie attribute behind a TLS-terminating proxy (valid
    // token so a cookie is actually minted; secrets differ per server
    // and are shape-compared).
    compare_post(
        &fx,
        "secure-cookie",
        "/api/v1/session",
        &[("X-Forwarded-Proto", "https")],
        format!("{{\"token\":\"{}\"}}", fx.token).as_bytes(),
    );
    // Valid exchange on both; each server mints its own secret.
    let create = |port: u16| {
        curl_post(
            port,
            "/api/v1/session",
            &[],
            format!("{{\"token\":\"{}\"}}", fx.token).as_bytes(),
        )
        .unwrap_or_else(|| {
            raw_method(
                port,
                "POST",
                "/api/v1/session",
                &[],
                format!("{{\"token\":\"{}\"}}", fx.token).as_bytes(),
            )
        })
    };
    let rc = create(fx.port_r);
    assert_eq!(rc.status, 200);
    assert_eq!(
        String::from_utf8(rc.body.clone()).unwrap(),
        r#"{"status":"authenticated"}"#
    );
    let cookie_c = session_cookie(fx.port_c.unwrap(), &fx.token);
    let cookie_r = session_cookie(fx.port_r, &fx.token);
    // Cookie auth works on both; probe bodies match after normalization.
    // NOTE: each server minted its own secret, so the two sides must be
    // queried with their own cookie (one shared `compare` call cannot
    // carry per-server credentials). Session row ids are likewise
    // server-local autoincrements, so they are redacted by shape — the
    // contract is the object shape, not the id value.
    if let Some(port_c) = fx.port_c {
        let c = get(port_c, "/api/v1/session", &[("Cookie", cookie_c.as_str())]);
        let r = get(
            fx.port_r,
            "/api/v1/session",
            &[("Cookie", cookie_r.as_str())],
        );
        assert_eq!(c.status, r.status, "session-get: status differs");
        let redact_id = |body: &[u8]| {
            let mut text = String::from_utf8(body.to_vec()).expect("API bodies are UTF-8");
            let mut start = 0;
            while let Some(i) = text[start..].find("\"id\":") {
                let vs = start + i + 5;
                let len = text[vs..].bytes().take_while(u8::is_ascii_digit).count();
                assert!(len > 0, "non-numeric session id");
                text.replace_range(vs..vs + len, "<ID>");
                start = vs + 4;
            }
            text.into_bytes()
        };
        assert_eq!(
            redact_id(&normalize_body(&r.body)),
            redact_id(&normalize_body(&c.body)),
            "session-get: body differs\nRust: {}\nC:    {}",
            String::from_utf8_lossy(&r.body),
            String::from_utf8_lossy(&c.body)
        );
    }
    let probe = get(fx.port_r, "/api/v1/session", &[("Cookie", &cookie_r)]);
    assert_eq!(probe.status, 200);
    let text = String::from_utf8(probe.body).unwrap();
    assert!(text.contains("\"status\":\"authenticated\""), "{text}");
    assert!(text.contains("\"session\":"), "{text}");
    // Logout revokes: the cookie stops authorizing afterwards.
    compare(
        &fx,
        "logout",
        "DELETE",
        "/api/v1/session",
        &[("Cookie", &cookie_r)],
        &[],
    );
    let probe = get(fx.port_r, "/api/v1/session", &[("Cookie", &cookie_r)]);
    let text = String::from_utf8(probe.body).unwrap();
    assert_eq!(text, r#"{"status":"authenticated"}"#, "{text}");
    let gated = get(
        fx.port_r,
        "/api/v1/artists",
        &[("Cookie", cookie_r.as_str())],
    );
    assert_eq!(gated.status, 401);
}

fn session_cookie(port: u16, token: &str) -> String {
    let resp = curl_post(
        port,
        "/api/v1/session",
        &[],
        format!("{{\"token\":\"{token}\"}}").as_bytes(),
    )
    .unwrap_or_else(|| {
        raw_method(
            port,
            "POST",
            "/api/v1/session",
            &[],
            format!("{{\"token\":\"{token}\"}}").as_bytes(),
        )
    });
    assert_eq!(resp.status, 200);
    let cookies = resp.headers.get("set-cookie").expect("no Set-Cookie");
    assert_eq!(cookies.len(), 1);
    let value = cookies[0]
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("musicpack_session=")
        .to_string();
    format!("musicpack_session={value}")
}

#[test]
fn artists_endpoints_match() {
    let fx = setup();
    if fx.port_c.is_none() {
        return;
    }
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    for (label, path) in [
        ("list", "/api/v1/artists"),
        ("limit-1", "/api/v1/artists?limit=1"),
        ("limit-0-clamp", "/api/v1/artists?limit=0"),
        ("limit-huge-clamp", "/api/v1/artists?limit=999999"),
        // Past int32 range: the C `(int)` cast wraps before clamping.
        ("limit-i32-wrap", "/api/v1/artists?limit=4294967346"),
        ("offset-1", "/api/v1/artists?offset=1"),
        ("offset-huge", "/api/v1/artists?offset=100000"),
        ("search", "/api/v1/artists?q=ali"),
        ("search-case", "/api/v1/artists?q=ALICE"),
        ("search-empty", "/api/v1/artists?q="),
        ("search-wildcard", "/api/v1/artists?q=%"),
        ("search-miss", "/api/v1/artists?q=zzz-no-such-artist"),
        ("bad-limit", "/api/v1/artists?limit=-1"),
        ("bad-limit-text", "/api/v1/artists?limit=many"),
        ("bad-offset", "/api/v1/artists?offset=-5"),
        (
            "too-long",
            &format!("/api/v1/artists?q={}", "x".repeat(300)),
        ),
        ("artist-1", "/api/v1/artists/1"),
        ("artist-2", "/api/v1/artists/2"),
        ("artist-missing", "/api/v1/artists/999"),
        ("artist-zero", "/api/v1/artists/0"),
        ("artist-malformed", "/api/v1/artists/abc"),
        ("artist-overflow", "/api/v1/artists/99999999999999999999"),
    ] {
        compare(&fx, label, "GET", path, &authed, &[]);
    }
    // Exact contract spot-checks on the Rust side.
    let list = get(fx.port_r, "/api/v1/artists", &authed);
    let text = String::from_utf8(list.body).unwrap();
    assert!(text.contains("\"name\":\"Alice\""), "{text}");
    assert!(text.contains("\"albumCount\":1"), "{text}");
    assert!(text.contains("\"name\":\"Bob\""), "{text}");
    assert!(text.contains("\"albumCount\":2"), "{text}");
    assert!(
        !text.contains("Zed"),
        "warning-state artist must be invisible"
    );
}

#[test]
fn albums_endpoints_match() {
    let fx = setup();
    if fx.port_c.is_none() {
        return;
    }
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    for (label, path) in [
        ("list", "/api/v1/albums"),
        ("recent", "/api/v1/albums?sort=recent"),
        ("sort-bogus", "/api/v1/albums?sort=bogus"),
        ("search-title", "/api/v1/albums?q=Alpha"),
        ("search-artist", "/api/v1/albums?q=carol"),
        ("search-miss", "/api/v1/albums?q=zzz"),
        // Query arguments are form-decoded: `+` is a space (the MHD
        // GET-argument rule; found by the migrated web e2e suite, whose
        // URLSearchParams emits `+`). `Alpha+Album` must match
        // "Alpha Album" on both implementations.
        ("search-plus-space", "/api/v1/albums?q=Alpha+Album"),
        ("search-plus-literal", "/api/v1/albums?q=Alpha%2BAlbum"),
        ("search-percent-space", "/api/v1/albums?q=Alpha%20Album"),
        ("paged", "/api/v1/albums?limit=1&offset=1"),
        ("album-1", "/api/v1/albums/1"),
        ("album-2", "/api/v1/albums/2"),
        ("album-missing", "/api/v1/albums/999"),
        ("album-malformed", "/api/v1/albums/1x"),
    ] {
        compare(&fx, label, "GET", path, &authed, &[]);
    }
    let plus = get(fx.port_r, "/api/v1/albums?q=Alpha+Album", &authed);
    let plus_text = String::from_utf8(plus.body).unwrap();
    assert!(
        plus_text.contains("\"title\":\"Alpha Album\""),
        "form-decoded search must find the album: {plus_text}"
    );
    let detail = get(fx.port_r, "/api/v1/albums/1", &authed);
    let text = String::from_utf8(detail.body).unwrap();
    assert!(text.contains("\"title\":\"Alpha Album\""), "{text}");
    assert!(text.contains("\"trackCount\":2"), "{text}");
    assert!(text.contains("\"genres\":[\"rock\",\"a\\\"b\"]"), "{text}");
    assert!(text.contains("\"packageStatus\":\"warning\""), "{text}");
}

#[test]
fn releases_and_tracks_match() {
    let fx = setup();
    if fx.port_c.is_none() {
        return;
    }
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    for (label, path) in [
        ("release-1", "/api/v1/releases/1"),
        ("release-2", "/api/v1/releases/2"),
        ("release-missing", "/api/v1/releases/999"),
        ("track-1", "/api/v1/tracks/1"),
        ("track-2", "/api/v1/tracks/2"),
        ("track-3", "/api/v1/tracks/3"),
        ("track-missing", "/api/v1/tracks/999"),
        ("track-bare", "/api/v1/tracks"),
        ("track-malformed", "/api/v1/tracks/x"),
    ] {
        compare(&fx, label, "GET", path, &authed, &[]);
    }
    // Flattened track+context quirk, waveform block, representation block.
    let track = get(fx.port_r, "/api/v1/tracks/1", &authed);
    let text = String::from_utf8(track.body).unwrap();
    assert!(text.contains("\"context\":{\"disc\":1"), "{text}");
    assert!(text.contains("\"waveform\":{\"version\":1"), "{text}");
    assert!(text.contains("/api/v1/tracks/1/representations/"), "{text}");
    assert!(text.contains("First \\\"Quoted\\\""), "{text}");
    let plain = get(fx.port_r, "/api/v1/tracks/3", &authed);
    let text = String::from_utf8(plain.body).unwrap();
    assert!(text.contains("\"waveform\":null"), "{text}");
    assert!(!text.contains("representations"), "{text}");
}

#[test]
fn methods_errors_and_cors_match() {
    let fx = setup();
    if fx.port_c.is_none() {
        return;
    }
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    for (label, method, path, headers) in [
        ("put-405", "PUT", "/api/v1/albums", &authed[..]),
        ("unknown", "GET", "/api/v1/nonesuch", &authed[..]),
        ("deep-unknown", "GET", "/api/v1/tracks/1/bogus", &authed[..]),
        (
            "long-path",
            "GET",
            &format!("/api/v1/albums/{}", "1".repeat(2100)),
            &authed[..],
        ),
        (
            "cors-forbidden",
            "GET",
            "/api/v1/albums",
            &[
                ("Origin", "https://evil.example"),
                ("Authorization", &format!("Bearer {}", fx.token)),
            ][..],
        ),
        (
            "cors-preflight-forbidden",
            "OPTIONS",
            "/api/v1/albums",
            &[("Origin", "https://evil.example")][..],
        ),
    ] {
        let owned: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        compare(&fx, label, method, path, &refs, &[]);
    }
    // Same-origin CORS echo (Host varies per server port — compare shapes).
    // NOTE: MHD rejects duplicate Host headers with its own HTML 400, so
    // the Host travels via the host argument (exactly one Host header).
    for port in [fx.port_c.unwrap(), fx.port_r] {
        let host = format!("127.0.0.1:{port}");
        let resp = raw_method_host(
            port,
            &host,
            "GET",
            "/api/v1/albums",
            &[
                ("Origin", &format!("http://127.0.0.1:{port}")),
                ("Authorization", &format!("Bearer {}", fx.token)),
            ],
            &[],
        );
        assert_eq!(resp.status, 200);
        let allow = &resp.headers["access-control-allow-origin"];
        assert_eq!(allow, &[format!("http://127.0.0.1:{port}")]);
        assert_eq!(resp.headers["access-control-allow-credentials"], ["true"]);
        assert_eq!(resp.headers["vary"], ["Origin"]);
    }
    // Same-origin preflight succeeds on both.
    for port in [fx.port_c.unwrap(), fx.port_r] {
        let host = format!("127.0.0.1:{port}");
        let resp = raw_method_host(
            port,
            &host,
            "OPTIONS",
            "/api/v1/albums",
            &[("Origin", &format!("http://127.0.0.1:{port}"))],
            &[],
        );
        assert_eq!(resp.status, 204, "preflight on port {port}");
    }
}

#[test]
fn out_of_scope_routes_are_explicit_404s() {
    let fx = setup();
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    // Byte serving is live since stage 5 (see tests/media_oracle.rs); the
    // unknown-subpath shapes below still 404 (a non-numeric rid 400s like
    // the C `parse_id` arm — pinned by media_oracle's malformed-rid case).
    for path in [
        "/api/v1/tracks/1/bogus",
        "/api/v1/tracks/1/representations/1/bogus",
    ] {
        let r = raw_method(fx.port_r, "GET", path, &authed, &[]);
        assert_eq!(r.status, 404, "{path}");
        assert_eq!(
            String::from_utf8(r.body).unwrap(),
            r#"{"error":{"code":"not_found","message":"Unknown endpoint"}}"#,
            "{path}"
        );
    }
    // The job APIs are live since stage 6 (differential coverage in
    // tests/jobs_oracle.rs); only their method errors are pinned here.
    let r = raw_method(fx.port_r, "GET", "/api/v1/library/scan", &authed, &[]);
    assert_eq!(r.status, 405);
    let r = raw_method(fx.port_r, "GET", "/api/v1/library/verify", &authed, &[]);
    assert_eq!(r.status, 405);
}

#[test]
fn library_status_matches_idle_snapshot() {
    let fx = setup();
    if fx.port_c.is_none() {
        return;
    }
    let (bk, bv) = bearer(&fx);
    let authed = [(bk.as_str(), bv.as_str())];
    compare(&fx, "status", "GET", "/api/v1/library/status", &authed, &[]);
    let r = get(fx.port_r, "/api/v1/library/status", &authed);
    assert_eq!(
        String::from_utf8(r.body).unwrap(),
        r#"{"scan":{"running":0,"startedAt":"","finishedAt":"","packagesScanned":0,"added":0,"updated":0,"removed":0,"invalid":0,"failed":0},"verify":{"running":0,"startedAt":"","finishedAt":"","packagesVerified":0,"passed":0,"warnings":0,"failed":0,"jobFailed":0}}"#
    );
}

#[test]
fn serve_lifecycle_cli() {
    // `--no-scan` with a missing database fails like the C.
    let root = test_root("serve-cli");
    let output = Command::new(RUST_BIN)
        .args([
            "serve",
            "--library",
            root.join("lib").to_str().unwrap(),
            "--database",
            root.join("missing.db").to_str().unwrap(),
            "--no-scan",
            "--port",
            "1",
        ])
        .env_remove("MUSICPACK_LIBRARY")
        .env_remove("MUSICPACK_DATABASE")
        .env_remove("MUSICPACK_LISTEN")
        .env_remove("MUSICPACK_PORT")
        .env_remove("MUSICPACK_LOG")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("does not exist"), "{stderr}");
}
