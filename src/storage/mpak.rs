//! MPAK-backed [`PackageBackend`]: container members exposed to the shared
//! verifier.
//!
//! The backend performs no verification of its own beyond exposing members
//! safely: SHA-256, budgets, manifest semantics and waveform checks are the
//! generic verifier's job (`crate::validation`). What it does own is
//! *container* consistency — `INDX`/`TAIL`/version/reserved/resync/duplicate
//! checks — reported through [`PackageBackend::verify_extra`], exactly where
//! the reference calls `musicpack_mpak_verify_extra`.
//!
//! # Memory model
//!
//! A container is **streamed from a [`ByteSource`]**, never buffered whole:
//! the scanner reads a 14-byte header at a time (plus bounded INDX/MANF/
//! TAIL payloads) and member access returns a range reader over the shared
//! source. This mirrors the reference's seekable `FILE*` + `mpak_cio`
//! design and keeps native files, server storage, and a future browser
//! range source viable without whole-file allocations.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use sha2::Digest;

use crate::error::Error;
use crate::format::checksum;
use crate::format::manifest::Manifest;
use crate::format::mpak::{self, ByteSource, MemberReader, MpakReader, ReadError};
use crate::validation::Report;

use super::{BackendError, ObjectId, OpenedAsset, PackageBackend, VerificationSink};

/// An in-memory container source.
///
/// Convenience re-export of the format-layer type (tests, embedded use).
pub use crate::format::mpak::MemorySource;

/// A seekable file container source.
///
/// Uses `seek` + `read_exact` per request (the reference's
/// `seek_absolute`), so it is portable without platform file extensions.
/// The file must not be mutated while the source is in use (the caller's
/// responsibility, matching the reference's documented post-verification
/// immutability requirement).
pub struct FileSource {
    file: Mutex<File>,
    size: u64,
}

impl FileSource {
    /// Opens a file as a container source.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let file = File::open(path.as_ref()).map_err(|e| Error::Io {
            detail: format!("cannot open '{}': {e}", path.as_ref().display()),
        })?;
        let size = file
            .metadata()
            .map_err(|e| Error::Io {
                detail: e.to_string(),
            })?
            .len();
        Ok(Self {
            file: Mutex::new(file),
            size,
        })
    }
}

impl ByteSource for FileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), ReadError> {
        let end = offset
            .checked_add(out.len() as u64)
            .ok_or_else(|| ReadError {
                detail: "read range overflow".into(),
            })?;
        if end > self.size {
            return Err(ReadError {
                detail: "read past end of container".into(),
            });
        }
        let mut file = self.file.lock().map_err(|_| ReadError {
            detail: "container source lock poisoned".into(),
        })?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| ReadError {
            detail: e.to_string(),
        })?;
        file.read_exact(out).map_err(|e| ReadError {
            detail: e.to_string(),
        })
    }
}

/// A scanned MPAK container, exposed as a package backend.
pub struct MpakBackend {
    source: Arc<dyn ByteSource>,
    reader: MpakReader,
    manifest: Vec<u8>,
}

impl MpakBackend {
    /// Scans `source` (normal reader semantics) and prepares the embedded
    /// manifest for parsing.
    ///
    /// Fails when the container is malformed, has no `MANF`, or has more
    /// than one; the manifest bytes are NUL-checked exactly like the
    /// reference (`memchr` before JSON parsing).
    pub fn open(source: Arc<dyn ByteSource>) -> Result<Self, Error> {
        let reader = mpak::scan(source.as_ref(), false)?;
        if reader.manifest_count() == 0 {
            return Err(Error::Missing {
                path: "MANF".into(),
            });
        }
        if reader.manifest_count() > 1 {
            return Err(Error::Invalid {
                detail: "container has more than one MANF block".into(),
            });
        }
        let manifest = reader
            .manifest_bytes()
            .ok_or_else(|| Error::Missing {
                path: "MANF".into(),
            })?
            .to_vec();
        if manifest.contains(&0) {
            return Err(Error::Json {
                detail: "embedded manifest contains a NUL byte".into(),
            });
        }
        Ok(Self {
            source,
            reader,
            manifest,
        })
    }

