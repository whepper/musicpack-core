//! Integration tests for the canonical package-relative path rules.
//!
//! The invalid set mirrors the `unsafe-path-*` family of the `.mpack` v1
//! conformance corpus (`tests/generate_mpack_conformance.py` in the
//! reference repository) plus the Windows/URL escape shapes the rules
//! exclude structurally. The valid set mirrors paths that occur in the
//! committed reference packages (`tests/reference/`).

use musicpack_core::format::path::{PathReason, validate};

const VALID_PATHS: &[&str] = &[
    // Reference-package shapes.
    "manifest.json",
    "audio/01 - Alphaville - Big in Japan.mpc",
    "audio/02 - Alphaville - Forever Young.mpc",
    "artwork/front.jpg",
    "booklet/booklet.pdf",
    "lyrics/03 - Café ♪.lrc",
    "extras/notes.txt",
    "analysis/waveform/01-01.wfm",
    "analysis/waveform/01-04.wfm",
    "analysis/waveform/12-01.wfm",
    "analysis/sonic.json",
    // Legal oddities.
    "extras/.hidden",
    "a/.../b", // '...' is a legal segment name
    "deep/nested/dir/structure/file.flac",
];

const INVALID_PATHS: &[&str] = &[
    // Conformance corpus unsafe-path-0..7.
    "../x",
    "/tmp/x",
    "a\\b",
    "a//b",
    "a/./b",
    "a/../b",
    "",
    "audio/",
    // Structural escapes.
    "C:\\Documents\\track.mpc",
    "C:/Documents/track.mpc",
    "\\\\server\\share\\track.mpc",
    "https://example.com/track.mpc",
    "file://host/path",
    "a:b",
    // Segment trickery.
    ".",
    "..",
    "./a",
    "../a",
    "a/.",
    "a/..",
    // Control characters.
    "a\0b",
    "a\u{1}b",
    "a\u{1f}b",
    "a\u{7f}b",
    "aud\rio/x",
];

#[test]
fn accepts_all_valid_paths() {
    for path in VALID_PATHS {
        assert!(validate(path).is_ok(), "unexpectedly rejected {path:?}");
    }
}

#[test]
fn rejects_all_invalid_paths() {
    for path in INVALID_PATHS {
        assert!(validate(path).is_err(), "unexpectedly accepted {path:?}");
    }
}

#[test]
fn boundary_length_in_bytes() {
    let max = "a".repeat(musicpack_core::limits::PATH_MAX_BYTES);
    assert!(validate(&max).is_ok());

    let over = "a".repeat(musicpack_core::limits::PATH_MAX_BYTES + 1);
    let err = validate(&over).unwrap_err();
    assert_eq!(err.reason, PathReason::TooLong);
    assert!(err.to_string().contains("path too long"));
}

#[test]
fn errors_convert_to_the_crate_error_type() {
    let err: musicpack_core::Error = validate("a/../b").unwrap_err().into();
    assert!(matches!(err, musicpack_core::Error::Path(_)));
    assert!(err.to_string().contains("unsafe path"));
}
