//! Filesystem discovery — the bounded walk that finds `.mpack` packages.
//!
//! This is the stage-2 half of the reference's `scanner.c` walk, stopping
//! before ingestion: it produces [`PackageCandidate`] values (path +
//! manifest bytes + parsed manifest + identity keys) and never touches
//! SQLite. Ownership, conflicts, the missing-package sweep and every other
//! ingestion concern belong to stage 3.
//!
//! Rules preserved from the reference (`walk` / `mp_scan_library`):
//!
//! - only directories are considered; recursion stops at a directory whose
//!   **file name** ends in `.mpack` (byte-exact, case-sensitive) —
//!   `.mpack` contents are never descended into, and `.mpak` (or any other
//!   file) is never a package;
//! - symlinks are never followed (POSIX `lstat`; skipped before any type
//!   decision), so traversal cannot escape the library root;
//! - entries that cannot be stated, overlong paths, and over-deep trees
//!   fail the whole discovery (fail-closed, like the reference);
//! - budgets abort the whole discovery: 100 000 filesystem objects,
//!   10 000 packages, 64 MiB of cumulative path bytes;
//! - the candidate set is sorted by path before it is returned. The
//!   reference enumerates in OS (`readdir`) order; sorting normalizes that
//!   without altering semantics (no ingestion order exists yet).
//!
//! Per-package manifest handling uses the core only:
//! [`DirectoryBackend`](musicpack_core::storage::directory::DirectoryBackend)
//! for the hardened `manifest.json` read, the core strict parser, and the
//! [`identity`](crate::identity) keys. A package whose manifest cannot be
//! read or parsed is still reported — as
//! [`CandidateBody::Invalid`] — so stage 3 can apply the invalid-row rules.

use std::fmt;
use std::path::{Path, PathBuf};

use musicpack_core::format::manifest::Manifest;
use musicpack_core::format::manifest::ParsedManifest;
use musicpack_core::storage::directory::DirectoryBackend;

use crate::identity;

/// Maximum scan depth (`MAX_SCAN_DEPTH`).
pub const MAX_DEPTH: u32 = 64;
/// Maximum filesystem objects visited (`MAX_SCAN_OBJECTS`).
pub const MAX_OBJECTS: u64 = 100_000;
/// Maximum `.mpack` packages collected (`MAX_SCAN_PACKAGES`).
pub const MAX_PACKAGES: u64 = 10_000;
/// Maximum cumulative path bytes (`MAX_SCAN_PATH_BYTES`).
pub const MAX_PATH_BYTES: u64 = 64 * 1024 * 1024;

/// Why a discovered package carries no identity keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    /// `manifest.json` could not be read under the hardened rules.
    UnreadableManifest(String),
    /// The manifest bytes do not parse as a valid `.mpack` manifest.
    InvalidManifest(String),
}

/// The validated body of a discovered package.
///
/// The parsed manifest is boxed: the `Valid` variant would otherwise be
/// ~700 bytes next to the small `Invalid` reason (clippy
/// `large_enum_variant`), and every candidate pays for the larger variant.
#[derive(Debug, Clone)]
pub enum CandidateBody {
    /// Parsed manifest plus the derived identity keys.
    Valid {
        manifest: Box<Manifest>,
        fingerprint: String,
        group_key: String,
        release_key: String,
    },
    /// The package exists but yields no keys (stage 3 maps this to the
    /// invalid-row rules; discovery itself never drops it).
    Invalid(InvalidReason),
}

/// Something discovered on disk: a `.mpack` directory plus whatever stage 3
/// needs to decide its fate. This is deliberately not a database model.
#[derive(Debug, Clone)]
pub struct PackageCandidate {
    /// The package directory, as discovered under the walk root.
    pub path: PathBuf,
    /// SHA-256 of the raw `manifest.json` bytes (`""` when the manifest
    /// could not be read — the reference records `""` for unreadable
    /// packages and the real hash for unparsable ones).
    pub manifest_sha256: String,
    pub body: CandidateBody,
}

/// Why discovery failed outright (fail-closed, like the reference's
/// aborted scan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoverError {
    /// The walk root could not be opened or enumerated.
    CannotOpenRoot { path: PathBuf, detail: String },
    /// A subtree could not be stated, a path was overlong, or the depth
    /// limit was hit — the traversal is incomplete, so no partial result
    /// is returned.
    IncompleteTraversal { detail: String },
    /// A resource budget was exceeded.
    BudgetExceeded { detail: String },
}

impl std::fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoverError::CannotOpenRoot { path, detail } => {
                write!(f, "cannot open library root '{}': {detail}", path.display())
            }
            DiscoverError::IncompleteTraversal { detail } => {
                write!(f, "library traversal incomplete: {detail}")
            }
            DiscoverError::BudgetExceeded { detail } => {
                write!(f, "scan resource budget exceeded: {detail}")
            }
        }
    }
}

impl std::error::Error for DiscoverError {}

struct Budget {
    objects: u64,
    packages: u64,
    path_bytes: u64,
}

struct Walker {
    budget: Budget,
    failed: bool,
    candidates: Vec<PackageCandidate>,
}

impl Walker {
    fn account(&mut self, path: &Path) -> Result<(), DiscoverError> {
        self.budget.objects += 1;
        self.budget.path_bytes += path.as_os_str().as_encoded_bytes().len() as u64;
        if self.budget.objects > MAX_OBJECTS || self.budget.path_bytes > MAX_PATH_BYTES {
            return Err(DiscoverError::BudgetExceeded {
                detail: format!(
                    "walked {} objects, {} path bytes",
                    self.budget.objects, self.budget.path_bytes
                ),
            });
        }
        Ok(())
    }

