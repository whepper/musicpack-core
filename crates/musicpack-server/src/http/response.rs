//! HTTP response assembly for the read-only API.
//!
//! Every response carries an explicit `Content-Length` (MHD does the same)
//! and `Connection: close` (the one deliberate transport simplification:
//! MHD negotiates keep-alive, but correctness never depends on it and the
//! Web client uses `fetch`, which handles close transparently). `HEAD`
//! answers carry the GET headers with an empty body (MHD suppresses the
//! body the same way). An IMF-fixdate `Date` header is emitted like MHD's.
//!
//! CORS behaviour mirrors `mp_api_handle` exactly: no `Origin` header →
//! no CORS headers; allowed origin (whitelist or same-as-`Host`) → echo
//! origin + credentials + `Vary: Origin`; anything else → `403
//! origin_forbidden` before dispatch; `OPTIONS` with an allowed origin →
//! `204` with the exact preflight header set.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;

/// A response body: buffered bytes or a bounded slice of an open file.
///
/// `FileRange` is how media is served without ever loading a whole file:
/// the connection layer seeks once and pumps bounded chunks (`serve_object`
/// via `MHD_create_response_from_fd*`). Memory use is independent of the
/// object size.
#[derive(Debug)]
pub enum Body {
    Bytes(Vec<u8>),
    FileRange { file: File, offset: u64, len: u64 },
}

/// A response: status, headers (insertion order) and body.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

/// Streaming chunk size (64 KiB — the client's block unit; also bounds
/// per-write memory independent of object size).
pub const STREAM_CHUNK: usize = 64 * 1024;

impl Response {
    pub(crate) fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Body::Bytes(body),
        }
    }

    /// A file-slice body for media responses.
    pub fn file_body(status: u16, file: File, offset: u64, len: u64) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Body::FileRange { file, offset, len },
        }
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// A JSON response (`application/json; charset=utf-8`, `no-store`).
    pub fn json(status: u16, body: String) -> Self {
        Self::new(status, body.into_bytes())
            .header("Content-Type", "application/json; charset=utf-8")
            .header("Cache-Control", "no-store")
    }

    /// The JSON error envelope with its status.
    pub fn error(status: u16, code: &str, message: &str) -> Self {
        Self::json(status, super::json::Json::error(code, message))
    }

    /// 204 with the session-clearing cookie (`handle_session_delete`).
    pub fn session_cleared() -> Self {
        Self::new(204, Vec::new())
            .header(
                "Set-Cookie",
                "musicpack_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
            )
            .header("Cache-Control", "no-store")
    }

    /// Attaches `Set-Cookie` for a fresh session secret
    /// (`handle_session_create`'s cookie string, verbatim).
    pub fn with_session_cookie(mut self, secret: &str, secure: bool) -> Self {
        // `Max-Age=2592000` is `MP_SESSION_MAX_AGE_DAYS * 24 * 3600`.
        let mut cookie = format!(
            "musicpack_session={secret}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"
        );
        if secure {
            cookie.push_str("; Secure");
        }
        self.headers.push(("Set-Cookie".to_string(), cookie));
        self
    }

    /// The CORS headers for an allowed origin on a normal response.
    fn cors(mut self, origin: &str) -> Self {
        self.headers.push((
            "Access-Control-Allow-Origin".to_string(),
            origin.to_string(),
        ));
        self.headers.push((
            "Access-Control-Allow-Credentials".to_string(),
            "true".to_string(),
        ));
        self.headers
            .push(("Vary".to_string(), "Origin".to_string()));
        self
    }

    /// The preflight response (204 + the exact C header set).
    pub fn preflight(origin: &str) -> Self {
        Self::new(204, Vec::new())
            .header("Access-Control-Allow-Origin", origin)
            .header(
                "Access-Control-Allow-Methods",
                "GET, HEAD, POST, DELETE, OPTIONS",
            )
            .header(
                "Access-Control-Allow-Headers",
                "Authorization, Content-Type",
            )
            .header("Access-Control-Allow-Credentials", "true")
            .header("Access-Control-Max-Age", "600")
            .header("Vary", "Origin")
    }

    /// The `Content-Length` value: buffered size, or the file-slice
    /// length (MHD reports the fd/object sizes the same way).
    pub fn content_length(&self) -> u64 {
        match &self.body {
            Body::Bytes(bytes) => bytes.len() as u64,
            Body::FileRange { len, .. } => *len,
        }
    }

    /// Serializes status line + headers (+ buffered bodies). File bodies
    /// are never embedded here — the connection layer streams them after
    /// the head (see [`Response::write_to`]).
    pub fn serialize(&self, head_only: bool) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status)).into_bytes();
        for (name, value) in &self.headers {
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("Content-Length: {}\r\n", self.content_length()).as_bytes());
        out.extend_from_slice(format!("Date: {}\r\n", imf_fixdate(SystemTime::now())).as_bytes());
        out.extend_from_slice(b"Connection: close\r\n\r\n");
        if !head_only {
            if let Body::Bytes(bytes) = &self.body {
                out.extend_from_slice(bytes);
            }
        }
        out
    }

    /// Writes the response: head, then the body (streaming file slices in
    /// bounded chunks). `head_only` (HEAD requests) emits headers with the
    /// GET `Content-Length` and no body.
    pub fn write_to(
        &mut self,
        stream: &mut std::net::TcpStream,
        head_only: bool,
    ) -> std::io::Result<()> {
        stream.write_all(&self.serialize(true))?;
        if head_only {
            return Ok(());
        }
        match &mut self.body {
            Body::Bytes(bytes) => {
                stream.write_all(bytes)?;
            }
            Body::FileRange { file, offset, len } => {
                file.seek(SeekFrom::Start(*offset))?;
                let mut remaining = *len;
                let mut chunk = vec![0u8; STREAM_CHUNK];
                while remaining > 0 {
                    let want = (remaining as usize).min(STREAM_CHUNK);
                    file.read_exact(&mut chunk[..want])?;
                    stream.write_all(&chunk[..want])?;
                    remaining -= want as u64;
                }
            }
        }
        Ok(())
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// `true` when `origin` is the same origin as `host` (`origin_matches_host`:
/// strip `scheme://`, compare the authority case-insensitively).
pub fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some(authority) = origin.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = authority.split('/').next().unwrap_or(authority);
    authority.eq_ignore_ascii_case(host)
}

