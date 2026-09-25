//! Container (`.mpak`) members as engine byte sources.
//!
//! The engine never touches a filesystem, network or browser storage: a host
//! hands it a [`SourceBackend`]. This module is the container-shaped member of
//! that seam — the same thing [`PackageSourceBackend`](crate::source::PackageSourceBackend)
//! does for a directory bundle, except the package is a single MPAK v1 file
//! reached through **range reads** rather than a directory tree.
//!
//! # The `mpak:` source form
//!
//! A container member is addressed by one URL-shaped key:
//!
//! ```text
//! mpak:<container-url>#<member-path>
//! ```
//!
//! The form itself is defined once, in
//! `musicpack_core::player::source_url` (re-exported here), because the server's
//! library index produces the same canonical key for an indexed container
//! member; this module adds only the *engine-side* half — reading a member
//! through a range transport.
//!
//! Two rules keep the host free of container knowledge:
//!
//! - **Byte reads are container-absolute.** [`RangeByteSource`] asks the host
//!   for bytes of the *container* at absolute offsets; the member offset/length
//!   arithmetic happens inside core's `MpakReader`. A host therefore implements
//!   one flat range reader and nothing else — a browser HTTP networker and an
//!   offline OPFS handle satisfy the same contract.
//! - **The host is told the transport URL, not the member key.**
//!   [`transport_url`] resolves the key to the URL/key that actually serves
//!   bytes, which is what a host needs in order to open its transport. The
//!   member part is never handed to a host.
//!
//! # Sizing
//!
//! Scanning a container needs its total size up front (the `TAIL` block lives at
//! the end of the file), so a container source must carry the **transport**
//! size in [`PlaybackSource::byte_size`] — the size of the container, not of the
//! member. Plain sources ignore it, exactly as before.
//!
//! # Cache behaviour
//!
//! A scanned container is cached per (container URL, transport size) for the
//! lifetime of the backend, so opening a second member of the same container
//! costs no re-scan. Only successful scans are cached: a failed scan (an
//! unreachable or malformed container) is retried on the next open rather than
//! cached as a permanent failure. The cache is **not** invalidated when a
//! container's bytes change under a stable URL — a host that can rewrite a
//! container must build a new backend (the web worker already builds one engine,
//! and with it one backend, per open).
//!
//! [`PlaybackSource::byte_size`]: musicpack_core::player::types::PlaybackSource::byte_size

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;

use musicpack_core::format::mpak::{ByteSource, ReadError};
use musicpack_core::player::types::PlaybackSource;
use musicpack_core::storage::PackageBackend;
use musicpack_core::storage::mpak::MpakBackend;

use crate::source::{PackageSourceBackend, SourceBackend, SourceError};

// The `mpak:` key form itself is a *core* concern: the server's library index
// produces the same canonical key for an indexed container member, so both
// sides must agree on one definition. Re-exported here so this module's public
// API — and every existing call site — is unchanged.
pub use musicpack_core::player::source_url::{
    CONTAINER_PREFIX, CONTAINER_SEPARATOR, ContainerSourceUrl, SourceUrlError,
    format_container_source, parse_container_source, transport_url,
};

/// The host's byte transport: a **synchronous** range reader over one URL or
/// store key. Returns the bytes available at `offset` (`Ok(empty)` at EOF) or a
/// human-readable failure. The browser host implements it with a blocking
/// mailbox; a native host with a file or an in-memory buffer.
pub type RangeFetcher = Rc<dyn Fn(&str, u64, usize) -> Result<Vec<u8>, String>>;

/// A [`ByteSource`] over one transport URL, for core's container parser.
///
/// Core's `ByteSource::read_at` is an **exact** read, while a range transport
/// may legitimately return less than asked for (a block-aligned host caps its
/// own reads). Short reads are therefore resumed rather than treated as
/// failures; only a transport that makes no progress is an error.
pub struct RangeByteSource {
    fetch: RangeFetcher,
    url: String,
    size: u64,
}

impl RangeByteSource {
    /// Wraps `fetch` as the byte source of the container at `url`, `size` bytes
    /// long.
    pub fn new(fetch: RangeFetcher, url: impl Into<String>, size: u64) -> Self {
        Self {
            fetch,
            url: url.into(),
            size,
        }
    }
}