    /// Opens a file-backed container.
    pub fn open_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let source = FileSource::open(path)?;
        Self::open(Arc::new(source))
    }

    /// The exact embedded manifest bytes.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    /// The scanned container facts.
    pub fn reader(&self) -> &MpakReader {
        &self.reader
    }

    /// Streams a member's bytes (bounded by `max`; mirrors the reference's
    /// `musicpack_package_read_member` limit).
    pub fn read_member(&self, path: &str, max: usize) -> Result<Vec<u8>, Error> {
        let member = self.reader.member(path).ok_or_else(|| Error::Missing {
            path: path.to_string(),
        })?;
        if member.length > max as u64 {
            return Err(Error::Io {
                detail: format!("'{path}' exceeds {max}-byte read limit"),
            });
        }
        let mut reader = MemberReader::new(self.source.clone(), member.offset, member.length);
        let mut bytes = Vec::with_capacity(member.length as usize);
        reader.read_to_end(&mut bytes).map_err(|e| Error::Io {
            detail: e.to_string(),
        })?;
        Ok(bytes)
    }

    /// The container size in bytes.
    pub fn file_size(&self) -> u64 {
        self.reader.file_size()
    }
}

impl PackageBackend for MpakBackend {
    fn open_asset(&self, path: &str) -> Result<OpenedAsset, BackendError> {
        match self.reader.member(path) {
            None => Err(BackendError::Missing),
            Some(member) => Ok(OpenedAsset {
                len: member.length,
                reader: Box::new(MemberReader::new(
                    self.source.clone(),
                    member.offset,
                    member.length,
                )),
            }),
        }
    }

    fn object_id(&self, _path: &str) -> Option<ObjectId> {
        // The reference disables inode dedup for the MPAK backend.
        None
    }

    fn list_files(&self) -> Vec<String> {
        // Scan (DATA) order: deterministic, unlike a directory readdir.
        self.reader
            .members()
            .iter()
            .map(|m| m.path.clone())
            .collect()
    }

    fn meta_files(&self) -> &[&str] {
        // `MANF` is not a member; no name is storage-owned.
        &[]
    }

    fn verify_extra(&self, manifest: &Manifest, sink: &mut dyn VerificationSink) {
        verify_container(self, manifest, sink);
    }
}

/// Opens a container, parses its embedded manifest, and verifies it.
///
/// Open/parse failures are [`Error`]s; verification findings are in the
/// [`Report`]. Companion to `directory::verify_directory`.
pub fn verify_mpak(source: Arc<dyn ByteSource>) -> Result<Report, Error> {
    let backend = MpakBackend::open(source)?;
    let parsed = crate::format::manifest::ParsedManifest::parse(backend.manifest_bytes())?;
    Ok(crate::validation::verify(parsed.manifest(), &backend))
}

/// Convenience: verify a file-backed container.
pub fn verify_mpak_file(path: impl AsRef<Path>) -> Result<Report, Error> {
    verify_mpak(Arc::new(FileSource::open(path)?))
}

