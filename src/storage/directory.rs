//! Directory-bundle backend: the reference adapter for native targets.
//!
//! Port of the directory half of `core/libmusicpack/src/package.c`:
//! containment resolution, hardened regular-file opening, link-count
//! rejection, streaming hashing and regular-file enumeration.
//!
//! # Security policy (per referenced path)
//!
//! 1. **Containment** (`musicpack_path_resolve`): the package root is
//!    canonicalized once; every *existing* path prefix is canonicalized and
//!    must stay within the canonical root. A prefix that does not exist
//!    stops the walk (remaining components cannot escape). Resolution is
//!    pathname-based, exactly like the reference (not `openat`-relative).
//! 2. **Final-component symlinks are rejected on unix.** The reference opens
//!    with `O_NOFOLLOW`; a symlink is therefore indistinguishable from a
//!    missing file. This port rejects a symlink before opening and
//!    classifies it as [`BackendError::Missing`], matching the observable
//!    outcome. On Windows the reference uses a following `_stat` instead,
//!    so symlinks that resolve to regular files are accepted there (see
//!    "Windows semantics" below); containment still rejects escapes.
//! 3. **Regular files only, link count ≤ 1 on unix.** Directories, FIFOs,
//!    sockets and device nodes are rejected before opening (a FIFO would
//!    otherwise block a read open); the opened handle's metadata is then
//!    checked again so the regular-file/link-count/size facts are bound to
//!    the object actually being read, as the reference's `fstat` does.
//!    `nlink > 1` treats any hard-linked object as an outside alias. The
//!    reference disables the link-count check on Windows (no `st_nlink`
//!    there), so this port does the same.
//!
//! # Documented TOCTOU limitation
//!
//! The reference closes the final-symlink race with `O_NOFOLLOW`; this port
//! uses a `lstat` check followed by `open` (std-only, no `libc`), so a path
//! swapped to a symlink *between* those two syscalls would be followed. The
//! check that binds facts to the opened handle (regular file, `nlink`,
//! size) still applies, and the reference itself documents that containment
//! is pathname-based rather than descriptor-relative. This window is
//! documented rather than hidden (see `docs/architecture.md`).
//!
//! # Windows semantics
//!
//! The reference genuinely behaves differently on Windows, and this port
//! matches it instead of pretending POSIX hardening exists there:
//!
//! - type checks use following metadata (`_stat` semantics): symlinks that
//!   resolve to regular files are accepted; directories, missing objects
//!   and non-regular files are still rejected;
//! - no hard-link (`nlink`) rejection;
//! - no inode dedup (`object_id` returns `None`, which disables
//!   deduplication exactly like the reference's Windows path);
//! - no unreferenced-file walk (`list_files` returns no files, so no
//!   warnings — the reference skips the walk on Windows).
//!
//! Rationale and decision record: `docs/adr/0015-windows-directory-adapter.md`.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

// Unix-only hardening primitives (`MetadataExt::nlink/dev/ino`,
// `FileTypeExt` special-file kinds). Windows follows the reference's
// relaxed `_stat` path instead (see the module docs).
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};

use crate::error::Error;
use crate::limits::MANIFEST_MAX_BYTES;
// `PATH_MAX_BYTES` bounds the unix-only file enumeration walk.
#[cfg(unix)]
use crate::limits::PATH_MAX_BYTES;

use super::{BackendError, ObjectId, OpenedAsset, PackageBackend};

/// Name of the manifest file inside a directory bundle.
pub const MANIFEST_NAME: &str = "manifest.json";

/// A directory-bundle package opened for reading and verification.
pub struct DirectoryBackend {
    root: PathBuf,
    root_real: PathBuf,
    manifest: Vec<u8>,
}