impl ByteSource for RangeByteSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), ReadError> {
        if out.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(out.len() as u64)
            .ok_or_else(|| ReadError {
                detail: "read range overflows".into(),
            })?;
        if end > self.size {
            return Err(ReadError {
                detail: format!(
                    "read {offset}..{end} is outside the {}-byte source",
                    self.size
                ),
            });
        }
        let mut filled = 0usize;
        while filled < out.len() {
            let want = out.len() - filled;
            let bytes =
                (self.fetch)(&self.url, offset + filled as u64, want).map_err(|e| ReadError {
                    detail: format!("range read at {} failed: {e}", offset + filled as u64),
                })?;
            if bytes.is_empty() {
                return Err(ReadError {
                    detail: format!(
                        "range read at {} returned no bytes with {} still to read",
                        offset + filled as u64,
                        out.len() - filled
                    ),
                });
            }
            let n = bytes.len().min(want);
            out[filled..filled + n].copy_from_slice(&bytes[..n]);
            filled += n;
        }
        Ok(())
    }
}

/// A plain range source: `url` read from offset 0 through the host's transport.
///
/// Requests are capped at 64 KiB, the transport window every current host uses
/// (the browser mailbox block, the OPFS read chunk, the engine's own read
/// window). The cap bounds each host call; a short reply simply advances.
struct RangeRead {
    fetch: RangeFetcher,
    url: String,
    pos: u64,
}

/// The per-request read window (64 KiB).
const RANGE_WINDOW: usize = 64 * 1024;

impl Read for RangeRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min(RANGE_WINDOW);
        let bytes = (self.fetch)(&self.url, self.pos, want).map_err(std::io::Error::other)?;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        self.pos += n as u64;
        Ok(n)
    }
}

/// The host byte source for both plain ranges and `mpak:` container members.
///
/// One backend serves both shapes: the source key decides. Plain keys stream
/// straight from the transport; container keys are scanned once (see
/// [`RangeByteSource`]) and the member is then read through core's
/// [`MpakReader`], so a member is byte-for-byte the same stream the equivalent
/// directory-bundle member would produce.
pub struct RangeSourceBackend {
    fetch: RangeFetcher,
    containers: RefCell<HashMap<String, Rc<dyn PackageBackend>>>,
}

impl RangeSourceBackend {
    /// Creates a backend over `fetch`.
    pub fn new(fetch: RangeFetcher) -> Self {
        Self {
            fetch,
            containers: RefCell::new(HashMap::new()),
        }
    }

    /// The number of containers currently held in the scan cache.
    pub fn cached_containers(&self) -> usize {
        self.containers.borrow().len()
    }

    /// Returns the scanned container for `parsed`, scanning it on first use.
    ///
    /// The transport size is part of the cache key: the same container URL with
    /// a different size is a different container.
    fn container(
        &self,
        parsed: &ContainerSourceUrl,
        size: u64,
    ) -> Result<Rc<dyn PackageBackend>, SourceError> {
        let key = format!("{}\u{0}{size}", parsed.container);
        if let Some(cached) = self.containers.borrow().get(&key) {
            return Ok(cached.clone());
        }
        let source = RangeByteSource::new(self.fetch.clone(), parsed.container.clone(), size);
        // `MpakBackend::open` takes `Arc<dyn ByteSource>` (core's storage seam is
        // shared with the native file backend, which is `Send`). The engine is
        // single-threaded by design — every other handle here is an `Rc`, and the
        // decoder state machine is not `Send` — so the `Arc` is a signature
        // requirement, never a concurrency claim.
        #[allow(clippy::arc_with_non_send_sync)]
        let backend = MpakBackend::open(Arc::new(source)).map_err(|e| {
            SourceError(format!("cannot scan container '{}': {e}", parsed.container))
        })?;
        let backend: Rc<dyn PackageBackend> = Rc::new(backend);
        self.containers.borrow_mut().insert(key, backend.clone());
        Ok(backend)
    }
}

