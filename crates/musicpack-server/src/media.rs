//! Media resolution: from a database [`MediaRef`](crate::store::MediaRef)
//! to an opened, validated file handle.
//!
//! This is the narrow bridge between the collector projection and the HTTP
//! byte layer, mirroring the C `serve_object` filesystem half:
//!
//! ```text
//! MediaRef (store) → serveability gate → resolve + containment → open →
//! fstat checks → magic/inline/filename → MediaResource → HTTP serving
//! ```
//!
//! Security properties (all preserved from the reference):
//!
//! - the relative path is re-validated against the canonical manifest path
//!   rules even though it came from the database;
//! - the package directory is canonicalized and every *existing* ancestor
//!   of the joined path must stay beneath it (the C
//!   `check_existing_ancestors`);
//! - the final component must not be a symlink (the C `O_NOFOLLOW`; via
//!   `symlink_metadata` here, the same mechanism the core's
//!   `DirectoryBackend` and `probe::is_regular_file` use — std-only, no
//!   `libc`, with the same inherent check-then-open TOCTOU the reference
//!   documents);
//! - the opened file must be regular with link count 1 (Unix; regular
//!   only elsewhere, like the C `_WIN32` branch);
//! - the physical path never leaves this module (errors carry only the
//!   C's fixed messages).
//!
//! The store lock must NOT be held across this work: callers resolve the
//! [`MediaRef`](crate::store::MediaRef) under the lock, drop it, then call
//! [`open`] — a slow or stuck filesystem must never serialize the API.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::store::MediaRef;

/// A servable media object: an opened file plus everything the HTTP layer
/// needs to answer 200/206/304/416 without further I/O beyond reads.
#[derive(Debug)]
pub struct MediaResource {
    /// The opened file, positioned at 0.
    pub file: File,
    /// `fstat` size — the only byte count serving trusts.
    pub size: u64,
    /// Stored MIME type (served verbatim as `Content-Type`).
    pub mime: String,
    /// Content hash for ETags (`None` → no ETag, like an empty sha256).
    pub sha256: Option<String>,
    /// Sanitized basename for `Content-Disposition`.
    pub filename: String,
    /// Inline-allowed MIME *and* magic-safe content.
    pub inline: bool,
}

/// Why media could not be opened, with the exact C status/message pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// Unknown/invisible id → 404 + message.
    NotFound(&'static str),
    /// Known but unservable → 503 + message.
    Unavailable(&'static str),
    /// I/O failure after open → 500 + message.
    Internal(&'static str),
}

impl MediaError {
    /// The HTTP status for this failure.
    pub fn status(&self) -> u16 {
        match self {
            MediaError::NotFound(_) => 404,
            MediaError::Unavailable(_) => 503,
            MediaError::Internal(_) => 500,
        }
    }

    /// The C error code for this failure (`not_found` / `unavailable` /
    /// `internal`).
    pub fn code(&self) -> &'static str {
        match self {
            MediaError::NotFound(_) => "not_found",
            MediaError::Unavailable(_) => "unavailable",
            MediaError::Internal(_) => "internal",
        }
    }

    /// The C message for this failure.
    pub fn message(&self) -> &'static str {
        match self {
            MediaError::NotFound(message)
            | MediaError::Unavailable(message)
            | MediaError::Internal(message) => message,
        }
    }
}

/// Opens and validates the file behind a resolved [`MediaRef`].
pub fn open(media: &MediaRef) -> Result<MediaResource, MediaError> {
    // The serveability gate: only `valid`/`warning` packages serve (the C
    // `serveable()`; `verify_status` was already gated by the resolver
    // query).
    if media.status != "valid" && media.status != "warning" {
        return Err(MediaError::Unavailable(
            "package unavailable; rescan the library",
        ));
    }
    // Canonical path rules first (the C `musicpack_path_resolve`), then the
    // final-component discipline with media's extra single-link policy.
    let Some(abs) = crate::pathsafe::resolve_contained(&media.package_path, &media.relative_path)
    else {
        return Err(MediaError::Unavailable("audio object not found"));
    };
    let (mut file, size) = crate::pathsafe::open_regular_file(&abs, true).map_err(|e| {
        use crate::pathsafe::OpenRegularError as E;
        match e {
            E::Missing => MediaError::Unavailable("source file missing"),
            E::NotRegularFile => MediaError::Unavailable("source file is not a regular file"),
            E::Io(_) => MediaError::Internal("cannot stat source file"),
        }
    })?;

    // Magic-byte inline safety (the C `fd_inline_safe`).
    let inline = is_inline_allowed(&media.mime) && magic_safe(&mut file, &media.mime);

    Ok(MediaResource {
        file,
        size,
        mime: c_mime_truncate(&media.mime),
        sha256: media.sha256.clone(),
        filename: disposition_name(&media.relative_path),
        inline,
    })
}

/// Truncates a stored MIME to the 47 bytes the reference serves.
///
/// The C `mp_object_ref.mime[48]` holds 47 chars plus NUL (`library.h:183`,
/// filled by `col_cpy`), so `serve_object` emits `Content-Type` verbatim
/// from the truncated field. The only MIME in the table reaching that
/// limit is the 48-char waveform type, served as
/// `application/vnd.musicpack.waveform-v1+octet-str`. No client reads
/// response MIMEs (the Web client checks status only), but byte-identical
/// compatibility is the point of the oracle, so the truncation is
/// reproduced here — at the HTTP boundary, never in the database (which
/// keeps the full string like the reference schema).
pub fn c_mime_truncate(mime: &str) -> String {
    const LIMIT: usize = 47;
    if mime.len() > LIMIT {
        mime[..LIMIT].to_string()
    } else {
        mime.to_string()
    }
}

