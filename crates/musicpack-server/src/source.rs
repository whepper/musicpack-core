//! Server library sources: a `.mpack` directory bundle or a `.mpak`
//! container.
//!
//! The server's library is a tree of *sources*, and every stage downstream
//! (identity, ingestion, the SQLite projection, the media layer) needs the same
//! three things from one of them: the manifest, whether a referenced object
//! exists, and that object's size. For a directory bundle those come from the
//! filesystem; for a container they come from the container's member table.
//! [`PackageSource`] is the single place that difference lives, so nothing else
//! in the server branches on the source kind.
//!
//! ```text
//! PackageSource
//!   ├── Directory { root }   → manifest.json + regular files
//!   └── Container { .. }    → MANF + member table (via musicpack-core)
//! ```
//!
//! The container arm never parses the container itself: it goes through
//! [`musicpack_core::storage::mpak::MpakBackend`], the authoritative
//! implementation, opened over the core `FileSource` (a native seek + read
//! handle — not the client transport's range adapter).
//!
//! [`playback_source`] is the other half of the contract: the canonical
//! `mpak:<container>#<member>` key an indexed container member is addressed by,
//! built by core's single formatter.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use musicpack_core::format::manifest::{Manifest, ParsedManifest};
use musicpack_core::player::source_url::{CONTAINER_PREFIX, format_container_source};
use musicpack_core::player::types::PlaybackSource;
use musicpack_core::storage::mpak::{FileSource, MpakBackend};

/// The manifest of a source, plus the manifest byte identity every source kind
/// agrees on.
pub struct SourceManifest {
    /// The parsed manifest.
    pub manifest: Manifest,
    /// SHA-256 over the **manifest bytes** as the source stores them
    /// (`manifest.json` for a bundle, the `MANF` payload for a container).
    pub manifest_sha256: String,
}

/// Why a source could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The manifest could not be read at all (missing/unreadable file, or a
    /// container that does not scan).
    UnreadableManifest(String),
    /// The manifest bytes were read but do not parse or validate.
    InvalidManifest(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::UnreadableManifest(detail) => {
                write!(f, "manifest unreadable: {detail}")
            }
            SourceError::InvalidManifest(detail) => {
                write!(f, "manifest invalid: {detail}")
            }
        }
    }
}

impl std::error::Error for SourceError {}

/// A library source the server can index.
pub enum PackageSource {
    /// A `.mpack` directory bundle.
    Directory {
        /// The package directory.
        root: PathBuf,
    },
    /// A single-file `.mpak` container.
    Container {
        /// The container file.
        path: PathBuf,
        /// The scanned container. `None` only for a source that failed to open,
        /// which never carries a manifest either.
        backend: Option<Arc<MpakBackend>>,
        /// The container's total size in bytes (0 when it could not be stated).
        size: u64,
    },
}

impl std::fmt::Debug for PackageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `MpakBackend` holds an open file handle; keep the debug output about
        // the identity, not the handle.
        match self {
            PackageSource::Directory { root } => {
                f.debug_struct("Directory").field("root", root).finish()
            }
            PackageSource::Container { path, size, .. } => f
                .debug_struct("Container")
                .field("path", path)
                .field("size", size)
                .finish(),
        }
    }
}

impl PackageSource {
    /// Opens a directory-bundle source.
    pub fn directory(root: impl Into<PathBuf>) -> Self {
        PackageSource::Directory { root: root.into() }
    }

    /// Opens a container source, reading its `MANF` through
    /// [`MpakBackend::open_file`] (core's native file-backed container
    /// adapter).
    ///
    /// A container that does not scan is still returned — with no backend — so
    /// the caller can report it as an invalid source with the real reason
    /// instead of a generic failure.
    pub fn container(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let backend = MpakBackend::open_file(&path).ok().map(Arc::new);
        PackageSource::Container {
            path,
            backend,
            size,
        }
    }

    /// Opens whatever an indexed package's recorded locator names.
    ///
    /// The kind is decided by [`crate::discover::classify_source_path`] — the
    /// same single classification point the walk and the verify pass use — so
    /// the byte layer can never disagree with the index about what a package
    /// is. A locator whose shape matches neither kind falls back to a directory
    /// bundle, which is every package the C reference knew about.
    pub fn for_locator(locator: &Path) -> Self {
        match crate::discover::classify_source_path(locator) {
            crate::discover::SourceKind::Container => PackageSource::container(locator),
            crate::discover::SourceKind::Directory => PackageSource::directory(locator),
        }
    }

