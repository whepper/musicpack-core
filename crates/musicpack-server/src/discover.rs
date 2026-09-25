//! Filesystem discovery — the bounded walk that finds library sources.
//!
//! This is the stage-2 half of the reference's `scanner.c` walk, stopping
//! before ingestion: it produces [`PackageCandidate`] values (locator +
//! manifest bytes + parsed manifest + identity keys) and never touches
//! SQLite. Ownership, conflicts, the missing-package sweep and every other
//! ingestion concern belong to stage 3.
//!
//! Rules preserved from the reference (`walk` / `mp_scan_library`):
//!
//! - only directories are walked; recursion stops at a directory whose
//!   **file name** ends in `.mpack` (byte-exact, case-sensitive) —
//!   `.mpack` contents are never descended into;
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
//! # Container sources (`.mpak`)
//!
//! A `.mpak` container is a **regular file**, not a `.mpack` directory, so the
//! reference's walk never sees one. This walker recognises it in exactly one
//! place, [`classify`]: a regular file whose name ends in `.mpak` is a library
//! source of kind [`SourceKind::Container`]. The Rust walker is therefore a
//! deliberate **superset** of the C reference; `tests/discovery.rs` pins that
//! difference, and its differential cross-check still agrees because it
//! compares only *valid* packages.
//!
//! Per-package manifest handling uses the core only:
//! [`DirectoryBackend`](musicpack_core::storage::directory::DirectoryBackend)
//! for the hardened `manifest.json` read, or
//! [`MpakBackend`](musicpack_core::storage::mpak::MpakBackend) for a
//! container's `MANF`, then the core strict parser, then the
//! [`identity`](crate::identity) keys. A package whose manifest cannot be
//! read or parsed is still reported — as
//! [`CandidateBody::Invalid`] — so stage 3 can apply the invalid-row rules.

use std::fmt;
use std::path::{Path, PathBuf};

use musicpack_core::format::manifest::Manifest;

use crate::identity;
use crate::source::PackageSource;

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

/// Something discovered on disk: a library source (a `.mpack` directory
/// bundle or a `.mpak` container) plus whatever stage 3 needs to decide its
/// fate. This is deliberately not a database model.
#[derive(Debug, Clone)]
pub struct PackageCandidate {
    /// The source's locator: the package directory, or the container file.
    pub path: PathBuf,
    /// Which kind of source this is.
    pub kind: SourceKind,
    /// SHA-256 of the raw manifest bytes (`""` when the manifest
    /// could not be read — the reference records `""` for unreadable
    /// packages and the real hash for unparsable ones).
    pub manifest_sha256: String,
    pub body: CandidateBody,
}

/// The physical shape of a library source.
///
/// Produced only by [`classify`] — the server's single classification point —
/// so no other code re-derives the source kind from a file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A `.mpack` directory bundle: `manifest.json` plus regular files.
    Directory,
    /// A single-file `.mpak` container: `MANF` plus a member table.
    Container,
}

impl SourceKind {
    /// `true` for a single-file container source.
    pub fn is_container(self) -> bool {
        matches!(self, SourceKind::Container)
    }
}

/// The classification of one walk entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryClass {
    /// A library source of this kind.
    Source(SourceKind),
    /// A directory to descend into.
    Descend,
    /// Not a source and not a directory: skip.
    Skip,
}

/// The server's **one** classification point: what is this entry?
///
/// Byte-exact and case-sensitive, like the reference's `ends_with` rule:
///
/// - a **directory** named `*.mpack` is a [`SourceKind::Directory`] source and
///   is never descended into;
/// - a **regular file** named `*.mpak` is a [`SourceKind::Container`] source;
/// - a plain directory is [`EntryClass::Descend`]; anything else is skipped.
///
/// The entry's type is a parameter rather than a stat: the walker has already
/// stated it, and keeping the type out of here makes this function pure and
/// trivially testable. A `.mpack` *file* and a `.mpak` *directory* are both
/// skipped — a source kind that does not match its physical shape is not a
/// source.
pub fn classify(name: &std::ffi::OsStr, is_dir: bool, is_file: bool) -> EntryClass {
    let bytes = name.as_encoded_bytes();
    if is_dir && bytes.ends_with(b".mpack") {
        return EntryClass::Source(SourceKind::Directory);
    }
    if is_file && bytes.ends_with(b".mpak") {
        return EntryClass::Source(SourceKind::Container);
    }
    if is_dir {
        return EntryClass::Descend;
    }
    EntryClass::Skip
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
            // One classification point for the whole server: a `.mpack`
            // directory bundle, a `.mpak` container file, a directory to
            // descend into, or nothing at all.
            match classify(&name, meta.is_dir(), meta.is_file()) {
                EntryClass::Source(kind) => {
                    self.budget.packages += 1;
                    if self.budget.packages > MAX_PACKAGES {
                        return Err(DiscoverError::BudgetExceeded {
                            detail: format!("more than {MAX_PACKAGES} packages"),
                        });
                    }
                    self.candidates.push(examine_source(next, kind));
                }
                EntryClass::Descend => {
                    self.walk_dir(&next, depth + 1)?;
                }
                EntryClass::Skip => {}
            }
        }
        Ok(())
    }
}

