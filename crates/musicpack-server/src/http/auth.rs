//! API authentication — the port of the C dispatch auth gate plus the
//! cookie parser.
//!
//! Rules preserved exactly:
//!
//! - `Authorization: Bearer <secret>` (case-insensitive 7-char prefix,
//!   secret verbatim, no trimming) is tried first; any missing/mismatched
//!   header falls back to the `musicpack_session` cookie — never both;
//! - the cookie parser skips leading spaces/semicolons, matches the exact
//!   name (which must be followed by `=`), takes the value to the next
//!   `;`/end, and treats empty values as absent (`musicpack_sessionX=`
//!   aborts the whole parse);
//! - cookie secrets longer than 63 chars are rejected (`len <
//!   MP_SESSION_SECRET_MAX`);
//! - any failure yields the single 401 envelope; there is no 403-for-auth
//!   (403 is CORS-only).

use std::collections::HashMap;

/// The session cookie name (`SESSION_COOKIE`).
pub const SESSION_COOKIE: &str = "musicpack_session";
/// Maximum accepted secret length (`MP_SESSION_SECRET_MAX`).
pub const SECRET_MAX: usize = 64;

/// Extracts our session cookie's value from a `Cookie` header
/// (`cookie_value`). Returns `None` when absent, empty, or malformed.
pub fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    let mut p = cookie;
    loop {
        p = p.trim_start_matches([' ', ';']);
        if p.len() < name.len() || &p[..name.len()] != name {
            let i = p.find(';')?;
            p = &p[i + 1..];
            continue;
        }
        p = &p[name.len()..];
        if !p.starts_with('=') {
            return None;
        }
        p = &p[1..];
        let end = p.find(';').unwrap_or(p.len());
        let value = &p[..end];
        return if value.is_empty() { None } else { Some(value) };
    }
}

/// The presented credential, in C precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// `Authorization: Bearer <secret>` (prefix matched, secret verbatim).
    Bearer(String),
    /// Valid-shaped session cookie.
    Cookie(String),
    /// Nothing usable presented.
    None,
}

/// Selects the credential from the request headers (the dispatch gate).
pub fn select_credential(headers: &HashMap<String, String>) -> Credential {
    if let Some(auth) = headers.get("authorization") {
        if auth.len() >= 7 && auth[..7].eq_ignore_ascii_case("Bearer ") {
            return Credential::Bearer(auth[7..].to_string());
        }
    }
    if let Some(cookie) = headers.get("cookie") {
        if let Some(value) = cookie_value(cookie, SESSION_COOKIE) {
            if value.len() < SECRET_MAX {
                return Credential::Cookie(value.to_string());
            }
        }
    }
    Credential::None
}

/// Parses the `POST /api/v1/session` body (`session_token_from_body`): a
/// bounded scan for `"token"`, optional spaces/tabs/colon, a quoted
/// non-empty value that fits the token cap. Returns `None` on any
/// deviation.
pub fn session_token_from_body(body: &[u8]) -> Option<String> {
    let needle = b"\"token\"";
    let mut i = 0;
    while i + needle.len() <= body.len() {
        if &body[i..i + needle.len()] == needle {
            break;
        }
        i += 1;
    }
    if i + needle.len() > body.len() {
        return None;
    }
    let mut j = i + needle.len();
    while j < body.len() && (body[j] == b' ' || body[j] == b'\t' || body[j] == b':') {
        j += 1;
    }
    if j >= body.len() || body[j] != b'"' {
        return None;
    }
    j += 1;
    let start = j;
    while j < body.len() && body[j] != b'"' {
        j += 1;
    }
    if j >= body.len() {
        return None;
    }
    let value = &body[start..j];
    if value.is_empty() || value.len() >= SECRET_MAX {
        return None;
    }
    std::str::from_utf8(value).ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_takes_precedence_verbatim() {
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Bearer abc".into());
        headers.insert("cookie".into(), format!("{SESSION_COOKIE}=cookie-secret"));
        assert_eq!(
            select_credential(&headers),
            Credential::Bearer("abc".into())
        );
        // Case-insensitive prefix, verbatim secret (no trimming).
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "bearer  spaced  ".into());
        assert_eq!(
            select_credential(&headers),
            Credential::Bearer(" spaced  ".into())
        );
    }

    #[test]
    fn cookie_fallback_rules() {
        // Missing/prefix-mismatched Authorization falls back to cookies.
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Basic abc".into());
        headers.insert(
            "cookie".into(),
            format!("other=1; {SESSION_COOKIE}=s3cr3t; x=2"),
        );
        assert_eq!(
            select_credential(&headers),
            Credential::Cookie("s3cr3t".into())
        );
        // Empty value is absent.
        let mut headers = HashMap::new();
        headers.insert("cookie".into(), format!("{SESSION_COOKIE}=; x=1"));
        assert_eq!(select_credential(&headers), Credential::None);
        // musicpack_sessionX= aborts the parse.
        let mut headers = HashMap::new();
        headers.insert(
            "cookie".into(),
            format!("{SESSION_COOKIE}X=bad; {SESSION_COOKIE}=good"),
        );
        assert_eq!(select_credential(&headers), Credential::None);
        // Overlong secrets are rejected.
        let mut headers = HashMap::new();
        headers.insert(
            "cookie".into(),
            format!("{SESSION_COOKIE}={}", "s".repeat(64)),
        );
        assert_eq!(select_credential(&headers), Credential::None);
        let mut headers = HashMap::new();
        headers.insert(
            "cookie".into(),
            format!("{SESSION_COOKIE}={}", "s".repeat(63)),
        );
        assert_eq!(
            select_credential(&headers),
            Credential::Cookie("s".repeat(63))
        );
    }

    #[test]
    fn session_body_parsing_matches_the_c_scanner() {
        assert_eq!(
            session_token_from_body(br#"{"token":"mpk_abc"}"#),
            Some("mpk_abc".into())
        );
        assert_eq!(
            session_token_from_body(b"{\"token\" : \t: \"mpk_x\"}"),
            Some("mpk_x".into())
        );
        for bad in [
            &b""[..],
            b"{}",
            b"{\"token\":\"\"}",
            b"{\"token\":123}",
            b"{\"token\":\"unterminated}",
            b"{\"tok\":1}",
        ] {
            assert_eq!(session_token_from_body(bad), None, "{bad:?}");
        }
        // Overlong values are rejected (cap is the token-secret max).
        let mut big = b"{\"token\":\"".to_vec();
        big.extend(std::iter::repeat_n(b'a', 64));
        big.extend(b"\"}");
        assert_eq!(session_token_from_body(&big), None);
    }
}