impl DirectoryBackend {
    /// Opens a directory bundle: checks the root is a directory and reads
    /// `manifest.json` with the reference's hardened regular-file rules
    /// (regular file, no symlink, link count ≤ 1, ≤ 16 MiB, no NUL byte).
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref().to_path_buf();
        // The reference stats the root with `stat` on Windows (following a
        // symlinked root) and `lstat` semantics elsewhere; match it.
        #[cfg(unix)]
        let meta = fs::symlink_metadata(&root).map_err(|_| Error::Missing {
            path: root.display().to_string(),
        })?;
        #[cfg(not(unix))]
        let meta = fs::metadata(&root).map_err(|_| Error::Missing {
            path: root.display().to_string(),
        })?;
        if !meta.is_dir() {
            return Err(Error::Invalid {
                detail: format!("package root '{}' is not a directory", root.display()),
            });
        }
        let root_real = fs::canonicalize(&root).map_err(|e| Error::Io {
            detail: format!("cannot resolve package root '{}': {e}", root.display()),
        })?;

        let manifest_path = root.join(MANIFEST_NAME);
        let manifest = read_manifest_hardened(&manifest_path)?;

        Ok(Self {
            root,
            root_real,
            manifest,
        })
    }

    /// The raw `manifest.json` bytes read at open time.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    /// Re-reads `manifest.json` with the hardened rules (used by the packer
    /// to detect the manifest changing between verification and packing,
    /// mirroring the reference's separate read).
    pub fn reread_manifest(&self) -> Result<Vec<u8>, Error> {
        read_manifest_hardened(&self.root.join(MANIFEST_NAME))
    }

    /// The package root as given.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a canonical relative path to an absolute path inside the
    /// package, applying the containment rules. `Err(())` means unsafe.
    fn resolve(&self, rel: &str) -> Result<PathBuf, ()> {
        let mut prefix = PathBuf::new();
        for segment in rel.split('/') {
            prefix.push(segment);
            match fs::canonicalize(self.root_real.join(&prefix)) {
                Ok(resolved) => {
                    // Component-wise containment (never a raw string prefix):
                    // `/pkg/ab` must not "contain" a neighbour `/pkg/abc`.
                    if !resolved.starts_with(&self.root_real) {
                        return Err(());
                    }
                }
                // Any resolution failure (typically ENOENT) stops the walk,
                // exactly like the reference's `check_existing_ancestors`.
                Err(_) => break,
            }
        }
        Ok(self.root_real.join(rel))
    }

    /// Regular-file enumeration (unix only; see [`Self::list_files`]).
    #[cfg(unix)]
    fn walk_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let mut stack = vec![(self.root.clone(), String::new())];
        while let Some((dir, rel_base)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let rel = if rel_base.is_empty() {
                    name.to_string()
                } else {
                    format!("{rel_base}/{name}")
                };
                if rel.len() > PATH_MAX_BYTES {
                    continue;
                }
                // symlink_metadata (lstat) semantics: links are skipped
                // like the reference's lstat walk, never followed.
                let Ok(lmeta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                let ft = lmeta.file_type();
                if ft.is_dir() {
                    stack.push((entry.path(), rel));
                } else if lmeta.is_file() {
                    files.push(rel);
                }
            }
        }
        // Deterministic order (the reference's readdir order is
        // filesystem-dependent and not an API).
        files.sort();
        files
    }
}

