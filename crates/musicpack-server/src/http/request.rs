//! Minimal blocking HTTP/1.1 request parser for the read-only API.
//!
//! Only what the MusicPack API needs: the request line, headers (stored
//! lowercase, first value wins for duplicates — the subset the router
//! reads), the query string (split on `&`/`=`, form-decoded like MHD's
//! `MHD_GET_ARGUMENT_KIND`: `%XX` lenient plus `+` → space — the path
//! keeps `+` literal), and an optional body
//! capped at [`BODY_MAX`] bytes (the reference's `MP_REQUEST_BODY_MAX`).
//!
//! Rejection policy mirrors the reference stack: MHD rejects overlong or
//! malformed requests at the transport layer (no JSON envelope there), so
//! parse failures surface as [`ParseError`] and the server closes the
//! connection — except an overlong path, which the C dispatch answers
//! with a 400 envelope (handled by the router, not here).
//!
//! Only `HTTP/1.0` and `HTTP/1.1` request versions are accepted; absolute
//! request targets (`http://host/path`) are reduced to their origin form.

use std::collections::HashMap;

/// Maximum request-target + header block size (headers beyond this abort
/// the connection; the reference relies on MHD's own transport limits).
pub const HEAD_MAX: usize = 32 * 1024;
/// Maximum request body (`MP_REQUEST_BODY_MAX`).
pub const BODY_MAX: usize = 4096;
/// Maximum request-target length the router will answer (the C dispatch
/// 400s at `path too long` for `>= 2048`).
pub const PATH_MAX: usize = 2048;

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Why a request could not be parsed (the connection is closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// No complete header block within [`HEAD_MAX`].
    Truncated,
    /// Header block exceeds [`HEAD_MAX`].
    TooLarge,
    /// Malformed request line, version, header, chunking or length.
    Malformed,
    /// Body exceeds [`BODY_MAX`].
    BodyTooLarge,
}

/// Percent-decodes `%XX` sequences with MHD's leniency: a `%` not
/// followed by two hex digits is kept literally (proven against the C
/// server: `?q=%` and `?q=%zz` answer 200 with the `%` escaped by the
/// LIKE layer, never a transport error). `+` stays literal here — the
/// path keeps `+` (proven: the C serves a file named `a+b.js` for
/// `/a+b.js`); only query arguments form-decode `+` (see
/// [`parse_query`]). Returns `None` only when the result is not valid
/// UTF-8.
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Form-decodes one query-argument token: `+` becomes a space **before**
/// the lenient `%XX` decoding (the MHD `MHD_GET_ARGUMENT_KIND` rule —
/// proven against the C server: `?q=Two+Disc` matches the title
/// "Two Disc", while `?q=Two%20Disc` and `?q=Two+Disc` are the same
/// request). `%2B` therefore still yields a literal `+`. The path never
/// uses this (see [`percent_decode`]).
fn form_decode(token: &str) -> Option<String> {
    percent_decode(&token.replace('+', " "))
}

fn parse_query(query: &str) -> Result<HashMap<String, String>, ParseError> {
    let mut map = HashMap::new();
    if query.is_empty() {
        return Ok(map);
    }
    for pair in query.split('&') {
        let (key, value) = match pair.find('=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, ""),
        };
        let key = form_decode(key).ok_or(ParseError::Malformed)?;
        // First value wins (matches MHD's lookup behaviour).
        if let std::collections::hash_map::Entry::Vacant(slot) = map.entry(key) {
            slot.insert(form_decode(value).ok_or(ParseError::Malformed)?);
        }
    }
    Ok(map)
}

