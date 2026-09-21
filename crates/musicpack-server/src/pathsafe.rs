//! Filesystem path safety, shared by every module that turns a stored or
//! requested relative path into an opened file (the C
//! `musicpack_path_resolve` + open discipline):
//!
//! ```text
//! validate(rel) → canonicalize(root) → contain existing ancestors →
//! final component: no symlink, regular file → File
//! ```
//!
//! Security properties (identical to the reference):
//!
//! - the relative path is re-validated against the canonical path rules
//!   even when it came from the database or the URL;
//! - the root is canonicalized and every *existing* ancestor of the joined
//!   path must stay beneath it (the C `check_existing_ancestors`);
//! - the final component must not be a symlink (the C `O_NOFOLLOW`; via
//!   `symlink_metadata` here, std-only, no `libc`, with the same inherent
//!   check-then-open TOCTOU window the reference documents);
//! - physical paths never appear in errors surfaced to clients.
//!
//! Callers own the *policy* differences on top: media serving additionally
//! requires link count 1 and inline-safety, static hosting does not (both
//! exactly like the C).

use std::path::PathBuf;

/// Resolves `relative` beneath `root` with full containment checking.
///
/// Mirrors the C `musicpack_path_resolve`: strict path validation first,
/// then `realpath(root)`, then every existing ancestor of the joined path
/// is canonicalized and required to stay under the root. Returns `None`
/// for any validation/containment failure (the C `MUSICPACK_ERR_PATH`).
pub fn resolve_contained(root: &str, relative: &str) -> Option<PathBuf> {
    if musicpack_core::format::path::validate(relative).is_err() {
        return None;
    }
    let root_real = std::fs::canonicalize(root).ok()?;
    let mut prefix = PathBuf::new();
    for segment in relative.split('/') {
        prefix.push(segment);
        let candidate = root_real.join(&prefix);
        // Only existing ancestors constrain (missing trailing components
        // would be created by no one here — the file must simply exist).
        if candidate.symlink_metadata().is_ok() {
            let resolved = std::fs::canonicalize(&candidate).ok()?;
            if !resolved.starts_with(&root_real) {
                return None;
            }
        }
    }
    Some(root_real.join(relative))
}

/// Why [`open_regular_file`] refused a path, in the order the checks run.
#[derive(Debug)]
pub enum OpenRegularError {
    /// Absent, or the final component is a symlink (the C maps both to
    /// its "missing" message).
    Missing,
    /// Exists but is not a regular file, or violates the single-link
    /// policy (the C "not a regular file" message).
    NotRegularFile,
    /// The file opened but could not be stat'd afterwards.
    Io(std::io::Error),
}

/// Opens `abs` with the final-component discipline shared by all serving
/// paths: the entry must not be a symlink and must be a regular file.
/// Returns the opened handle plus its `fstat` size.
///
/// `require_single_link` is the media-serving policy (the C requires
/// `nlink == 1` for library objects but not for static files).
pub fn open_regular_file(
    abs: &std::path::Path,
    require_single_link: bool,
) -> Result<(std::fs::File, u64), OpenRegularError> {
    let link_meta = std::fs::symlink_metadata(abs).map_err(|_| OpenRegularError::Missing)?;
    if link_meta.file_type().is_symlink() {
        return Err(OpenRegularError::Missing);
    }
    if !link_meta.is_file() {
        return Err(OpenRegularError::NotRegularFile);
    }
    #[cfg(unix)]
    if require_single_link {
        use std::os::unix::fs::MetadataExt;
        if link_meta.nlink() > 1 {
            return Err(OpenRegularError::NotRegularFile);
        }
    }
    let file = std::fs::File::open(abs).map_err(|_| OpenRegularError::Missing)?;
    let meta = file.metadata().map_err(OpenRegularError::Io)?;
    if !meta.is_file() {
        return Err(OpenRegularError::NotRegularFile);
    }
    Ok((file, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Unique temp dir per call — tests run in parallel in one process.
    fn unique_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mp-{tag}-{}-{}", std::process::id(), n))
    }

    fn write(path: &std::path::Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn contained_resolution_walks_subdirectories() {
        let root = unique_dir("pathsafe-a");
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("assets/app.css"), b"body{}");
        let root_str = root.to_str().unwrap();
        let resolved = resolve_contained(root_str, "assets/app.css").unwrap();
        assert!(resolved.is_file());
        assert_eq!(std::fs::read(&resolved).unwrap(), b"body{}".to_vec());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn traversal_and_invalid_paths_are_rejected() {
        let root = unique_dir("pathsafe-b");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root_str = root.to_str().unwrap();
        for rel in [
            "../outside.txt",
            "a/../../outside.txt",
            "/absolute.txt",
            "a//b.txt",
            "a/./b.txt",
            "",
            "a\\b.txt",
            "a:b.txt",
        ] {
            assert!(resolve_contained(root_str, rel).is_none(), "{rel:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_through_existing_ancestor_is_contained() {
        let root = unique_dir("pathsafe-c");
        let outside = unique_dir("pathsafe-c-out");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        write(&outside.join("secret.txt"), b"secret");
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let root_str = root.to_str().unwrap();
        assert!(resolve_contained(root_str, "link/secret.txt").is_none());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_and_hardlink_policy() {
        let root = unique_dir("pathsafe-d");
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("real.txt"), b"hi");
        write(&root.join("twin.txt"), b"hi");
        std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();
        // `hard.txt` shares an inode with `twin.txt` (nlink 2); `real.txt`
        // keeps nlink 1.
        std::fs::hard_link(root.join("twin.txt"), root.join("hard.txt")).unwrap();
        let root_str = root.to_str().unwrap();
        // A symlinked final component is always rejected.
        let linked = resolve_contained(root_str, "link.txt").unwrap();
        assert!(open_regular_file(&linked, false).is_err());
        // Single-link regular file: fine with and without the requirement.
        let real = resolve_contained(root_str, "real.txt").unwrap();
        assert!(open_regular_file(&real, false).is_ok());
        assert!(open_regular_file(&real, true).is_ok());
        // Hard link: only the media-serving policy rejects it.
        let hard = resolve_contained(root_str, "hard.txt").unwrap();
        assert!(open_regular_file(&hard, false).is_ok());
        assert!(open_regular_file(&hard, true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