/// Classifies the request's CORS posture, mirroring `mp_api_handle`.
pub enum Cors {
    /// No `Origin` header: plain dispatch, no CORS headers.
    None,
    /// Allowed origin (whitelist or same-as-Host): dispatch, then echo.
    Allowed(String),
    /// Disallowed origin: reject before dispatch.
    Forbidden,
    /// Allowed preflight: answer 204 directly.
    Preflight(String),
}

pub fn classify_cors(
    config: &Config,
    headers: &std::collections::HashMap<String, String>,
    method: &str,
) -> Cors {
    let Some(origin) = headers.get("origin") else {
        return Cors::None;
    };
    let host = headers.get("host").map(String::as_str).unwrap_or("");
    let allowed =
        config.allow_origin.iter().any(|o| o == origin) || origin_matches_host(origin, host);
    if method == "OPTIONS" {
        if allowed {
            return Cors::Preflight(origin.clone());
        }
        return Cors::Forbidden;
    }
    if allowed {
        Cors::Allowed(origin.clone())
    } else {
        Cors::Forbidden
    }
}

/// Applies the CORS decision: forbidden → 403 envelope (with a distinct
/// preflight message, like the C), preflight → 204, otherwise the
/// dispatch response with CORS headers attached.
pub fn apply_cors(response: Response, cors: &Cors) -> Response {
    match cors {
        Cors::None => response,
        Cors::Allowed(origin) => response.cors(origin),
        Cors::Preflight(origin) => Response::preflight(origin),
        Cors::Forbidden => Response::error(403, "origin_forbidden", "origin not allowed"),
    }
}

/// IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) from a `SystemTime`
/// (days-from-civil algorithm; no date dependency).
pub fn imf_fixdate(now: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    // 1970-01-01 was a Thursday (index 0).
    let weekday = DAYS[days.rem_euclid(7) as usize];
    let (year, month, day) = civil_from_days(days + 719_468);
    format!(
        "{weekday}, {day:02} {} {year:04} {:02}:{:02}:{:02} GMT",
        MONTHS[(month - 1) as usize],
        time / 3600,
        (time / 60) % 60,
        time % 60
    )
}

/// Days since 1970-01-01 (civil) → (year, month, day); Howard Hinnant's
/// algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixdate_matches_the_reference_epoch() {
        // 1970-01-01 00:00:00 UTC was a Thursday.
        assert_eq!(imf_fixdate(UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
        // 1789828800 == Sat, 19 Sep 2026 14:40:00 UTC.
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1789828800);
        assert_eq!(imf_fixdate(t), "Sat, 19 Sep 2026 14:40:00 GMT");
    }

    #[test]
    fn origin_host_matching_is_scheme_agnostic() {
        assert!(origin_matches_host(
            "http://a.example:8080",
            "a.example:8080"
        ));
        assert!(origin_matches_host("https://A.EXAMPLE", "a.example"));
        assert!(origin_matches_host("http://h/prefix", "h"));
        assert!(!origin_matches_host("http://a.example", "b.example"));
        assert!(!origin_matches_host("no-scheme", "no-scheme"));
        assert!(!origin_matches_host("http://a.example.evil", "a.example"));
    }

    #[test]
    fn head_serialization_keeps_length_without_body() {
        let r = Response::json(200, r#"{"a":1}"#.to_string());
        let bytes = r.serialize(true);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Content-Length: 7\r\n"));
        assert!(!text.ends_with("{\"a\":1}"));
        let bytes = r.serialize(false);
        assert!(bytes.ends_with(b"{\"a\":1}"));
    }

    #[test]
    fn error_envelope_carries_status() {
        let r = Response::error(404, "not_found", "Album not found");
        assert_eq!(r.status, 404);
        let Body::Bytes(body) = r.body else {
            panic!("error responses are buffered");
        };
        assert_eq!(
            String::from_utf8(body).unwrap(),
            r#"{"error":{"code":"not_found","message":"Album not found"}}"#
        );
    }

    #[test]
    fn file_body_reports_slice_length() {
        let dir = std::env::temp_dir().join(format!("media-body-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("obj.bin");
        std::fs::write(&path, b"0123456789abcdef").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let r = Response::file_body(206, file, 4, 6);
        assert_eq!(r.status, 206);
        assert_eq!(r.content_length(), 6);
        let head = String::from_utf8(r.serialize(true)).unwrap();
        assert!(head.contains("Content-Length: 6\r\n"));
        assert!(!head.ends_with("456789"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