impl PackageBackend for DirectoryBackend {
    fn open_asset(&self, path: &str) -> Result<OpenedAsset, BackendError> {
        let abs = self.resolve(path).map_err(|_| BackendError::UnsafePath)?;

        // Type decision before opening. On unix, symlinks are missing
        // (O_NOFOLLOW semantics) and special files must never be opened (a
        // FIFO read open would block). On Windows the reference follows
        // with `_stat`, so only the regular-file test applies there.
        #[cfg(unix)]
        let lmeta = fs::symlink_metadata(&abs).map_err(|_| BackendError::Missing)?;
        #[cfg(not(unix))]
        let lmeta = fs::metadata(&abs).map_err(|_| BackendError::Missing)?;
        let ft = lmeta.file_type();
        if ft.is_symlink() || !lmeta.is_file() {
            return Err(BackendError::Missing);
        }
        #[cfg(unix)]
        if ft.is_fifo() || ft.is_socket() || ft.is_block_device() || ft.is_char_device() {
            return Err(BackendError::Missing);
        }

        let file = File::open(&abs).map_err(|_| BackendError::Missing)?;
        // Bind the accepted facts to the opened handle (reference `fstat`).
        let meta = file
            .metadata()
            .map_err(|e| BackendError::Io(e.to_string()))?;
        if !meta.is_file() {
            return Err(BackendError::Missing);
        }
        // The reference disables hard-link rejection on Windows.
        #[cfg(unix)]
        if meta.nlink() > 1 {
            return Err(BackendError::Missing);
        }
        Ok(OpenedAsset {
            len: meta.len(),
            reader: Box::new(file),
        })
    }

    fn object_id(&self, path: &str) -> Option<ObjectId> {
        // Stable identity for same-pass dedup. Off unix there is none: the
        // reference disables inode dedup on Windows (`st_ino` is
        // unreliable there), and returning `None` disables dedup the same
        // way (see `ObjectId`).
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
        #[cfg(unix)]
        {
            // lstat on the resolved path (reference `inode_of`).
            let abs = self.resolve(path).ok()?;
            let meta = fs::symlink_metadata(abs).ok()?;
            Some(ObjectId {
                device: meta.dev(),
                inode: meta.ino(),
            })
        }
    }

    fn list_files(&self) -> Vec<String> {
        // The reference skips the unreferenced-file walk on Windows, so no
        // files means no warnings there — exactly its observable behavior.
        #[cfg(not(unix))]
        {
            Vec::new()
        }
        #[cfg(unix)]
        {
            self.walk_files()
        }
    }

    fn meta_files(&self) -> &[&str] {
        &[MANIFEST_NAME]
    }
}

/// Reads `manifest.json` with the reference's hardened rules.
pub(crate) fn read_manifest_hardened(path: &Path) -> Result<Vec<u8>, Error> {
    // The reference stats with `stat` on Windows (following) and `lstat`
    // semantics elsewhere; match it.
    #[cfg(unix)]
    let meta = fs::symlink_metadata(path).map_err(|_| Error::Missing {
        path: MANIFEST_NAME.to_string(),
    })?;
    #[cfg(not(unix))]
    let meta = fs::metadata(path).map_err(|_| Error::Missing {
        path: MANIFEST_NAME.to_string(),
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Error::Io {
            detail: format!("'{MANIFEST_NAME}' is not a regular file"),
        });
    }
    let mut file = File::open(path).map_err(|_| Error::Missing {
        path: MANIFEST_NAME.to_string(),
    })?;
    let meta = file.metadata().map_err(|e| Error::Io {
        detail: e.to_string(),
    })?;
    // The reference disables hard-link rejection on Windows.
    #[cfg(unix)]
    if meta.nlink() > 1 {
        return Err(Error::Io {
            detail: format!("'{MANIFEST_NAME}' has more than one hard link"),
        });
    }
    if meta.len() > MANIFEST_MAX_BYTES as u64 {
        return Err(Error::Invalid {
            detail: format!("'{MANIFEST_NAME}' size exceeds {MANIFEST_MAX_BYTES} bytes"),
        });
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes).map_err(|e| Error::Io {
        detail: e.to_string(),
    })?;
    // The reference rejects an embedded NUL before JSON parsing
    // (`memchr(json, '\0', json_len)` in `musicpack_package_open_dir`).
    if bytes.contains(&0) {
        return Err(Error::Json {
            detail: format!("'{MANIFEST_NAME}' contains a NUL byte"),
        });
    }
    Ok(bytes)
}

