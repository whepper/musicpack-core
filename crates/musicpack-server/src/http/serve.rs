//! Byte serving: conditional and range evaluation over an opened
//! [`MediaResource`](crate::media::MediaResource) — the port of the C
//! `serve_object` decision tree.
//!
//! The tree, in order:
//!
//! 1. `If-None-Match` first (RFC 9110 §13.1.1): an exact match with the
//!    strong `"sha256"` ETag answers `304` with ETag + `Cache-Control` +
//!    `nosniff` + sandbox CSP — and none of `Content-Type`,
//!    `Accept-Ranges` or `Content-Disposition`.
//! 2. `Range` (+ `If-Range`): a present `If-Range` that mismatches the
//!    current ETag discards the Range (full `200`, so a stale partial
//!    resume cannot corrupt the cached object). Absent `If-Range`, or an
//!    absent content hash, honors the Range.
//! 3. No (or discarded) Range → `200` with `Content-Type`,
//!    `Accept-Ranges: bytes`, validators (`ETag` when hashed +
//!    `Cache-Control: private, max-age=0, must-revalidate`) and the
//!    content headers (`nosniff` always; `attachment` disposition unless
//!    inline-allowed *and* magic-safe; sandbox CSP always).
//! 4. Satisfiable Range → `206` with the above plus `Content-Range`.
//! 5. Malformed *or* unsatisfiable Range → `416` with `Content-Range:
//!    bytes */N`, `Accept-Ranges`, `nosniff` and sandbox CSP — and none of
//!    `ETag`, `Cache-Control`, `Content-Type` or `Content-Disposition`.
//!
//! Sizes always come from the opened file (`fstat`), never the database.

use std::collections::HashMap;

use super::range::{RangeOutcome, parse_range};
use super::response::Response;
use crate::media::MediaResource;

/// `Content-Security-Policy` for package-controlled bytes (sandboxed, no
/// origin access for embedded HTML/SVG).
const SANDBOX_CSP: &str = "sandbox; default-src 'none'; img-src 'self'";
/// Revalidation policy: URLs are stable but bytes can change on rescan.
const CACHE_CONTROL: &str = "private, max-age=0, must-revalidate";

/// The strong ETag for a content hash (`"hex"`, like the C `add_validators`).
/// `None` when the object carries no hash (no ETag anywhere then).
fn etag(sha256: &Option<String>) -> Option<String> {
    sha256
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{s}\""))
}

/// Builds the byte response for an opened [`MediaResource`] and the
/// request headers. The file slice is attached, never read here — the
/// connection layer streams it (or suppresses it for HEAD).
pub fn serve_media(headers: &HashMap<String, String>, media: MediaResource) -> Response {
    let MediaResource {
        file,
        size,
        mime,
        sha256,
        filename,
        inline,
    } = media;
    let size_i64 = size.min(i64::MAX as u64) as i64;
    let tag = etag(&sha256);

    // If-None-Match takes precedence over Range (exact string equality;
    // `*` and weak tags never match — the C `strcmp`).
    if let (Some(tag), Some(inm)) = (tag.as_deref(), headers.get("if-none-match")) {
        if inm == tag {
            let mut response = Response::new(304, Vec::new());
            response.headers.push(("ETag".into(), tag.to_string()));
            response
                .headers
                .push(("Cache-Control".into(), CACHE_CONTROL.into()));
            response
                .headers
                .push(("X-Content-Type-Options".into(), "nosniff".into()));
            response
                .headers
                .push(("Content-Security-Policy".into(), SANDBOX_CSP.into()));
            return response;
        }
    }

    // Range (+ If-Range): a mismatched validator discards the Range.
    let mut range = headers.get("range").map(String::as_str);
    if let (Some(range_value), Some(ir), Some(tag)) =
        (range, headers.get("if-range"), tag.as_deref())
    {
        if ir != tag {
            let _ = range_value;
            range = None;
        }
    }

    // The evaluated slice: full object, sub-range, or a 416 with no body.
    enum Slice {
        Body(u64, u64),
        Empty416,
    }
    let (status, slice, content_range) = match range {
        None => (200, Slice::Body(0, size), None),
        Some(header) => match parse_range(header, size_i64) {
            RangeOutcome::Ok(range) => (
                206,
                Slice::Body(range.start, range.length),
                Some(format!(
                    "bytes {}-{}/{}",
                    range.start,
                    range.start + range.length - 1,
                    size
                )),
            ),
            RangeOutcome::Invalid | RangeOutcome::Unsatisfiable => {
                (416, Slice::Empty416, Some(format!("bytes */{size}")))
            }
        },
    };

    let mut response = match slice {
        Slice::Body(offset, len) => Response::file_body(status, file, offset, len),
        Slice::Empty416 => Response::new(416, Vec::new()),
    };
    if status == 200 || status == 206 {
        response.headers.push(("Content-Type".into(), mime));
        response
            .headers
            .push(("Accept-Ranges".into(), "bytes".into()));
        if let Some(tag) = tag.as_deref() {
            response.headers.push(("ETag".into(), tag.to_string()));
        }
        response
            .headers
            .push(("Cache-Control".into(), CACHE_CONTROL.into()));
        if status == 206 {
            response.headers.push((
                "Content-Range".into(),
                content_range.expect("206 always carries Content-Range"),
            ));
        }
        add_content_headers(&mut response, inline, &filename);
    } else {
        response.headers.push((
            "Content-Range".into(),
            content_range.expect("416 always carries Content-Range"),
        ));
        response
            .headers
            .push(("Accept-Ranges".into(), "bytes".into()));
        response
            .headers
            .push(("X-Content-Type-Options".into(), "nosniff".into()));
        response
            .headers
            .push(("Content-Security-Policy".into(), SANDBOX_CSP.into()));
    }
    response
}

