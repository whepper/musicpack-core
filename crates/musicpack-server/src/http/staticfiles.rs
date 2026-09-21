//! Static hosting + SPA fallback — the port of the C `http.c`
//! `serve_static`.
//!
//! Behaviour (all pinned against the reference by `tests/static_oracle.rs`):
//!
//! - gated by `--static-dir` (empty = disabled) and taken **before** the
//!   API's CORS/method gates, exactly like the C `access_handler` branch —
//!   so any method (GET/HEAD/POST/…) of an existing file serves it, and
//!   static responses carry no CORS headers;
//! - `/api/…` is never routed here (the prefix check is the C
//!   `strncmp(url, "/api/", 5)`);
//! - paths are resolved through the Stage-5 containment discipline
//!   ([`crate::pathsafe`]: canonical path rules, existing-ancestor
//!   containment, final-component NOFOLLOW + regular file) — the server
//!   can never serve outside the configured directory. Unlike media
//!   serving, hard links are allowed (the C static branch does not check
//!   `nlink`);
//! - 200 responses carry `Content-Type` from the shared MIME table
//!   ([`crate::probe::mime_for_path`], the C `mp_mime_for_path`),
//!   `Cache-Control: no-cache`, and the cross-origin isolation pair
//!   (`COOP: same-origin`, `COEP: require-corp`) the SharedArrayBuffer
//!   range reader requires;
//! - SPA fallback: an unknown path **without a dot** under a **GET**
//!   serves `index.html` (deep links like `/albums/2`); asset paths keep
//!   their extension and 404 normally; HEAD of an unknown path 404s (the
//!   C checks `strcmp(method, "GET")`);
//! - the static 404 is the JSON error envelope with **only** a
//!   `Content-Type` header (the C `static_error` adds no `Cache-Control`
//!   and no isolation headers).

use super::json::Json;
use super::response::Response;

/// Serves one static request. `path` is the percent-decoded request path
/// (query already split), `method` the raw method token.
pub fn serve(static_dir: &str, path: &str, method: &str) -> Response {
    let rel = path.strip_prefix('/').unwrap_or(path);
    let rel = if rel.is_empty() { "index.html" } else { rel };

    if let Some(response) = try_file(static_dir, rel) {
        return response;
    }

    // SPA fallback: GET-only, extension-less client routes serve the shell.
    if method == "GET" && !rel.contains('.') {
        if let Some(response) = try_file(static_dir, "index.html") {
            return response;
        }
    }
    not_found()
}

/// Resolves + opens one file under the static root with full containment
/// and builds the 200 response. `None` = "go to fallback".
fn try_file(static_dir: &str, rel: &str) -> Option<Response> {
    let abs = crate::pathsafe::resolve_contained(static_dir, rel)?;
    let (file, size) = crate::pathsafe::open_regular_file(&abs, false).ok()?;
    Some(
        Response::file_body(200, file, 0, size)
            .header("Content-Type", crate::probe::mime_for_path(rel))
            .header("Cache-Control", "no-cache")
            .header("Cross-Origin-Opener-Policy", "same-origin")
            .header("Cross-Origin-Embedder-Policy", "require-corp"),
    )
}

/// The static 404: JSON envelope, `Content-Type` only (the C
/// `static_error` — no `no-store`, no isolation headers).
fn not_found() -> Response {
    Response::new(404, Json::error("not_found", "Not found").into_bytes())
        .header("Content-Type", "application/json; charset=utf-8")
}

/// The transport-level gate (the C `access_handler` condition): static
/// hosting is enabled and the path is not reserved for the API.
pub fn handles(static_dir: &str, path: &str) -> bool {
    !static_dir.is_empty() && !path.starts_with("/api/")
}

#[cfg(test)]
mod tests {
    use super::super::response::Body;
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Unique temp dir per call — the tests run in parallel in one
    /// process, so a `process::id()`-only name would collide.
    fn unique_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mp-{tag}-{}-{}", std::process::id(), n))
    }

    fn fixture() -> String {
        let root = unique_dir("static");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("index.html"), b"<html>shell</html>").unwrap();
        std::fs::write(root.join("app.js"), b"console.log(1);").unwrap();
        std::fs::write(root.join("assets/app.css"), b"body{}").unwrap();
        std::fs::write(root.join("README"), b"plain").unwrap();
        root.to_str().unwrap().to_string()
    }

    #[test]
    fn files_serve_with_the_static_header_set() {
        let dir = fixture();
        let r = serve(&dir, "/", "GET");
        assert_eq!(r.status, 200);
        assert_eq!(r.headers[0].1, "text/html");
        assert_eq!(r.headers[1].1, "no-cache");
        assert_eq!(r.headers[2].1, "same-origin");
        assert_eq!(r.headers[3].1, "require-corp");

        let r = serve(&dir, "/app.js", "GET");
        assert_eq!(r.status, 200);
        assert_eq!(r.headers[0].1, "text/javascript");

        let r = serve(&dir, "/assets/app.css", "GET");
        assert_eq!(r.status, 200);
        assert_eq!(r.headers[0].1, "text/css");

        // Method is irrelevant on the file branch (the C quirk).
        let r = serve(&dir, "/app.js", "POST");
        assert_eq!(r.status, 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spa_fallback_is_get_only_and_extension_less() {
        let dir = fixture();
        let r = serve(&dir, "/albums/2", "GET");
        assert_eq!(r.status, 200, "deep link serves the shell");
        assert_eq!(r.headers[0].1, "text/html");

        let r = serve(&dir, "/albums/2", "HEAD");
        assert_eq!(r.status, 404, "fallback is GET-only");

        let r = serve(&dir, "/missing.png", "GET");
        assert_eq!(r.status, 404, "asset paths 404 normally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_static_404_carries_only_content_type() {
        let dir = fixture();
        let r = serve(&dir, "/missing.png", "GET");
        assert_eq!(r.status, 404);
        assert_eq!(
            r.headers,
            vec![(
                "Content-Type".into(),
                "application/json; charset=utf-8".into()
            )]
        );
        assert_eq!(
            String::from_utf8(match &r.body {
                Body::Bytes(b) => b.clone(),
                _ => unreachable!(),
            })
            .unwrap(),
            r#"{"error":{"code":"not_found","message":"Not found"}}"#
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn traversal_shapes_never_serve() {
        let dir = fixture();
        // The transport hands over percent-decoded paths (like MHD), so
        // every traversal shape decodes to something containing a dot —
        // containment fails first, then the dot check skips the SPA
        // fallback → plain 404 on both.
        for path in [
            "/../secret.txt",
            "/../secret",
            "/a/../index.html",
            "/..",
            "/./index.html",
        ] {
            let r = serve(&dir, path, "GET");
            assert_eq!(r.status, 404, "{path}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_are_rejected() {
        let dir = fixture();
        let outside = unique_dir("static-out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.js"), b"secret").unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.js"),
            std::path::Path::new(&dir).join("leak.js"),
        )
        .unwrap();
        let r = serve(&dir, "/leak.js", "GET");
        assert_eq!(r.status, 404, "final-component symlink must not serve");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn gate_matches_the_c_prefix_check() {
        assert!(handles("/srv/web", "/"));
        assert!(handles("/srv/web", "/albums/2"));
        assert!(!handles("", "/"));
        assert!(!handles("/srv/web", "/api/v1/health"));
        assert!(!handles("/srv/web", "/api/v1/tracks/1/audio"));
        // `/api` exactly is NOT reserved (the C strncmp compares 5 bytes).
        assert!(handles("/srv/web", "/api"));
    }
}
