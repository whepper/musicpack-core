//! Opening a package from either storage backend.
//!
//! Directory bundles use the core's POSIX adapter; `.mpak` files use the
//! portable container backend. The core does the parsing, scanning and
//! verification — this module only chooses the backend and exposes the
//! common handle.

use std::path::Path;

use musicpack_core::Error;
use musicpack_core::format::manifest::ParsedManifest;
use musicpack_core::storage::PackageBackend;
use musicpack_core::storage::mpak::MpakBackend;
use musicpack_core::validation::Report;

/// An opened package (directory bundle or single-file container).
pub enum Package {
    /// A directory bundle (POSIX adapter; boxed: the adapter is much
    /// larger than the container handle).
    #[cfg(unix)]
    Directory(Box<musicpack_core::storage::directory::DirectoryBackend>),
    /// An MPAK v1 single-file container (boxed: the scanned container
    /// state dwarfs the directory handle).
    Container(Box<MpakBackend>),
}

impl Package {
    /// Opens a path: a directory is a bundle, a regular file a container
    /// (mirroring the reference tool's dispatch).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let meta = std::fs::symlink_metadata(path).map_err(|_| Error::Missing {
            path: path.display().to_string(),
        })?;
        if meta.is_dir() {
            return Self::open_directory(path);
        }
        if meta.is_file() {
            return Ok(Package::Container(Box::new(MpakBackend::open_file(path)?)));
        }
        Err(Error::Invalid {
            detail: format!("'{}' is not a directory or regular file", path.display()),
        })
    }

    #[cfg(unix)]
    fn open_directory(path: &Path) -> Result<Self, Error> {
        Ok(Package::Directory(Box::new(
            musicpack_core::storage::directory::DirectoryBackend::open(path)?,
        )))
    }

    #[cfg(not(unix))]
    fn open_directory(path: &Path) -> Result<Self, Error> {
        Err(Error::Invalid {
            detail: format!(
                "directory packages are not supported on this platform yet ('{}'); \
                 use an .mpak container",
                path.display()
            ),
        })
    }

    /// The storage backend (for the shared verifier).
    pub fn backend(&self) -> &dyn PackageBackend {
        match self {
            #[cfg(unix)]
            Package::Directory(backend) => backend.as_ref(),
            Package::Container(backend) => backend.as_ref(),
        }
    }

    /// The exact manifest bytes.
    pub fn manifest_bytes(&self) -> &[u8] {
        match self {
            #[cfg(unix)]
            Package::Directory(backend) => backend.manifest_bytes(),
            Package::Container(backend) => backend.manifest_bytes(),
        }
    }

    /// Parses the manifest with the core's strict parser.
    pub fn parsed(&self) -> Result<ParsedManifest, Error> {
        ParsedManifest::parse(self.manifest_bytes())
    }

    /// Runs the core verification (parse + shared verifier).
    pub fn verify(&self) -> Result<Report, Error> {
        let parsed = self.parsed()?;
        Ok(musicpack_core::validation::verify(
            parsed.manifest(),
            self.backend(),
        ))
    }

    /// A short storage-kind label for diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            #[cfg(unix)]
            Package::Directory(_) => "directory",
            Package::Container(_) => "container",
        }
    }
}
