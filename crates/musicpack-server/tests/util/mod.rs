//! Shared harness for the stage-6 differential oracles
//! (`static_oracle`, `jobs_oracle`): a raw-socket HTTP client, free-port
//! pickup, readiness polling, and a spawn-both-servers helper with
//! kill-on-drop. The older oracle files (`api_oracle`, `media_oracle`)
//! keep their self-contained harnesses by convention; new files share
//! this one. Compiled into each test binary, so helpers used by only one
//! of them are dead code in the others.
#![allow(dead_code)]

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// One HTTP response: status, header list (insertion order), full body.
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    /// Headers minus transport artifacts (`date`, `connection`,
    /// `content-length`), sorted — the comparable contract surface.
    pub fn contract_headers(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .headers
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "date" | "connection" | "content-length"))
            .cloned()
            .collect();
        out.sort();
        out
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends one raw request and reads the full response (`Connection:
/// close` on both servers makes read-to-end safe).
pub fn raw_request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
    use std::io::{Read, Write};
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("Connection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).unwrap();
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
    let headers: Vec<(String, String)> = lines
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap();
            (k.trim().to_ascii_lowercase(), v.trim().to_string())
        })
        .collect();
    Resp {
        status,
        headers,
        body: buf[head_end + 4..].to_vec(),
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Polls until `GET /api/v1/health` answers (health is public).
pub fn wait_ready(port: u16) {
    use std::io::{Read, Write};
    for _ in 0..100 {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(300)));
            if s.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .is_ok()
            {
                let mut buf = Vec::new();
                if s.read_to_end(&mut buf).is_ok() && buf.starts_with(b"HTTP/1.1 200") {
                    return;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("server on port {port} never became ready");
}

pub fn clean_env(cmd: &mut Command) {
    for key in [
        "MUSICPACK_LIBRARY",
        "MUSICPACK_DATABASE",
        "MUSICPACK_LISTEN",
        "MUSICPACK_PORT",
        "MUSICPACK_LOG",
    ] {
        cmd.env_remove(key);
    }
}

pub fn legacy_server_path() -> Option<PathBuf> {
    std::env::var_os("MUSICPACK_LEGACY_SERVER").map(PathBuf::from)
}

pub struct Proc {
    pub port: u16,
    child: Child,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One running server (`serve --no-scan`), logging discarded, stdio
/// nulled so the test harness never blocks on a full pipe.
pub fn spawn(bin: &Path, args: &[&str]) -> Proc {
    let port = free_port();
    let port_string = port.to_string();
    let mut argv: Vec<&str> = args.to_vec();
    argv.push("--port");
    argv.push(&port_string);
    let mut cmd = Command::new(bin);
    cmd.args(&argv).stdout(Stdio::null()).stderr(Stdio::null());
    clean_env(&mut cmd);
    let child = cmd.spawn().expect("spawn server");
    let proc = Proc { port, child };
    wait_ready(proc.port);
    proc
}

/// Both servers over copies of the same database. The legacy side is
/// `None` when `MUSICPACK_LEGACY_SERVER` is unset (tests then print a
/// notice and run their Rust-side assertions only).
pub struct Pair {
    pub rust: Proc,
    pub legacy: Option<Proc>,
}

/// `extra` is inserted before `--no-scan` (used for `--static-dir`).
pub fn spawn_pair(lib: &Path, db_r: &Path, db_c: Option<&Path>, extra: &[&str]) -> Pair {
    let rust_bin = PathBuf::from(env!("CARGO_BIN_EXE_musicpack-server"));
    let lib_string = lib.to_str().unwrap().to_string();
    let db_r_string = db_r.to_str().unwrap().to_string();
    let mut rust_args: Vec<&str> = vec![
        "serve",
        "--library",
        &lib_string,
        "--database",
        &db_r_string,
    ];
    rust_args.extend_from_slice(extra);
    rust_args.push("--no-scan");
    let rust = spawn(&rust_bin, &rust_args);
    let legacy = legacy_server_path().map(|bin| {
        let db_c_string = db_c.expect("c db path").to_str().unwrap().to_string();
        let mut c_args: Vec<&str> = vec![
            "serve",
            "--library",
            &lib_string,
            "--database",
            &db_c_string,
        ];
        c_args.extend_from_slice(extra);
        c_args.push("--no-scan");
        spawn(&bin, &c_args)
    });
    if legacy.is_none() {
        eprintln!("notice: MUSICPACK_LEGACY_SERVER is not set; C comparison skipped");
    }
    Pair { rust, legacy }
}

/// Extracts an integer field from a flat rendered-JSON body
/// (`"key":123`) — the status snapshots are machine-shaped and fixed.
pub fn json_int(body: &str, key: &str) -> i64 {
    let needle = format!("\"{key}\":");
    let start = body
        .find(&needle)
        .unwrap_or_else(|| panic!("key {key} missing in {body}"))
        + needle.len();
    let rest = &body[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap()
}

/// Extracts a string field from a flat rendered-JSON body
/// (`"key":"value"`), without unescaping.
pub fn json_str<'a>(body: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\":");
    let start = body
        .find(&needle)
        .unwrap_or_else(|| panic!("key {key} missing in {body}"))
        + needle.len();
    let rest = &body[start..];
    assert!(rest.starts_with('"'), "string value expected for {key}");
    let end = rest[1..].find('"').unwrap() + 1;
    &rest[1..end]
}