/// Parses one request from bytes already read. Returns the request and the
/// number of bytes consumed (headers + declared body, which must be fully
/// present) or a [`ParseError`]. `NeedMore` is reported through
/// `Ok(None)`.
pub fn parse(data: &[u8]) -> Result<Option<(Request, usize)>, ParseError> {
    let head_end = match data.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => i,
        None => {
            return Err(if data.len() > HEAD_MAX {
                ParseError::TooLarge
            } else {
                ParseError::Truncated
            });
        }
    };
    if head_end > HEAD_MAX {
        return Err(ParseError::TooLarge);
    }
    let head = std::str::from_utf8(&data[..head_end]).map_err(|_| ParseError::Malformed)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(ParseError::Malformed)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Malformed);
    };
    if parts.next().is_some() || method.is_empty() || target.is_empty() {
        return Err(ParseError::Malformed);
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ParseError::Malformed);
    }
    // Absolute-form targets reduce to origin-form.
    let target = target
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or(target);
    let (path_raw, query_raw) = match target.find('?') {
        Some(i) => (&target[..i], &target[i + 1..]),
        None => (target, ""),
    };
    if path_raw.is_empty() || !path_raw.starts_with('/') {
        return Err(ParseError::Malformed);
    }
    let path = percent_decode(path_raw).ok_or(ParseError::Malformed)?;
    let query = parse_query(query_raw)?;

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(ParseError::Malformed)?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.contains(' ') || name.contains('\t') {
            return Err(ParseError::Malformed);
        }
        headers
            .entry(name)
            .or_insert_with(|| value.trim().to_string());
    }

    // Chunked bodies are rejected (the C stack never sees them either for
    // this API surface: MHD buffers the 4 KiB body, no chunked contract).
    if headers
        .get("transfer-encoding")
        .is_some_and(|v| !v.eq_ignore_ascii_case("identity"))
    {
        return Err(ParseError::Malformed);
    }
    let content_length: usize = match headers.get("content-length") {
        Some(v) => v.parse().map_err(|_| ParseError::Malformed)?,
        None => 0,
    };
    if content_length > BODY_MAX {
        return Err(ParseError::BodyTooLarge);
    }
    let total = head_end + 4 + content_length;
    if data.len() < total {
        return Err(ParseError::Truncated);
    }
    Ok(Some((
        Request {
            method: method.to_string(),
            path,
            query,
            headers,
            body: data[head_end + 4..total].to_vec(),
        },
        total,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Request {
        let raw = format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n");
        let (req, used) = parse(raw.as_bytes()).unwrap().unwrap();
        assert_eq!(used, raw.len());
        req
    }

    #[test]
    fn request_line_query_and_headers() {
        let req = get("/api/v1/albums?limit=10&offset=5&q=a%20b");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/v1/albums");
        assert_eq!(req.query.get("limit").map(String::as_str), Some("10"));
        assert_eq!(req.query.get("offset").map(String::as_str), Some("5"));
        assert_eq!(req.query.get("q").map(String::as_str), Some("a b"));
        assert_eq!(req.headers.get("host").map(String::as_str), Some("x"));
    }

    #[test]
    fn query_form_decoding_matches_mhd() {
        // %2B keeps a literal plus (escape wins over the form rule)…
        let req = get("/s?a=1%2B2&a=second");
        assert_eq!(req.query.get("a").map(String::as_str), Some("1+2"));
        // …while a bare plus is a space in query arguments (MHD
        // GET-argument form decoding; the differential evidence that
        // caught this: `?q=Two+Disc` matched "Two Disc" on the C server
        // and returned zero results before the fix).
        let req = get("/s?q=Two+Disc&b=x+y");
        assert_eq!(req.query.get("q").map(String::as_str), Some("Two Disc"));
        assert_eq!(req.query.get("b").map(String::as_str), Some("x y"));
    }

    #[test]
    fn path_plus_stays_literal() {
        // The path is NOT form-decoded: `+` is a plain path character
        // (the C serves a file named `a+b.js` for GET /a+b.js).
        let req = get("/a+b.js");
        assert_eq!(req.path, "/a+b.js");
    }

    #[test]
    fn absolute_form_reduces_to_origin_form() {
        let raw = "GET http://127.0.0.1:8080/api/v1/health HTTP/1.1\r\nHost: x\r\n\r\n";
        let (req, _) = parse(raw.as_bytes()).unwrap().unwrap();
        assert_eq!(req.path, "/api/v1/health");
    }

    #[test]
    fn malformed_escapes_are_kept_literally_like_mhd() {
        // Proven against the C server: `?q=%` / `?q=%zz` answer 200 (the
        // LIKE layer escapes the literal `%`), never a transport error.
        assert_eq!(percent_decode("%"), Some("%".into()));
        assert_eq!(percent_decode("%zz"), Some("%zz".into()));
        assert_eq!(percent_decode("a%2Fb"), Some("a/b".into()));
        assert_eq!(percent_decode("%20"), Some(" ".into()));
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        for raw in [
            "GET / HTTP/1.1\r\nNo-Colon\r\n\r\n",
            "GET  HTTP/1.1\r\n\r\n",
            "GET / HTTP/2.0\r\n\r\n",
            "GET not-a-path HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            "GET / HTTP/1.1\r\nContent-Length: abc\r\n\r\n",
        ] {
            assert!(parse(raw.as_bytes()).is_err(), "must reject: {raw:?}");
        }
        // Lowercase methods are accepted by the parser (dispatch 405s them,
        // like the C strcmp chain).
        let raw = "get /api/v1/health HTTP/1.1\r\n\r\n";
        assert_eq!(parse(raw.as_bytes()).unwrap().unwrap().0.method, "get");
    }

    #[test]
    fn body_limits_match_the_reference() {
        let head = "POST /api/v1/session HTTP/1.1\r\nContent-Length: 4097\r\n\r\n";
        assert_eq!(parse(head.as_bytes()), Err(ParseError::BodyTooLarge));
        let head = "POST /api/v1/session HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort";
        assert_eq!(parse(head.as_bytes()), Err(ParseError::Truncated));
        let raw = "POST /api/v1/session HTTP/1.1\r\nContent-Length: 5\r\n\r\n{\"a\"}";
        let (req, used) = parse(raw.as_bytes()).unwrap().unwrap();
        assert_eq!(used, raw.len());
        assert_eq!(req.body, b"{\"a\"}");
    }

    #[test]
    fn oversized_headers_abort() {
        let raw = format!("GET / HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(HEAD_MAX));
        assert_eq!(parse(raw.as_bytes()), Err(ParseError::TooLarge));
    }
}
