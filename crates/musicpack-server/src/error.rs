//! Server error taxonomy: a small hand-rolled enum, matching the repo
//! convention (no error-framework dependency) and the reference CLI's exit
//! classes.

use std::fmt;

/// Everything that can go wrong in the stage-1 server surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// A persistence-layer failure (message carries the SQLite context).
    Store(String),
    /// The database was written by a newer schema than this server
    /// understands. Fail closed; never guess.
    DatabaseTooNew { found: i64, supported: i64 },
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::Store(detail) => write!(f, "database error: {detail}"),
            ServerError::DatabaseTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than the supported \
                 version {supported}; upgrade musicpack-server"
            ),
        }
    }
}

impl std::error::Error for ServerError {}

/// CLI-level outcomes. Usage problems exit 2, failures exit 1, and
/// not-implemented commands exit 2 with an explicit notice (the reference
/// reserves 2 for "the invocation could not be served"). The help/version
/// variants are successful outcomes surfaced through the parser; they exit
/// 0 exactly like the reference's getopt handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// Bad invocation (unknown command, missing/invalid option value).
    Usage(String),
    /// The command ran but failed.
    Failure(String),
    /// `--help`/`-h` was seen; print usage and exit 0.
    HelpRequested,
    /// `--version` was seen; print the banner and exit 0.
    VersionRequested,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_new_message_names_both_versions() {
        let e = ServerError::DatabaseTooNew {
            found: 11,
            supported: 10,
        };
        let message = e.to_string();
        assert!(message.contains("11"));
        assert!(message.contains("10"));
        assert!(message.contains("newer"));
    }
}
