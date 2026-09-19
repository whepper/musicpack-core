//! Platform-independent storage seam for package verification.
//!
//! The domain core never touches a filesystem directly. Verification asks a
//! [`PackageBackend`] for manifest-referenced objects and applies the
//! MusicPack semantics (existence, size budgets, SHA-256, waveform payload
//! consistency, unreferenced-file warnings) on top.
//!
//! Implementations planned or present:
//!
//! | Backend | Status | Security policy |
//! |---------|--------|-----------------|
//! | Directory bundle ([`directory`]) | reference adapter, `#[cfg(unix)]` | containment resolution, regular-file + link-count checks (port of `package.c`) |
//! | MPAK container | phase 6 | member-range reads; container rules |
//! | Server storage | later | inherits the adapter's verified-only discipline |
//! | Browser/WebAssembly | later | in-memory / OPFS-backed byte sources |
//!
//! The trait is deliberately small: opening an object (with the backend's
//! security checks bound to the opened handle), an optional stable object
//! identity (the reference's inode dedup), an optional regular-file listing
//! (unreferenced-file warnings), and the storage's own meta file names
//! (excluded from those warnings, e.g. `manifest.json`).

use std::io::Read;

#[cfg(unix)]
pub mod directory;
pub mod mpak;

/// A stable object identity used to avoid hashing the same underlying file
/// twice within one verification pass.
///
/// The reference implementation records `(st_dev, st_ino)` (POSIX only;
/// disabled on Windows, where `st_ino` is unreliable). Backends that cannot
/// provide a trustworthy identity return `None` from
/// [`PackageBackend::object_id`], which disables deduplication exactly like
/// the reference's MPAK/Windows paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    /// Device id (`st_dev`).
    pub device: u64,
    /// Inode number (`st_ino`).
    pub inode: u64,
}

/// Why a backend could not open a manifest-referenced object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendError {
    /// The path traverses or resolves outside the package root
    /// (containment violation). Reported as `unsafe path`.
    UnsafePath,
    /// The object is absent, is not a regular file, is a symlink, or has
    /// more than one hard link. The reference implementation reports all
    /// of these as `missing file`.
    Missing,
    /// An I/O failure while opening the object.
    Io(String),
}

/// An opened package object.
///
/// `len` is the size observed on the opened handle (bound to the same
/// object the reader reads from). Callers that only need the size may drop
/// the reader without reading.
pub struct OpenedAsset {
    /// Size in bytes, from the opened handle's metadata.
    pub len: u64,
    /// Streaming reader over the object's bytes.
    pub reader: Box<dyn Read>,
}

/// A sink for backend-specific verification findings.
///
/// The validation layer owns the report; the storage layer owns
/// container-specific checks (e.g. MPAK `INDX`/`TAIL` consistency). This
/// trait inverts the dependency so a backend can report findings without
/// the storage module depending on `validation`.
pub trait VerificationSink {
    /// Records a fatal finding.
    fn error(&mut self, message: &str);
    /// Records a non-fatal finding.
    fn warning(&mut self, message: &str);
}

/// A storage backend that exposes a package's objects to the verifier.
pub trait PackageBackend {
    /// Opens `path` (a canonical package-relative path validated at parse
    /// time) with this backend's security policy.
    ///
    /// Each call performs its own open: the reference verifier queries the
    /// size and later re-opens to hash, so a changed object between the two
    /// calls is observed as it would be there.
    fn open_asset(&self, path: &str) -> Result<OpenedAsset, BackendError>;

    /// A stable identity for `path`, when the backend has one. Used only
    /// for same-pass deduplication; `None` disables deduplication.
    fn object_id(&self, path: &str) -> Option<ObjectId>;

    /// Lists every regular file in the package as a canonical relative
    /// path. Used for the unreferenced-file warning. Backends without file
    /// enumeration return an empty list.
    fn list_files(&self) -> Vec<String> {
        Vec::new()
    }

    /// Names that belong to the storage itself rather than package content
    /// and are therefore not "unreferenced files" (a directory bundle's
    /// `manifest.json`). MPAK returns nothing (its `MANF` is not a member).
    fn meta_files(&self) -> &[&str] {
        &[]
    }

    /// Backend-specific verification findings, appended after the generic
    /// asset checks and the unreferenced-file scan.
    ///
    /// The reference calls the MPAK equivalent
    /// (`musicpack_mpak_verify_extra`) at exactly this point: container
    /// consistency (INDX/TAIL/version/reserved/resync/duplicates) is the
    /// backend's concern, while SHA-256, budgets and manifest semantics
    /// stay in the generic verifier.
    fn verify_extra(
        &self,
        _manifest: &crate::format::manifest::Manifest,
        _sink: &mut dyn VerificationSink,
    ) {
    }
}
