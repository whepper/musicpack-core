//! Error taxonomy for `musicpack-core`.
//!
//! The categories mirror the `musicpack_status` codes of the C reference
//! (`core/libmusicpack/include/musicpack/error.h`) so that behaviour can be
//! mapped one-to-one during differential testing:
//!
//! | C status | Rust variant |
//! |----------|--------------|
//! | `MUSICPACK_ERR_INVALID` | [`Error::Invalid`] |
//! | `MUSICPACK_ERR_JSON` | [`Error::Json`] |
//! | `MUSICPACK_ERR_VERSION` | [`Error::Version`] |
//! | `MUSICPACK_ERR_IO` | [`Error::Io`] |
//! | `MUSICPACK_ERR_NOMEM` | allocation failure (panic in Rust; not modelled) |
//! | `MUSICPACK_ERR_CHECKSUM` | [`Error::Checksum`] |
//! | `MUSICPACK_ERR_PATH` | [`Error::Path`] |
//! | `MUSICPACK_ERR_MISSING` | [`Error::Missing`] |
//!
//! [`Error::Unsupported`] has no direct C equivalent; it exists so the
//! Rust parser can report unsupported features without overloading
//! `Invalid`. Resource-budget violations (manifest size, asset counts,
//! array caps) are reported as [`Error::Invalid`] — exactly the status the
//! C reference returns for them — with `"exceeds"` phrasing in the detail.
//!
//! # Message stability
//!
//! The C test suites assert *stderr substrings* on failure (for example
//! `"checksum mismatch"`, `"missing file"`, `"exceeds"`). A future
//! `musicpack` CLI built on this crate must keep producing those phrases,
//! so the [`std::fmt::Display`] renderings here intentionally use that
//! vocabulary. Treat the wording as soft-compatible surface: changing it is
//! a compatibility decision, not a cosmetic one.

use std::fmt;

use crate::format::path::PathError;
use crate::json::JsonError;

/// The result type used throughout `musicpack-core`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors produced while parsing, validating, or writing MusicPack data.
///
/// The enum is `#[non_exhaustive]`: new variants will be added as parser
/// and validation phases land. No variant carries floating-point data, so
/// the type is [`Eq`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Malformed content or an invalid argument (`MUSICPACK_ERR_INVALID`).
    Invalid {
        /// What was invalid and why.
        detail: String,
    },
    /// The input is not well-formed strict JSON (`MUSICPACK_ERR_JSON`).
    Json {
        /// Parse failure description (position, reason).
        detail: String,
    },
    /// Unsupported format identity or schema version (`MUSICPACK_ERR_VERSION`).
    Version {
        /// The version value found in the input, as rendered.
        found: String,
        /// The version this build supports.
        supported: u64,
    },
    /// Backing storage or transport failure (`MUSICPACK_ERR_IO`).
    Io {
        /// I/O failure description.
        detail: String,
    },
    /// A referenced object's SHA-256 does not match its declaration
    /// (`MUSICPACK_ERR_CHECKSUM`).
    Checksum {
        /// Package-relative path of the object.
        path: String,
        /// Declared (expected) digest, lowercase hex.
        expected: String,
        /// Computed (actual) digest, lowercase hex.
        actual: String,
    },
    /// A manifest-referenced object is absent (`MUSICPACK_ERR_MISSING`).
    Missing {
        /// Package-relative path of the object.
        path: String,
    },
    /// A package-relative path violates the canonical rules
    /// (`MUSICPACK_ERR_PATH`).
    Path(PathError),
    /// A well-formed but unsupported feature (e.g. a future container major).
    Unsupported {
        /// What is unsupported.
        what: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid { detail } => write!(f, "invalid: {detail}"),
            Error::Json { detail } => write!(f, "malformed JSON: {detail}"),
            Error::Version { found, supported } => {
                write!(f, "unsupported version {found} (supported: {supported})")
            }
            Error::Io { detail } => write!(f, "I/O failure: {detail}"),
            Error::Checksum {
                path,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch: {path}: expected {expected}, got {actual}"
            ),
            Error::Missing { path } => write!(f, "missing file: {path}"),
            Error::Path(e) => write!(f, "{e}"),
            Error::Unsupported { what } => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Path(e) => Some(e),
            _ => None,
        }
    }
}

impl From<PathError> for Error {
    fn from(e: PathError) -> Self {
        Error::Path(e)
    }
}

impl From<JsonError> for Error {
    fn from(e: JsonError) -> Self {
        Error::Json {
            detail: e.to_string(),
        }
    }
}