/// Opens a directory bundle, parses its manifest, and verifies it.
///
/// Open/parse failures are [`Error`]s (the CLI's "cannot open package");
/// verification findings are returned in the [`Report`](crate::validation::Report).
pub fn verify_directory(root: impl AsRef<Path>) -> Result<crate::validation::Report, Error> {
    let backend = DirectoryBackend::open(root)?;
    let parsed = crate::format::manifest::ParsedManifest::parse(backend.manifest_bytes())?;
    Ok(crate::validation::verify(parsed.manifest(), &backend))
}

/// Reads a referenced member's raw bytes (bounded by `max`), applying the
/// same containment/regular-file/size policy as verification.
///
/// Mirror of `musicpack_package_read_member` for the directory backend.
pub fn read_member(backend: &DirectoryBackend, path: &str, max: usize) -> Result<Vec<u8>, Error> {
    let opened = backend
        .open_asset(path)
        .map_err(|e| map_backend_error(e, path))?;
    if opened.len > max as u64 {
        return Err(Error::Io {
            detail: format!("'{path}' exceeds {max}-byte read limit"),
        });
    }
    let mut reader = opened.reader;
    let mut bytes = Vec::with_capacity(opened.len as usize);
    reader.read_to_end(&mut bytes).map_err(|e| Error::Io {
        detail: e.to_string(),
    })?;
    Ok(bytes)
}

fn map_backend_error(e: BackendError, path: &str) -> Error {
    match e {
        BackendError::UnsafePath => Error::Invalid {
            detail: format!("unsafe path '{path}'"),
        },
        BackendError::Missing => Error::Missing {
            path: path.to_string(),
        },
        BackendError::Io(detail) => Error::Io { detail },
    }
}

// ---------------------------------------------------------------------
// packing a directory bundle into an MPAK container
// ---------------------------------------------------------------------

/// `PackSource` over a verified directory bundle.
struct DirectoryPackSource<'a> {
    backend: &'a DirectoryBackend,
    manifest: Vec<u8>,
    members: Vec<crate::format::mpak::PackMember>,
}

impl crate::format::mpak::PackSource for DirectoryPackSource<'_> {
    fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    fn members(&self) -> &[crate::format::mpak::PackMember] {
        &self.members
    }

    fn member_size(&self, path: &str) -> Result<u64, Error> {
        // `open_asset` applies the regular-file/link-count/containment
        // policy, so the packer cannot read an object verification would
        // have rejected.
        Ok(self
            .backend
            .open_asset(path)
            .map_err(|e| map_backend_error(e, path))?
            .len)
    }

    fn read_member(&self, path: &str) -> Result<Box<dyn Read + '_>, Error> {
        Ok(self
            .backend
            .open_asset(path)
            .map_err(|e| map_backend_error(e, path))?
            .reader)
    }
}

/// Packs a verified directory bundle into a deterministic MPAK v1
/// container — the core of the reference's `musicpack pack`.
///
/// Mirrors `musicpack_mpak_pack_dir`:
///
/// 1. the directory package is verified first (only verified packages are
///    packed, so the manifest hashes describe the stored bytes);
/// 2. `manifest.json` is re-read and its canonical model must equal the
///    verified one (a manifest changed underneath the packer aborts);
/// 3. the exact manifest bytes are embedded (never regenerated);
/// 4. members are written in [`crate::format::mpak::canonical_pack_order`]
///    with their declared SHA-256 verified while copying.
pub fn pack_directory_to_writer(dir: &Path, out: &mut dyn std::io::Write) -> Result<(), Error> {
    let backend = DirectoryBackend::open(dir)?;
    let parsed = crate::format::manifest::ParsedManifest::parse(backend.manifest_bytes())?;

    // Only verified packages are packed.
    let report = crate::validation::verify(parsed.manifest(), &backend);
    if !report.is_ok() {
        return Err(Error::Checksum {
            path: MANIFEST_NAME.to_string(),
            expected: "a verified package".into(),
            actual: format!("{} verification error(s)", report.errors()),
        });
    }

    // The manifest must not have changed between verification and packing.
    let fresh = backend.reread_manifest()?;
    let fresh_parsed = crate::format::manifest::ParsedManifest::parse(&fresh)?;
    if fresh_parsed.manifest().write_canonical()? != parsed.manifest().write_canonical()? {
        return Err(Error::Checksum {
            path: MANIFEST_NAME.to_string(),
            expected: "verified manifest".into(),
            actual: "manifest changed during pack".into(),
        });
    }

    let members: Vec<crate::format::mpak::PackMember> =
        crate::format::mpak::canonical_pack_order(parsed.manifest())
            .into_iter()
            .map(|(path, sha256_hex)| crate::format::mpak::PackMember {
                path: path.to_string(),
                sha256_hex: sha256_hex.to_string(),
            })
            .collect();

    let source = DirectoryPackSource {
        backend: &backend,
        manifest: fresh,
        members,
    };
    crate::format::mpak::write_mpak(&source, out)
}