/// The source kind of an already-recorded package locator.
///
/// Reuses [`classify`] so the verify pass cannot drift from the walk: a
/// `.mpak` file is a container and a `.mpack` directory is a bundle, and
/// anything else (a locator that no longer has the right shape) falls back to
/// [`SourceKind::Directory`], which is what every package the C reference knew
/// about was.
pub fn classify_source_path(path: &Path) -> SourceKind {
    let name = path.file_name().unwrap_or(path.as_os_str());
    match classify(name, path.is_dir(), path.is_file()) {
        EntryClass::Source(kind) => kind,
        _ => SourceKind::Directory,
    }
}

/// Reads a discovered source's manifest and derives its identity keys.
///
/// Both source kinds go through [`PackageSource`], so the manifest bytes, the
/// manifest hash and the identity derivation are literally the same code for a
/// directory bundle and a container.
fn examine_source(path: PathBuf, kind: SourceKind) -> PackageCandidate {
    let source = match kind {
        SourceKind::Directory => PackageSource::directory(path.clone()),
        SourceKind::Container => PackageSource::container(path.clone()),
    };
    let (manifest_sha256, body) = match source.read_manifest() {
        Ok(read) => {
            let manifest = read.manifest;
            let body = match derive_identity(&manifest) {
                Ok((fingerprint, group_key, release_key)) => CandidateBody::Valid {
                    manifest: Box::new(manifest),
                    fingerprint,
                    group_key,
                    release_key,
                },
                Err(detail) => CandidateBody::Invalid(InvalidReason::InvalidManifest(detail)),
            };
            (read.manifest_sha256, body)
        }
        Err(crate::source::SourceError::UnreadableManifest(detail)) => (
            String::new(),
            CandidateBody::Invalid(InvalidReason::UnreadableManifest(detail)),
        ),
        Err(crate::source::SourceError::InvalidManifest(detail)) => {
            // The bytes were read but did not parse: record the real hash of
            // them, exactly as the reference does for a malformed
            // `manifest.json`.
            let sha = source.raw_manifest_hash().unwrap_or_default();
            (
                sha,
                CandidateBody::Invalid(InvalidReason::InvalidManifest(detail)),
            )
        }
    };
    PackageCandidate {
        path,
        kind,
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
        let dir = |name: &str| classify(OsStr::new(name), true, false);
        let file = |name: &str| classify(OsStr::new(name), false, true);
        assert_eq!(
            dir("album.mpack"),
            EntryClass::Source(SourceKind::Directory)
        );
        assert_eq!(dir(".mpack"), EntryClass::Source(SourceKind::Directory));
        assert_eq!(dir("album.MPACK"), EntryClass::Descend);
        assert_eq!(dir("album.mpack2"), EntryClass::Descend);
        assert_eq!(dir("mpack"), EntryClass::Descend);
        assert_eq!(
            file("album.mpak"),
            EntryClass::Source(SourceKind::Container)
        );
        assert_eq!(file("album.MPAK"), EntryClass::Skip);
        assert_eq!(file("album.mpak2"), EntryClass::Skip);
        assert_eq!(file("mpak"), EntryClass::Skip);
        // A kind that does not match its physical shape is not a source: a
        // `.mpack` file and a `.mpak` directory are both skipped.
        assert_eq!(file("album.mpack"), EntryClass::Skip);
        assert_eq!(dir("album.mpak"), EntryClass::Descend);
    }

    #[test]
    fn a_recorded_locator_classifies_by_its_own_shape() {
        // The verify pass recovers a package's kind from its stored locator
        // through this same classifier, so it cannot drift from the walk.
        let root = std::env::temp_dir().join(format!("classify-source-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join("a.mpack");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(classify_source_path(&dir), SourceKind::Directory);
        let file = root.join("a.mpak");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(classify_source_path(&file), SourceKind::Container);
        // A locator that no longer has the right shape falls back to the
        // reference's only kind.
        assert_eq!(
            classify_source_path(&root.join("gone.mpack")),
            SourceKind::Directory
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