    /// The byte extent of `rel` **inside its backing file**, as
    /// `(offset, length)`.
    ///
    /// This is the whole of what byte serving needs from a container: core's
    /// member table is the only authority for both numbers, and nothing about
    /// them comes from a URL. `None` for a directory bundle (a file has no
    /// extent within itself) and for a member that is not in the table.
    pub fn member_extent(&self, rel: &str) -> Option<(u64, u64)> {
        match self {
            PackageSource::Directory { .. } => None,
            PackageSource::Container { backend, .. } => backend
                .as_ref()?
                .reader()
                .member(rel)
                .map(|m| (m.offset, m.length)),
        }
    }

    /// `true` for a single-file container source.
    pub fn is_container(&self) -> bool {
        matches!(self, PackageSource::Container { .. })
    }

    /// The source's locator: the package directory, or the container file.
    pub fn locator(&self) -> &Path {
        match self {
            PackageSource::Directory { root } => root,
            PackageSource::Container { path, .. } => path,
        }
    }

    /// The source's manifest and its manifest-byte hash.
    pub fn read_manifest(&self) -> Result<SourceManifest, SourceError> {
        match self {
            PackageSource::Directory { root } => {
                let backend = musicpack_core::storage::directory::DirectoryBackend::open(root)
                    .map_err(|e| SourceError::UnreadableManifest(e.to_string()))?;
                let bytes = backend.manifest_bytes();
                let sha = musicpack_core::identity::manifest_hash(bytes);
                let manifest = ParsedManifest::parse(bytes)
                    .map_err(|e| SourceError::InvalidManifest(e.to_string()))?
                    .manifest()
                    .clone();
                Ok(SourceManifest {
                    manifest,
                    manifest_sha256: sha,
                })
            }
            PackageSource::Container { backend, .. } => {
                let backend = backend.as_ref().ok_or_else(|| {
                    SourceError::UnreadableManifest("container unreadable".into())
                })?;
                // The MANF payload is the manifest; the container rules
                // (exactly one MANF, NUL check) were already applied by
                // `MpakBackend::open`.
                let bytes = backend.manifest_bytes();
                let sha = musicpack_core::identity::manifest_hash(bytes);
                let manifest = ParsedManifest::parse(bytes)
                    .map_err(|e| SourceError::InvalidManifest(e.to_string()))?
                    .manifest()
                    .clone();
                Ok(SourceManifest {
                    manifest,
                    manifest_sha256: sha,
                })
            }
        }
    }