/// Packs a verified directory bundle into a new `.mpak` file.
pub fn pack_directory(dir: &Path, out_path: &Path) -> Result<(), Error> {
    let mut file = std::fs::File::create(out_path).map_err(|e| Error::Io {
        detail: format!("cannot create '{}': {e}", out_path.display()),
    })?;
    pack_directory_to_writer(dir, &mut file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "musicpack-core-storage-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn minimal_manifest(rel: &str, digest: &str) -> String {
        format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"{rel}","sha256":"{digest}"}}}}]}}]}}"#
        )
    }

    #[test]
    fn opens_and_reads_a_regular_member() {
        let dir = temp_dir("regular");
        fs::create_dir_all(dir.join("audio")).unwrap();
        let digest = crate::format::checksum::sha256_hex(b"one");
        fs::write(
            dir.join("manifest.json"),
            minimal_manifest("audio/01.bin", &digest),
        )
        .unwrap();
        let mut f = File::create(dir.join("audio/01.bin")).unwrap();
        f.write_all(b"one").unwrap();

        let backend = DirectoryBackend::open(&dir).expect("opens");
        assert!(!backend.manifest_bytes().is_empty());
        let opened = backend.open_asset("audio/01.bin").expect("opens member");
        assert_eq!(opened.len, 3);
        let bytes = read_member(&backend, "audio/01.bin", 1024).unwrap();
        assert_eq!(bytes, b"one");
        // Stable identity exists on unix; off unix (Windows) the reference
        // disables inode dedup, so there is none by design.
        #[cfg(unix)]
        assert!(backend.object_id("audio/01.bin").is_some());
        #[cfg(not(unix))]
        assert!(backend.object_id("audio/01.bin").is_none());
        // The unreferenced-file walk is unix-only (the reference skips it
        // on Windows, so the listing is empty there by design).
        #[cfg(unix)]
        assert_eq!(backend.list_files(), vec!["audio/01.bin", "manifest.json"]);
        #[cfg(not(unix))]
        assert!(backend.list_files().is_empty());
        assert_eq!(backend.meta_files(), &["manifest.json"]);
    }

    #[test]
    fn rejects_embedded_nul_in_manifest() {
        let dir = temp_dir("nul");
        let digest = crate::format::checksum::sha256_hex(b"one");
        let mut bytes = minimal_manifest("audio/01.bin", &digest).into_bytes();
        bytes.push(0);
        fs::write(dir.join("manifest.json"), bytes).unwrap();
        let err = DirectoryBackend::open(&dir).err().expect("rejects");
        assert!(matches!(err, Error::Json { .. }), "{err:?}");
    }

    #[test]
    fn rejects_a_non_directory_root() {
        let dir = temp_dir("notdir");
        let file = dir.join("plain.mpak");
        fs::write(&file, b"x").unwrap();
        assert!(DirectoryBackend::open(&file).is_err());
    }
}