impl SourceBackend for RangeSourceBackend {
    fn open_source(&self, source: &PlaybackSource) -> Result<Box<dyn Read>, SourceError> {
        let parsed = parse_container_source(&source.url).map_err(|e| SourceError(e.to_string()))?;
        let Some(parsed) = parsed else {
            return Ok(Box::new(RangeRead {
                fetch: self.fetch.clone(),
                url: source.url.clone(),
                pos: 0,
            }));
        };
        // Scanning needs the container's own size; a member length would place
        // the TAIL lookup in the wrong place.
        let size = source.byte_size.ok_or_else(|| {
            SourceError(format!(
                "container member '{}' needs byte_size set to the size of container '{}'",
                parsed.member, parsed.container
            ))
        })?;
        let backend = self.container(&parsed, size)?;
        // The member open itself is the existing package-bridge behaviour: the
        // member path is the source URL, and the backend applies the
        // container's own member rules.
        PackageSourceBackend::new(backend).open_source(&PlaybackSource {
            kind: source.kind.clone(),
            url: parsed.member,
            byte_size: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The key form is core's; these assertions exist so a future edit that
    /// re-points the re-export at a divergent copy fails here too. The
    /// exhaustive parser tests live in `musicpack_core::player::source_url`.
    #[test]
    fn the_key_form_is_the_shared_core_definition() {
        use musicpack_core::player::source_url as core_form;
        let key = format_container_source("/srv/library/a.mpak", "audio/01.mpc");
        assert_eq!(
            key,
            core_form::format_container_source("/srv/library/a.mpak", "audio/01.mpc")
        );
        assert_eq!(CONTAINER_PREFIX, core_form::CONTAINER_PREFIX);
        assert_eq!(CONTAINER_SEPARATOR, core_form::CONTAINER_SEPARATOR);
        let parsed = parse_container_source(&key)
            .unwrap()
            .expect("container key");
        assert_eq!(
            parsed,
            ContainerSourceUrl {
                container: "/srv/library/a.mpak".into(),
                member: "audio/01.mpc".into(),
            }
        );
        assert_eq!(transport_url(&key), "/srv/library/a.mpak");
    }

    fn counting_fetch(bytes: Vec<u8>, calls: Rc<std::cell::Cell<usize>>) -> RangeFetcher {
        Rc::new(move |_url: &str, offset: u64, len: usize| {
            calls.set(calls.get() + 1);
            let start = (offset as usize).min(bytes.len());
            let end = (start + len).min(bytes.len());
            Ok(bytes[start..end].to_vec())
        })
    }

    #[test]
    fn byte_source_reads_exactly_and_resumes_short_reads() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let calls = Rc::new(std::cell::Cell::new(0));
        // A host that never returns more than 64 bytes per call: every read must
        // still be satisfied exactly.
        let capped = {
            let bytes = bytes.clone();
            let calls = calls.clone();
            Rc::new(move |url: &str, offset: u64, len: usize| {
                calls.set(calls.get() + 1);
                assert_eq!(url, "container.mpak");
                let start = (offset as usize).min(bytes.len());
                let end = (start + len.min(64)).min(bytes.len());
                Ok(bytes[start..end].to_vec())
            }) as RangeFetcher
        };
        let source = RangeByteSource::new(capped, "container.mpak", bytes.len() as u64);
        assert_eq!(source.size(), bytes.len() as u64);
        let mut out = vec![0u8; 300];
        source.read_at(100, &mut out).unwrap();
        assert_eq!(out.as_slice(), &bytes[100..400]);
        assert!(calls.get() > 1, "a capped host must be called repeatedly");

        // A zero-length read is a no-op, not an EOF error.
        source.read_at(0, &mut []).unwrap();
    }

    #[test]
    fn byte_source_rejects_ranges_outside_the_source_and_stalled_transports() {
        let bytes = vec![7u8; 64];
        let source = RangeByteSource::new(
            counting_fetch(bytes.clone(), Rc::new(Cell::new(0))),
            "c",
            64,
        );
        let mut out = [0u8; 8];
        assert!(source.read_at(60, &mut out).is_err(), "past the end");

        let stalled = RangeByteSource::new(
            Rc::new(|_url: &str, _offset: u64, _len: usize| Ok(Vec::new())),
            "c",
            64,
        );
        assert!(stalled.read_at(0, &mut out).is_err(), "no progress");

        let failing = RangeByteSource::new(
            Rc::new(|_url: &str, _offset: u64, _len: usize| Err("boom".into())),
            "c",
            64,
        );
        assert!(failing.read_at(0, &mut out).is_err(), "transport error");
    }

    #[test]
    fn plain_sources_still_stream_from_the_transport() {
        let calls = Rc::new(Cell::new(0));
        let backend = RangeSourceBackend::new(counting_fetch(vec![1u8; 300], calls.clone()));
        let mut reader = backend
            .open_source(&PlaybackSource {
                kind: musicpack_core::player::types::SourceKind::HttpRange,
                url: "https://host/audio.mpc".into(),
                byte_size: Some(300),
            })
            .expect("plain source");
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();
        assert_eq!(all.len(), 300);
        assert_eq!(
            backend.cached_containers(),
            0,
            "a plain source scans nothing"
        );
    }

    #[test]
    fn a_container_member_without_a_transport_size_is_a_clear_error() {
        let backend = RangeSourceBackend::new(counting_fetch(Vec::new(), Rc::new(Cell::new(0))));
        let error = backend
            .open_source(&PlaybackSource {
                kind: musicpack_core::player::types::SourceKind::Other("mpak".into()),
                url: format_container_source("https://host/x.mpak", "audio/01.mpc"),
                byte_size: None,
            })
            .err()
            .expect("size is required");
        assert!(error.0.contains("byte_size"), "{}", error.0);
    }
}