/// Port of `musicpack_mpak_verify_extra`.
fn verify_container(backend: &MpakBackend, manifest: &Manifest, sink: &mut dyn VerificationSink) {
    let reader = &backend.reader;

    if reader.minor() > 0 {
        sink.warning(&format!(
            "container: newer minor version {} (downgrade-compatible)",
            reader.minor()
        ));
    }
    if reader.reserved_nonzero() {
        sink.warning("container: nonzero reserved header bytes tolerated");
    }
    if reader.resynced() {
        sink.warning(
            "container: damaged block framing; best-effort resynchronization used (recovery scan)",
        );
    }
    if reader.duplicate_members() > 0 {
        if let Some(example) = reader.duplicate_example() {
            sink.error(&format!("duplicate object path '{example}'"));
        }
    }

    // ---- INDX ----
    if !reader.indx_present() {
        sink.warning("index: missing INDX; sequential scan used");
    } else {
        if reader.indx_extra() {
            sink.warning("index: extra INDX block ignored");
        }
        if reader.indx_invalid() {
            if reader.indx_duplicate() {
                sink.error("index: duplicate INDX path; index discarded");
            }
            sink.warning("index: corrupt INDX discarded; sequential scan used");
        } else {
            // Manifest/index consistency: every referenced asset must have
            // exactly one INDX entry with a matching declared hash.
            for (path, sha_hex) in collect_manifest_assets(manifest) {
                if reader.member(path).is_none() {
                    sink.error(&format!("index: missing entry for '{path}'"));
                    continue;
                }
                if let (Some(indx_sha), Some(declared)) = (
                    reader.indx_sha256(path),
                    checksum::sha256_hex_to_bytes(sha_hex),
                ) {
                    if *indx_sha != declared {
                        sink.error(&format!("index: checksum mismatch '{path}'"));
                    }
                }
            }
        }
    }

    // ---- TAIL ----
    if !reader.tail_present() {
        sink.warning("completeness unproven (no TAIL)");
    } else {
        if reader.tail_extra() {
            sink.warning("tail: extra TAIL block ignored");
        }
        if reader.tail_malformed() {
            sink.warning("tail: malformed TAIL ignored");
        } else {
            if reader.tail_total_size() != reader.file_size() {
                sink.error("tail: total size mismatch");
            }
            if reader.tail_objects() as usize != reader.members().len() {
                sink.error(&format!(
                    "tail: object count mismatch ({} vs {})",
                    reader.tail_objects(),
                    reader.members().len()
                ));
            }
            let expected_indx_offset = if reader.indx_present() {
                reader.indx_offset()
            } else {
                0
            };
            if reader.tail_indx_offset() != expected_indx_offset {
                sink.error("tail: INDX offset mismatch");
            }
            match hash_prefix(&*backend.source, reader.tail_offset()) {
                Err(_) => sink.error("tail: package digest cannot be computed"),
                Ok(digest) => {
                    if digest != *reader.tail_digest() {
                        sink.error("tail: package digest mismatch");
                    }
                }
            }
        }
    }
}

/// Reference `collect_manifest_assets` order: per track the primary audio,
/// then representations, then waveform; then artwork, booklet, lyrics,
/// extras, analysis. (Distinct from the writer's pack order, which groups
/// all audio, then all representations, then all waveforms.)
fn collect_manifest_assets(manifest: &Manifest) -> Vec<(&str, &str)> {
    let mut refs: Vec<(&str, &str)> = Vec::new();
    for disc in &manifest.media {
        for track in &disc.tracks {
            refs.push((track.audio.path.as_str(), track.audio.sha256.as_str()));
            for representation in &track.representations {
                refs.push((representation.path.as_str(), representation.sha256.as_str()));
            }
            if let Some(waveform) = &track.waveform {
                refs.push((waveform.path.as_str(), waveform.sha256.as_str()));
            }
        }
    }
    for artwork in &manifest.artwork {
        refs.push((artwork.asset.path.as_str(), artwork.asset.sha256.as_str()));
    }
    for asset in &manifest.booklet {
        refs.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for asset in &manifest.lyrics {
        refs.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for asset in &manifest.extras {
        refs.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for analysis in &manifest.analysis {
        refs.push((analysis.asset.path.as_str(), analysis.asset.sha256.as_str()));
    }
    refs
}

/// SHA-256 over `source[0..length)` (streamed).
fn hash_prefix(source: &dyn ByteSource, length: u64) -> Result<[u8; 32], Error> {
    let mut hasher = sha2::Sha256::new();
    let mut pos: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    while pos < length {
        let want = std::cmp::min(buf.len() as u64, length - pos) as usize;
        source
            .read_at(pos, &mut buf[..want])
            .map_err(|e| Error::Io { detail: e.detail })?;
        hasher.update(&buf[..want]);
        pos += want as u64;
    }
    Ok(hasher.finalize().into())
}

/// Re-export for callers that want the raw format-level scan.
pub use crate::format::mpak::scan as scan_container;

/// Reads an independent SHA-256 of a member range (used by tests and
/// future server ingestion).
pub fn member_sha256(source: &dyn ByteSource, member: &mpak::Member) -> Result<[u8; 32], Error> {
    hash_prefix_range(source, member.offset, member.length)
}

fn hash_prefix_range(source: &dyn ByteSource, offset: u64, length: u64) -> Result<[u8; 32], Error> {
    let mut hasher = sha2::Sha256::new();
    let mut pos: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    while pos < length {
        let want = std::cmp::min(buf.len() as u64, length - pos) as usize;
        source
            .read_at(offset + pos, &mut buf[..want])
            .map_err(|e| Error::Io { detail: e.detail })?;
        hasher.update(&buf[..want]);
        pos += want as u64;
    }
    Ok(hasher.finalize().into())
}
