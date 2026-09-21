//! Authoring-pipeline errors.
//!
//! Stages fail distinctly so callers (the CLI, the future Tauri host) can
//! react per stage: a draft that does not parse, a draft that fails
//! validation, an unsupported source, an encoder failure, an identification
//! failure, a package-build failure, or a pack failure.
//!
//! Every message is actionable and includes disc/track context where a
//! track is the subject. The pipeline is fail-closed: any `Err` means no
//! package was published.

use std::fmt;

use musicpack_core::Error as CoreError;

/// An authoring-pipeline failure.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AuthorError {
    /// The draft JSON could not be read/parsed/mapped.
    Draft {
        /// What was wrong and where.
        detail: String,
    },
    /// The draft is structurally invalid (the `validate` stage).
    Validation {
        /// All validation errors.
        errors: Vec<String>,
        /// All non-fatal warnings.
        warnings: Vec<String>,
    },
    /// A track failed to decode/encode.
    Encode {
        /// Owning disc number.
        disc: i32,
        /// Owning track number.
        track: i32,
        /// What failed.
        detail: String,
    },
    /// The requested operation is not supported by the Rust pipeline (for
    /// example an encoder `(quality, sample-rate)` combination whose frozen
    /// tables do not exist yet).
    Unsupported {
        /// What is unsupported and why.
        detail: String,
    },
    /// MusicBrainz identification failed.
    Identification {
        /// What failed.
        detail: String,
    },
    /// The core package builder rejected the assembled package.
    Build(CoreError),
    /// The optional `.mpak` packing step failed.
    Pack {
        /// What failed.
        detail: String,
    },
    /// Filesystem failure inside the pipeline's own orchestration.
    Io {
        /// What failed.
        detail: String,
    },
    /// The caller cancelled the operation between tracks.
    Cancelled,
}

impl fmt::Display for AuthorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthorError::Draft { detail } => write!(f, "draft: {detail}"),
            AuthorError::Validation { errors, .. } => {
                write!(f, "draft validation failed: {}", errors.join("; "))
            }
            AuthorError::Encode {
                disc,
                track,
                detail,
            } => write!(f, "disc {disc} track {track}: {detail}"),
            AuthorError::Unsupported { detail } => write!(f, "unsupported: {detail}"),
            AuthorError::Identification { detail } => write!(f, "identification: {detail}"),
            AuthorError::Build(e) => write!(f, "package build failed: {e}"),
            AuthorError::Pack { detail } => write!(f, "pack failed: {detail}"),
            AuthorError::Io { detail } => write!(f, "I/O failure: {detail}"),
            AuthorError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for AuthorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuthorError::Build(e) => Some(e),
            _ => None,
        }
    }
}

impl From<CoreError> for AuthorError {
    fn from(e: CoreError) -> Self {
        AuthorError::Build(e)
    }
}

/// The result type used throughout `musicpack-author`.
pub type Result<T, E = AuthorError> = std::result::Result<T, E>;