/// MIMEs allowed to render inline (`mp_mime_inline_allowed`): byte-safe
/// raster images and audio streams. Everything that can carry active
/// content (SVG, HTML, JS, XML, text, PDF, fonts, WASM) is forced to
/// `attachment`.
pub fn is_inline_allowed(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "audio/musepack"
            | "audio/flac"
            | "audio/wav"
            | "audio/ogg"
    )
}

/// Magic-byte check for inline raster images (`fd_inline_safe`): inline
/// serving is allowed only when the leading bytes match the declared
/// image type. Audio (and non-image MIMEs) are exempt — media cannot
/// execute active content. The file position is restored.
fn magic_safe(file: &mut File, mime: &str) -> bool {
    if !matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/bmp"
    ) {
        return true;
    }
    let mut header = [0u8; 16];
    let bytes = match file.read(&mut header) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let _ = file.seek(SeekFrom::Start(0));
    if bytes == 0 {
        return false;
    }
    match mime {
        "image/jpeg" => bytes >= 3 && header[0] == 0xFF && header[1] == 0xD8 && header[2] == 0xFF,
        "image/png" => bytes >= 8 && header[..8] == *b"\x89PNG\r\n\x1a\n",
        "image/gif" => bytes >= 6 && (header[..6] == *b"GIF87a" || header[..6] == *b"GIF89a"),
        "image/webp" => bytes >= 12 && header[..4] == *b"RIFF" && header[8..12] == *b"WEBP",
        "image/bmp" => bytes >= 2 && header[0] == b'B' && header[1] == b'M',
        _ => false,
    }
}

/// Safe basename for `Content-Disposition` (`content_disposition_name`):
/// strips directories; the first 255 bytes outside printable ASCII (sans
/// `"`, `\`, `;`) become `_`. Byte-operated like the C (multibyte UTF-8
/// passes through); a codepoint split exactly at the 255-byte cap
/// degrades to `U+FFFD` instead of the C's raw split bytes — the only
/// reachable divergence, and only for ~250-byte non-ASCII names.
pub fn disposition_name(relative: &str) -> String {
    let base = relative.rsplit('/').next().unwrap_or(relative);
    let base = if base.is_empty() { "file" } else { base };
    let mut out = Vec::with_capacity(base.len().min(255));
    for &c in base.as_bytes().iter().take(255) {
        if c >= 0x20 && c != 0x7f && c != b'"' && c != b'\\' && c != b';' {
            out.push(c);
        } else {
            out.push(b'_');
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_sanitizes_like_the_reference() {
        assert_eq!(disposition_name("artwork/front.jpg"), "front.jpg");
        assert_eq!(disposition_name("a/b/c.TXT"), "c.TXT");
        // Quote, backslash and semicolon become underscores.
        assert_eq!(disposition_name("x/a\"b\\c;d.jpg"), "a_b_c_d.jpg");
        // Non-ASCII bytes pass through (only the listed ASCII is mapped).
        assert_eq!(disposition_name("música/ñ.jpg"), "ñ.jpg");
    }

    #[test]
    fn inline_allowlist_matches_the_reference() {
        for mime in [
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "image/bmp",
            "audio/musepack",
            "audio/flac",
            "audio/wav",
            "audio/ogg",
        ] {
            assert!(is_inline_allowed(mime), "{mime}");
        }
        for mime in [
            "image/svg+xml",
            "text/html",
            "application/pdf",
            "text/plain",
            "application/octet-stream",
            "application/wasm",
            "",
        ] {
            assert!(!is_inline_allowed(mime), "{mime}");
        }
    }

    #[test]
    fn serveability_gate_matches_the_reference() {
        let mut media = MediaRef {
            id: 1,
            release_id: 1,
            package_path: "/nonexistent".into(),
            relative_path: "audio/01.mpc".into(),
            mime: "audio/musepack".into(),
            codec: "musepack".into(),
            status: "unavailable".into(),
            sha256: None,
        };
        assert!(matches!(
            open(&media),
            Err(MediaError::Unavailable(
                "package unavailable; rescan the library"
            ))
        ));
        media.status = "conflict".into();
        assert!(matches!(open(&media), Err(MediaError::Unavailable(_))));
        media.status = "invalid".into();
        assert!(matches!(open(&media), Err(MediaError::Unavailable(_))));
    }

    #[test]
    fn mime_truncation_reproduces_the_reference_field() {
        // Short MIMEs pass through untouched.
        assert_eq!(c_mime_truncate("audio/musepack"), "audio/musepack");
        assert_eq!(c_mime_truncate("image/svg+xml"), "image/svg+xml");
        // The 48-char waveform MIME loses its final `e` (mime[48]).
        assert_eq!(
            c_mime_truncate("application/vnd.musicpack.waveform-v1+octet-stream"),
            "application/vnd.musicpack.waveform-v1+octet-str"
        );
    }

    #[test]
    fn error_taxonomy_carries_status_and_code() {
        assert_eq!(MediaError::NotFound("Track not found").status(), 404);
        assert_eq!(MediaError::NotFound("Track not found").code(), "not_found");
        assert_eq!(MediaError::Unavailable("source file missing").status(), 503);
        assert_eq!(
            MediaError::Unavailable("source file missing").code(),
            "unavailable"
        );
        assert_eq!(
            MediaError::Internal("cannot stat source file").status(),
            500
        );
    }
}