    fn walk_dir(&mut self, dir: &Path, depth: u32) -> Result<(), DiscoverError> {
        if depth >= MAX_DEPTH {
            self.failed = true;
            return Ok(());
        }
        let entries = std::fs::read_dir(dir).map_err(|_| {
            self.failed = true;
        });
        let Ok(entries) = entries else {
            return Ok(());
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    self.failed = true;
                    continue;
                }
            };
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let next = dir.join(&name);
            // The reference rejects paths that do not fit its fixed buffer.
            if next.as_os_str().as_encoded_bytes().len() >= 4096 + 2 {
                self.failed = true;
                continue;
            }
            self.account(&next)?;
            // Symlinks are never followed (the reference's lstat rule).
            let meta = match std::fs::symlink_metadata(&next) {
                Ok(m) => m,
                Err(_) => {
                    self.failed = true;
                    continue;
                }
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if !meta.is_dir() {
                continue;
            }
            // A directory whose entry name ends in `.mpack` is a package
            // and is never descended into (the reference's `ends_with`
            // rule, byte-exact on the name).
            if name_is_mpack_dir(&name) {
                self.budget.packages += 1;
                if self.budget.packages > MAX_PACKAGES {
                    return Err(DiscoverError::BudgetExceeded {
                        detail: format!("more than {MAX_PACKAGES} packages"),
                    });
                }
                self.candidates.push(examine_package(next));
            } else {
                self.walk_dir(&next, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// The reference matches `ends_with(d_name, ".mpack")` on the entry name:
/// byte-exact and case-sensitive. Only directories ever reach this check
/// (files are filtered above), so no extension parsing is involved.
fn name_is_mpack_dir(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().ends_with(b".mpack")
}

fn examine_package(path: PathBuf) -> PackageCandidate {
    let (manifest_sha256, body) = match DirectoryBackend::open(&path) {
        Ok(backend) => {
            let bytes = backend.manifest_bytes();
            let manifest_sha256 = crate::identity::manifest_hash(bytes);
            let body = match ParsedManifest::parse(bytes) {
                Ok(parsed) => {
                    let manifest = parsed.manifest().clone();
                    match derive_identity(&manifest) {
                        Ok((fingerprint, group_key, release_key)) => CandidateBody::Valid {
                            manifest: Box::new(manifest),
                            fingerprint,
                            group_key,
                            release_key,
                        },
                        Err(detail) => {
                            CandidateBody::Invalid(InvalidReason::InvalidManifest(detail))
                        }
                    }
                }
                Err(e) => CandidateBody::Invalid(InvalidReason::InvalidManifest(e.to_string())),
            };
            (manifest_sha256, body)
        }
        Err(e) => (
            String::new(),
            CandidateBody::Invalid(InvalidReason::UnreadableManifest(e.to_string())),
        ),
    };
    PackageCandidate {
        path,
        manifest_sha256,
        body,
    }
}

fn derive_identity(manifest: &Manifest) -> Result<(String, String, String), String> {
    let fingerprint = identity::package_fingerprint(manifest).map_err(|e| e.to_string())?;
    Ok((
        fingerprint,
        identity::group_key(manifest),
        identity::release_key(manifest),
    ))
}

/// Discovers `.mpack` package candidates under `root`.
///
/// Returns the candidates sorted by path. Any traversal failure or budget
/// breach fails the whole call — there are no partial results, exactly
/// like the reference's aborted scan.
pub fn discover(root: &Path) -> Result<Vec<PackageCandidate>, DiscoverError> {
    let mut walker = Walker {
        budget: Budget {
            objects: 0,
            packages: 0,
            path_bytes: 0,
        },
        failed: false,
        candidates: Vec::new(),
    };
    // The root itself is enumerated, never treated as a package — mirroring
    // the reference, which opens the root and walks its children.
    if !root.is_dir() {
        return Err(DiscoverError::CannotOpenRoot {
            path: root.to_path_buf(),
            detail: "not a directory".into(),
        });
    }
    walker.walk_dir(root, 0)?;
    if walker.failed {
        return Err(DiscoverError::IncompleteTraversal {
            detail: "one or more entries could not be visited".into(),
        });
    }
    walker.candidates.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(walker.candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_match_the_reference() {
        assert_eq!(MAX_DEPTH, 64);
        assert_eq!(MAX_OBJECTS, 100_000);
        assert_eq!(MAX_PACKAGES, 10_000);
        assert_eq!(MAX_PATH_BYTES, 64 * 1024 * 1024);
    }

    #[test]
    fn missing_root_is_an_error_not_an_empty_set() {
        let err = discover(Path::new("/nonexistent-musicpack-root-xyz")).unwrap_err();
        assert!(matches!(err, DiscoverError::CannotOpenRoot { .. }));
    }

    #[test]
    fn suffix_rule_is_byte_exact() {
        use std::ffi::OsStr;
        assert!(name_is_mpack_dir(OsStr::new("album.mpack")));
        assert!(name_is_mpack_dir(OsStr::new(".mpack")));
        assert!(!name_is_mpack_dir(OsStr::new("album.MPACK")));
        assert!(!name_is_mpack_dir(OsStr::new("album.mpack2")));
        assert!(!name_is_mpack_dir(OsStr::new("mpack")));
    }
}
