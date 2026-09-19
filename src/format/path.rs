//! Canonical package-relative path rules.
//!
//! Port of `musicpack_path_validate` (`core/libmusicpack/src/path.c`,
//! normative rules in `specs/musicpack-v1.md` §2). This is the pure
//! syntactic validation applied to every manifest-referenced path and every
//! MPAK `DATA` preamble path:
//!
//! Rejected:
//!
//! - empty paths and paths longer than [`PATH_MAX_BYTES`] bytes;
//! - absolute paths (leading `/`), drive letters, UNC and URL schemes —
//!   covered structurally by rejecting `\` and `:` outright;
//! - control characters (bytes `< 0x20` and `0x7f`);
//! - empty segments (`a//b`, trailing `/`), `.` and `..` segments.
//!
//! Everything else — including spaces, unicode, and dot-prefixed *names*
//! like `.hidden` — is accepted.
//!
//! # Compatibility notes
//!
//! - The C function returns a single `MUSICPACK_ERR_PATH`; the
//!   [`PathReason`] granularity here is Rust-side diagnostics only. The
//!   accept/reject *decision* is what must stay identical, and this port
//!   preserves the C's scanning order so the first offending byte class
//!   matches too.
//! - Filesystem containment (`realpath`/`GetFullPathName` resolution
//!   against the package root, symlink-escape rejection) is **not** part of
//!   this module: it belongs to the directory-bundle storage adapter, which
//!   performs I/O. The core parser only ever sees syntactic paths.

use std::fmt;

use crate::limits::PATH_MAX_BYTES;

/// Why a package-relative path was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathReason {
    /// The path is empty.
    Empty,
    /// The path exceeds [`PATH_MAX_BYTES`] bytes.
    TooLong,
    /// The path is absolute (leading `/`).
    Absolute,
    /// The path contains a backslash.
    Backslash,
    /// The path contains a colon (drive letters, URL schemes).
    Colon,
    /// The path contains a control character (`< 0x20` or `0x7f`).
    Control,
    /// The path contains an empty segment (`a//b`).
    EmptySegment,
    /// The path contains a `.` segment.
    DotSegment,
    /// The path contains a `..` segment.
    ParentSegment,
    /// The path ends with `/`.
    TrailingSeparator,
}

impl fmt::Display for PathReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PathReason::Empty => "empty path",
            PathReason::TooLong => "path too long",
            PathReason::Absolute => "absolute path",
            PathReason::Backslash => "backslash separator",
            PathReason::Colon => "colon (drive letter or URL scheme)",
            PathReason::Control => "control character",
            PathReason::EmptySegment => "empty segment",
            PathReason::DotSegment => "\".\" segment",
            PathReason::ParentSegment => "\"..\" segment",
            PathReason::TrailingSeparator => "trailing separator",
        };
        f.write_str(s)
    }
}

/// A package-relative path that violates the canonical rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    /// The offending path (truncated for display if enormous; the full
    /// value is preserved up to a sane bound).
    pub path: String,
    /// Why it was rejected.
    pub reason: PathReason,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsafe path {:?}: {}", self.path, self.reason)
    }
}

impl std::error::Error for PathError {}

/// Validates a package-relative path against the canonical rules.
///
/// Byte-for-byte behavioural port of `musicpack_path_validate`. The input
/// is a Rust `&str`; paths originate from JSON strings, so they are Unicode
/// by the time they reach this function (see the UTF-8 strictness note in
/// `docs/architecture.md` — Open Questions).
pub fn validate(path: &str) -> Result<(), PathError> {
    let bytes = path.as_bytes();
    let err = |reason| PathError {
        path: path.to_string(),
        reason,
    };

    if bytes.is_empty() {
        return Err(err(PathReason::Empty));
    }
    if bytes.len() > PATH_MAX_BYTES {
        return Err(err(PathReason::TooLong));
    }

    let mut seg_start = 0usize;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'\\' {
            return Err(err(PathReason::Backslash));
        }
        if c == b':' {
            return Err(err(PathReason::Colon));
        }
        if c < 0x20 || c == 0x7f {
            return Err(err(PathReason::Control));
        }
        if c == b'/' {
            if i == 0 {
                return Err(err(PathReason::Absolute));
            }
            let segment = &bytes[seg_start..i];
            if segment.is_empty() {
                return Err(err(PathReason::EmptySegment));
            }
            if segment == b"." {
                return Err(err(PathReason::DotSegment));
            }
            if segment == b".." {
                return Err(err(PathReason::ParentSegment));
            }
            seg_start = i + 1;
        }
    }

    // Final segment (mirrors the C tail checks).
    if seg_start == bytes.len() {
        return Err(err(PathReason::TrailingSeparator));
    }
    let segment = &bytes[seg_start..];
    if segment == b"." {
        return Err(err(PathReason::DotSegment));
    }
    if segment == b".." {
        return Err(err(PathReason::ParentSegment));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_paths() {
        for p in [
            "manifest.json",
            "audio/01 - Alphaville - Big in Japan.mpc",
            "audio/01.mpc",
            "artwork/front.jpg",
            "analysis/waveform/01-01.wfm",
            "analysis/waveform/12-01.wfm",
            "analysis/sonic.json",
            "extras/.hidden.txt",
            "lyrics/03 - Café ♪.lrc",
            "a/b/c/d/e/f.txt",
        ] {
            assert!(validate(p).is_ok(), "should accept {p:?}");
        }
    }

    #[test]
    fn rejects_the_conformance_unsafe_paths() {
        // The `unsafe-path-*` family from the v1 conformance corpus
        // (generate_mpack_conformance.py), plus the classic Windows/URL
        // escape shapes the rules structurally exclude.
        for p in [
            "../x",
            "/tmp/x",
            "a\\b",
            "a//b",
            "a/./b",
            "a/../b",
            "",
            "audio/",
            "C:\\x",
            "C:/x",
            "\\\\server\\share",
            "http://example.com/a",
            "a:b",
            ".",
            "..",
            "a/.",
            "a/..",
            "a/\0b",
            "a\u{7f}b",
            "a\u{1f}b",
        ] {
            assert!(validate(p).is_err(), "should reject {p:?}");
        }
    }

    #[test]
    fn length_bound_is_measured_in_bytes() {
        let ok = "a".repeat(PATH_MAX_BYTES);
        assert!(validate(&ok).is_ok());

        let too_long = "a".repeat(PATH_MAX_BYTES + 1);
        assert_eq!(validate(&too_long).unwrap_err().reason, PathReason::TooLong);

        // Two-byte characters halve the character budget (C strlen parity).
        let ü = "ü".repeat(PATH_MAX_BYTES / 2 + 1);
        assert_eq!(validate(&ü).unwrap_err().reason, PathReason::TooLong);
    }

    #[test]
    fn reasons_are_diagnosed() {
        let reason_of = |p: &str| validate(p).unwrap_err().reason;
        assert_eq!(reason_of(""), PathReason::Empty);
        assert_eq!(reason_of("/abs"), PathReason::Absolute);
        assert_eq!(reason_of("a\\b"), PathReason::Backslash);
        assert_eq!(reason_of("a:b"), PathReason::Colon);
        assert_eq!(reason_of("a\u{1}b"), PathReason::Control);
        assert_eq!(reason_of("a//b"), PathReason::EmptySegment);
        assert_eq!(reason_of("./a"), PathReason::DotSegment);
        assert_eq!(reason_of("../a"), PathReason::ParentSegment);
        assert_eq!(reason_of("a/"), PathReason::TrailingSeparator);
    }
}
