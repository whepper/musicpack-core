//! Byte-source seam for the decoder adapter.
//!
//! The engine never touches a filesystem, network, or browser storage. Hosts
//! provide a [`SourceBackend`] that resolves a [`PlaybackSource`] to a
//! streaming reader; [`MemorySourceBackend`] and [`PackageSourceBackend`]
//! cover deterministic tests and package members using only existing core
//! abstractions.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::rc::Rc;

use musicpack_core::player::types::PlaybackSource;
use musicpack_core::storage::PackageBackend;

/// Why a source could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

/// Resolves a playback source descriptor to a byte stream.
pub trait SourceBackend {
    /// Opens `source` for streaming reads.
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError>;
}

/// An in-memory backend keyed by the source URL (tests, embedded fixtures).
#[derive(Default)]
pub struct MemorySourceBackend {
    entries: HashMap<String, Vec<u8>>,
}

impl MemorySourceBackend {
    /// Creates an empty backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `url` bytes.
    pub fn insert(&mut self, url: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        self.entries.insert(url.into(), bytes.into());
    }
}

impl SourceBackend for MemorySourceBackend {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        match self.entries.get(&source.url) {
            Some(bytes) => Ok(Box::new(Cursor::new(bytes.clone()))),
            None => Err(SourceError(format!("no such source: {}", source.url))),
        }
    }
}

/// A backend over an existing [`PackageBackend`] (e.g. an MPAK container).
///
/// The source URL is a package-relative member path. The backend's own
/// containment/existence rules apply; two simultaneous opens (current +
/// standby) are independent, as the reference requires.
pub struct PackageSourceBackend {
    backend: Rc<dyn PackageBackend>,
}

impl PackageSourceBackend {
    /// Wraps a package backend.
    pub fn new(backend: Rc<dyn PackageBackend>) -> Self {
        Self { backend }
    }
}

impl SourceBackend for PackageSourceBackend {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        self.backend
            .open_asset(&source.url)
            .map(|opened| opened.reader)
            .map_err(|e| SourceError(format!("cannot open '{}': {e:?}", source.url)))
    }
}

impl SourceBackend for std::rc::Rc<dyn SourceBackend> {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        (**self).open_source(source)
    }
}

impl SourceBackend for std::sync::Arc<dyn SourceBackend> {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        (**self).open_source(source)
    }
}

impl SourceBackend for Box<dyn SourceBackend> {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        (**self).open_source(source)
    }
}