/// Security headers for package-controlled bytes (`add_content_headers`):
/// always `nosniff` + sandbox CSP; `attachment` disposition unless the
/// MIME is inline-allowed *and* the content proved magic-safe.
fn add_content_headers(response: &mut Response, inline: bool, filename: &str) {
    response
        .headers
        .push(("X-Content-Type-Options".into(), "nosniff".into()));
    if !inline {
        response.headers.push((
            "Content-Disposition".into(),
            format!("attachment; filename=\"{filename}\""),
        ));
    }
    response
        .headers
        .push(("Content-Security-Policy".into(), SANDBOX_CSP.into()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaResource;

    fn resource(size: u64, mime: &str, sha: Option<&str>, inline: bool) -> MediaResource {
        // `File` is only carried, never read here (unit scope asserts
        // headers/status); a small temp file stands in on every platform.
        let dir = std::env::temp_dir().join(format!("serve-null-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("null.bin");
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        MediaResource {
            file,
            size,
            mime: mime.into(),
            sha256: sha.map(str::to_string),
            filename: "obj.bin".into(),
            inline,
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn header(response: &Response, name: &str) -> Option<String> {
        response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn full_response_carries_validators_and_security_headers() {
        let response = serve_media(
            &headers(&[]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.content_length(), 1000);
        assert_eq!(
            header(&response, "Content-Type").as_deref(),
            Some("audio/musepack")
        );
        assert_eq!(header(&response, "Accept-Ranges").as_deref(), Some("bytes"));
        assert_eq!(header(&response, "ETag").as_deref(), Some("\"abc123\""));
        assert_eq!(
            header(&response, "Cache-Control").as_deref(),
            Some("private, max-age=0, must-revalidate")
        );
        assert_eq!(
            header(&response, "X-Content-Type-Options").as_deref(),
            Some("nosniff")
        );
        assert!(header(&response, "Content-Disposition").is_none());
        assert!(header(&response, "Content-Security-Policy").is_some());
    }

    #[test]
    fn ranges_slice_and_report() {
        let response = serve_media(
            &headers(&[("range", "bytes=100-199")]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 206);
        assert_eq!(response.content_length(), 100);
        assert_eq!(
            header(&response, "Content-Range").as_deref(),
            Some("bytes 100-199/1000")
        );
        assert_eq!(header(&response, "ETag").as_deref(), Some("\"abc123\""));
        // Suffix range reaches EOF.
        let response = serve_media(
            &headers(&[("range", "bytes=-1")]),
            resource(1000, "audio/flac", None, true),
        );
        assert_eq!(response.status, 206);
        assert_eq!(
            header(&response, "Content-Range").as_deref(),
            Some("bytes 999-999/1000")
        );
        // No ETag without a content hash.
        assert!(header(&response, "ETag").is_none());
    }

    #[test]
    fn bad_and_unsatisfiable_ranges_share_the_416_shape() {
        for range in ["bytes=9999-", "bytes=xyz", "bytes=5-2"] {
            let response = serve_media(
                &headers(&[("range", range)]),
                resource(1000, "audio/musepack", Some("abc123"), true),
            );
            assert_eq!(response.status, 416, "{range}");
            assert_eq!(
                header(&response, "Content-Range").as_deref(),
                Some("bytes */1000"),
                "{range}"
            );
            assert_eq!(header(&response, "Accept-Ranges").as_deref(), Some("bytes"));
            assert!(header(&response, "ETag").is_none(), "{range}");
            assert!(header(&response, "Cache-Control").is_none(), "{range}");
            assert!(header(&response, "Content-Type").is_none(), "{range}");
            assert!(
                header(&response, "Content-Disposition").is_none(),
                "{range}"
            );
            assert_eq!(response.content_length(), 0);
        }
    }

    #[test]
    fn if_none_match_wins_over_range_with_minimal_headers() {
        let response = serve_media(
            &headers(&[("if-none-match", "\"abc123\""), ("range", "bytes=0-99")]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 304);
        assert_eq!(header(&response, "ETag").as_deref(), Some("\"abc123\""));
        assert!(header(&response, "Content-Type").is_none());
        assert!(header(&response, "Accept-Ranges").is_none());
        assert!(header(&response, "Content-Disposition").is_none());
        // A miss falls through to the range.
        let response = serve_media(
            &headers(&[("if-none-match", "\"other\""), ("range", "bytes=0-99")]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 206);
    }

    #[test]
    fn if_range_mismatch_serves_full_200() {
        let response = serve_media(
            &headers(&[("range", "bytes=0-99"), ("if-range", "\"stale\"")]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.content_length(), 1000);
        // A match honors the range; absent hash ignores If-Range.
        let response = serve_media(
            &headers(&[("range", "bytes=0-99"), ("if-range", "\"abc123\"")]),
            resource(1000, "audio/musepack", Some("abc123"), true),
        );
        assert_eq!(response.status, 206);
        let response = serve_media(
            &headers(&[("range", "bytes=0-99"), ("if-range", "anything")]),
            resource(1000, "audio/musepack", None, true),
        );
        assert_eq!(response.status, 206);
    }

    #[test]
    fn unsafe_content_forces_attachment() {
        let response = serve_media(
            &headers(&[]),
            resource(100, "image/svg+xml", Some("abc123"), false),
        );
        assert_eq!(
            header(&response, "Content-Disposition").as_deref(),
            Some("attachment; filename=\"obj.bin\"")
        );
    }
}