    /// The manifest-referenced objects of this source, in the reference's
    /// exact object lists (primary audio, artwork, booklet, lyrics, extras,
    /// analysis — notably not representations or waveforms).
    pub fn manifest_objects<'m>(&self, manifest: &'m Manifest) -> Vec<&'m str> {
        let mut objects = Vec::new();
        for disc in &manifest.media {
            for track in &disc.tracks {
                objects.push(track.audio.path.as_str());
            }
        }
        for artwork in &manifest.artwork {
            objects.push(artwork.asset.path.as_str());
        }
        for asset in &manifest.booklet {
            objects.push(asset.path.as_str());
        }
        for asset in &manifest.lyrics {
            objects.push(asset.path.as_str());
        }
        for asset in &manifest.extras {
            objects.push(asset.path.as_str());
        }
        for analysis in &manifest.analysis {
            objects.push(analysis.asset.path.as_str());
        }
        objects
    }

    /// The SHA-256 of the manifest bytes as this source stores them, or `None`
    /// when the manifest could not be read at all.
    ///
    /// The reference records the real hash for a manifest that was read but did
    /// not parse, and `""` for one that could not be read; this is how a
    /// caller tells those two cases apart.
    pub fn raw_manifest_hash(&self) -> Option<String> {
        let bytes = self.raw_manifest_bytes()?;
        Some(musicpack_core::identity::manifest_hash(&bytes))
    }

    /// The manifest bytes as this source stores them (`manifest.json`, or the
    /// container's `MANF` payload), or `None` when they cannot be read.
    pub fn raw_manifest_bytes(&self) -> Option<Vec<u8>> {
        match self {
            PackageSource::Directory { root } => {
                let backend =
                    musicpack_core::storage::directory::DirectoryBackend::open(root).ok()?;
                Some(backend.manifest_bytes().to_vec())
            }
            PackageSource::Container { backend, .. } => {
                Some(backend.as_ref()?.manifest_bytes().to_vec())
            }
        }
    }

    /// Whether the object at the package-relative `rel` exists and is
    /// servable-shaped.
    ///
    /// For a directory bundle this is the reference's `is_regular_path` (not a
    /// symlink, regular, link count 1). For a container it is "the member is in
    /// the container's index" — core's member table is the only authority.
    pub fn object_exists(&self, rel: &str) -> bool {
        match self {
            PackageSource::Directory { root } => crate::probe::is_regular_file(&root.join(rel)),
            PackageSource::Container { backend, .. } => backend
                .as_ref()
                .and_then(|b| b.reader().member(rel))
                .is_some(),
        }
    }

    /// The object's size in bytes, or 0 when it cannot be stated (the
    /// reference's `file_size_of` convention: stat, follow links, 0 on
    /// failure).
    pub fn object_size(&self, rel: &str) -> u64 {
        match self {
            PackageSource::Directory { root } => crate::probe::file_size(&root.join(rel)),
            PackageSource::Container { backend, .. } => backend
                .as_ref()
                .and_then(|b| b.reader().member(rel))
                .map(|m| m.length)
                .unwrap_or(0),
        }
    }

    /// The byte length of the path this source resolves `rel` through: the
    /// joined filesystem path for a directory bundle, the member path for a
    /// container (which has no joined path).
    ///
    /// The reference bounds resolved asset paths against its fixed path buffer;
    /// this is the same bound applied to whichever path the source kind
    /// actually has.
    pub fn resolved_path_len(&self, rel: &str) -> usize {
        match self {
            PackageSource::Directory { root } => {
                root.join(rel).as_os_str().as_encoded_bytes().len()
            }
            PackageSource::Container { .. } => rel.len(),
        }
    }

    /// Opens the object's bytes.
    ///
    /// This is the read path the server uses to verify a member really is the
    /// bytes its manifest claims. A container member is bounded to its extent
    /// by core's `MemberReader`, so a caller cannot read past it.
    pub fn open_object(&self, rel: &str) -> Option<Box<dyn Read>> {
        match self {
            PackageSource::Directory { root } => {
                let abs = root.join(rel);
                if !crate::probe::is_regular_file(&abs) {
                    return None;
                }
                Some(Box::new(std::fs::File::open(abs).ok()?))
            }
            PackageSource::Container { backend, .. } => {
                use musicpack_core::storage::PackageBackend;
                let backend = backend.as_ref()?;
                backend.open_asset(rel).ok().map(|opened| opened.reader)
            }
        }
    }

    /// The codec probe facts for one audio object, or `None` when the object
    /// does not resolve or its codec cannot be read.
    ///
    /// The probe reads the object's leading bytes through [`Self::open_object`],
    /// so a container member is examined by exactly the code that examines a
    /// directory file.
    pub fn probe_object(&self, rel: &str) -> Option<ProbeFacts> {
        if !self.object_exists(rel) {
            return None;
        }
        let size = self.object_size(rel);
        let reader = self.open_object(rel)?;
        crate::probe::probe_reader(rel, reader).map(|facts| ProbeFacts { facts, size })
    }

    /// Every member of a container, as `(path, size)` pairs in container
    /// (DATA) order. Empty for a directory bundle, which has no member table.
    pub fn members(&self) -> Vec<(String, u64)> {
        match self {
            PackageSource::Directory { .. } => Vec::new(),
            PackageSource::Container { backend, .. } => backend
                .as_ref()
                .map(|b| {
                    b.reader()
                        .members()
                        .iter()
                        .map(|m| (m.path.clone(), m.length))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
    /// The codec probe for one audio object, in the store's
    /// [`TrackProbe`](crate::store::TrackProbe) shape.
    ///
    /// A directory bundle takes the reference's `probe_track` unchanged, so its
    /// indexed rows stay byte-identical to before. A container member is probed
    /// from the member's own bytes through the same
    /// [`crate::probe::probe_reader`], so the two kinds cannot report different
    /// codec facts for the same content. A member has no filesystem path, so
    /// `abs_path` is `None` and the size comes from the member table.
    pub fn probe_track(&self, rel: &str) -> crate::store::TrackProbe {
        use crate::store::TrackProbe;
        match self {
            PackageSource::Directory { root } => crate::probe::probe_track(root, rel),
            PackageSource::Container { .. } => {
                let size = self.object_size(rel);
                let unresolved = || TrackProbe {
                    abs_path: None,
                    size: 0,
                    codec: String::new(),
                    stream_version: 0,
                    sample_rate: 0,
                    channels: 0,
                };
                if !self.object_exists(rel) {
                    return unresolved();
                }
                let Some(reader) = self.open_object(rel) else {
                    return unresolved();
                };
                match crate::probe::probe_reader(rel, reader) {
                    Some(facts) => TrackProbe {
                        abs_path: None,
                        size,
                        codec: facts.codec,
                        stream_version: facts.stream_version,
                        sample_rate: facts.sample_rate,
                        channels: facts.channels,
                    },
                    // The failed-probe path: the object resolved (so it keeps
                    // its size) but its codec could not be read, and the sync
                    // falls back to the extension-derived codec.
                    None => TrackProbe {
                        abs_path: None,
                        size,
                        codec: String::new(),
                        stream_version: 0,
                        sample_rate: 0,
                        channels: 0,
                    },
                }
            }
        }
    }
}

/// The codec facts a probe recovered, with the object's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFacts {
    /// Codec string, stream version, sample rate and channels.
    pub facts: crate::probe::CodecFacts,
    /// The object's byte count.
    pub size: u64,
}

/// The canonical playback source for a member of the container at `container`.
///
/// This is the **only** way the server states a container member's source: the
/// key is built by core's single formatter, so an indexed track resolves back
/// to exactly the container and member it was indexed from. A directory-bundle
/// source has no container, so it has no such key — the media layer addresses
/// those by package path plus relative path, as before.
pub fn playback_source(container: &Path, member: &str) -> PlaybackSource {
    PlaybackSource {
        kind: musicpack_core::player::types::SourceKind::Other(
            CONTAINER_PREFIX.trim_end_matches(':').to_string(),
        ),
        url: format_container_source(&container.to_string_lossy(), member),
        byte_size: None,
    }
}

/// The canonical playback source of an **indexed** object.
///
/// This is the round-trip guarantee the index rests on: the stored locator
/// (`MediaRef::package_path`) and the stored member
/// (`MediaRef::relative_path`) are exactly the container and member the object
/// was indexed from, and they are re-assembled here with core's single
/// formatter — never by hand, and never re-derived from a file extension.
///
/// Returns `None` for an object that is not inside a container: a
/// directory-bundle package has no container locator, and the byte layer keeps
/// addressing those by package path plus relative path.
pub fn indexed_source(media: &crate::store::MediaRef) -> Option<PlaybackSource> {
    if !Path::new(&media.package_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mpak"))
    {
        return None;
    }
    Some(playback_source(
        Path::new(&media.package_path),
        &media.relative_path,
    ))
}

/// The container file and member path a canonical playback source names.
///
/// The inverse of [`playback_source`]: the server's round-trip guarantee for
/// every indexed container track. `Ok(None)` for a source that is not a
/// container key, `Err(..)` for a malformed one.
pub fn resolve_playback_source(
    source: &PlaybackSource,
) -> Result<Option<(String, String)>, String> {
    musicpack_core::player::source_url::parse_container_source(&source.url)
        .map(|parsed| parsed.map(|p| (p.container, p.member)))
        .map_err(|e| e.to_string())
}

/// Counts the manifest-referenced objects that are missing from `source`,
/// using the same object lists as [`PackageSource::manifest_objects`].
pub fn count_missing_objects(source: &PackageSource, manifest: &Manifest) -> usize {
    source
        .manifest_objects(manifest)
        .into_iter()
        .filter(|rel| !source.object_exists(rel))
        .count()
}

/// A map of member path to size for a container, for callers that need a
/// lookup table rather than repeated scans.
pub fn member_sizes(source: &PackageSource) -> HashMap<String, u64> {
    source.members().into_iter().collect()
}

/// Opens a container's `MANF` without keeping the container open.
///
/// Convenience for callers that only need the manifest (identity derivation,
/// for instance); `PackageSource::container` is the general entry point.
pub fn read_container_manifest(path: &Path) -> Result<SourceManifest, SourceError> {
    PackageSource::container(path).read_manifest()
}

/// The file-backed container source core exposes, for hosts that need to build
/// their own `ByteSource` over a container.
pub fn container_file_source(path: &Path) -> Result<FileSource, musicpack_core::Error> {
    FileSource::open(path)
}
