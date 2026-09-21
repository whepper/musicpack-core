//! Deterministic MPAK v1 writer — port of the writer half of `mpak.c`
//! (`musicpack_mpak_pack_dir`).
//!
//! Output is a pure function of the logical package: no timestamps, no
//! randomness, no filesystem enumeration order. Block order is fixed
//! (`header, INDX, MANF, DATA…, TAIL`), `DATA` follows the manifest's
//! canonical traversal order, and `INDX` entries are sorted by path bytes.
//! Two runs over equivalent input produce byte-identical containers.
//!
//! The whole layout is computed before the first byte is written (single
//! pass, no backpatching). Every member's declared SHA-256 is verified
//! while its bytes are copied, so a file that changes underneath the writer
//! aborts the pack instead of embedding a hash that does not describe the
//! stored bytes.
//!
//! The `TAIL` package digest covers every byte preceding `TAIL` and is
//! computed incrementally while writing (equivalent to the reference's
//! re-read of the flushed file, without the second pass).

use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::format::checksum;
use crate::format::manifest::Manifest;
use crate::limits::{MAX_FILE_BYTES, MAX_TOTAL_BYTES, PATH_MAX_BYTES};

use super::{
    BLOCK_HEADER_LEN, HEADER_LEN, MAGIC, MAJOR_VERSION, MAX_MANIFEST_LEN, MAX_MEMBERS,
    MINOR_VERSION, TAIL_PAYLOAD_LEN, block_header,
};

/// One member to store: canonical path + the SHA-256 its manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMember {
    /// Canonical package-relative path.
    pub path: String,
    /// 64 lowercase hex characters (validated by the writer).
    pub sha256_hex: String,
}

/// The logical package the writer packs.
///
/// Implementations supply the *exact* manifest bytes (never regenerated) and
/// the members in canonical traversal order.
pub trait PackSource {
    /// Exact `manifest.json` bytes to embed (the writer does not reformat).
    fn manifest_bytes(&self) -> &[u8];

    /// Members in canonical traversal order (see [`canonical_pack_order`]).
    fn members(&self) -> &[PackMember];

    /// Byte size of a member's content.
    fn member_size(&self, path: &str) -> Result<u64, Error>;

    /// Streams a member's content. The writer verifies the byte count and
    /// SHA-256 against the [`PackMember`] while copying.
    fn read_member(&self, path: &str) -> Result<Box<dyn Read + '_>, Error>;
}

/// The manifest's canonical traversal order for `DATA` blocks, matching the
/// reference writer exactly: all primary audio (media/track array order),
/// then all representations, then all waveforms, then per-track lyrics,
/// then artwork, booklet, lyrics, extras, analysis (each in array order).
///
/// Note: `specs/mpak-v1.md` §7 lists audio → representations → artwork… but
/// omits waveforms; the reference interleaves all waveforms between
/// representations and artwork. The implementation is the behavioural
/// authority (documented in `docs/architecture.md`); this order is what
/// makes reference byte-identity possible.
///
/// Per-track lyrics are the one Rust-defined group
/// (`docs/musicpack-lyrics-v1.md` §6.5): they slot after waveforms so all
/// per-track asset groups stay together ahead of the package-level
/// groups. Manifests without the additive field produce byte-identical
/// orders to before.
pub fn canonical_pack_order(manifest: &Manifest) -> Vec<(&str, &str)> {
    let mut order: Vec<(&str, &str)> = Vec::new();
    for disc in &manifest.media {
        for track in &disc.tracks {
            order.push((track.audio.path.as_str(), track.audio.sha256.as_str()));
        }
    }
    for disc in &manifest.media {
        for track in &disc.tracks {
            for representation in &track.representations {
                order.push((representation.path.as_str(), representation.sha256.as_str()));
            }
        }
    }
    for disc in &manifest.media {
        for track in &disc.tracks {
            if let Some(waveform) = &track.waveform {
                order.push((waveform.path.as_str(), waveform.sha256.as_str()));
            }
        }
    }
    for disc in &manifest.media {
        for track in &disc.tracks {
            for lyrics in &track.lyrics {
                order.push((lyrics.path.as_str(), lyrics.sha256.as_str()));
            }
        }
    }
    for artwork in &manifest.artwork {
        order.push((artwork.asset.path.as_str(), artwork.asset.sha256.as_str()));
    }
    for asset in &manifest.booklet {
        order.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for asset in &manifest.lyrics {
        order.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for asset in &manifest.extras {
        order.push((asset.path.as_str(), asset.sha256.as_str()));
    }
    for analysis in &manifest.analysis {
        order.push((analysis.asset.path.as_str(), analysis.asset.sha256.as_str()));
    }
    order
}

/// Writes a deterministic MPAK v1 container.
pub fn write_mpak<S: PackSource + ?Sized>(source: &S, out: &mut dyn Write) -> Result<(), Error> {
    let manifest = source.manifest_bytes();
    let members = source.members();

    let invalid = |detail: &str| Error::Invalid {
        detail: detail.to_string(),
    };

    if manifest.len() as u64 > MAX_MANIFEST_LEN {
        return Err(invalid("manifest exceeds the 16 MiB container budget"));
    }
    if members.len() > MAX_MEMBERS {
        return Err(invalid("more than 4096 members"));
    }

    // Sizes and total (checked, budgeted) before any layout arithmetic.
    let mut sizes: Vec<u64> = Vec::with_capacity(members.len());
    let mut total: u64 = 0;
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for member in members {
        let path_len = member.path.len();
        if path_len == 0 || path_len > PATH_MAX_BYTES {
            return Err(invalid("member path length out of bounds"));
        }
        if !checksum::is_valid_sha256_hex(&member.sha256_hex) {
            return Err(invalid("member sha256 is not 64 lowercase hex characters"));
        }
        if !seen.insert(member.path.as_str()) {
            return Err(invalid("duplicate member path"));
        }
        let size = source.member_size(&member.path)?;
        if size > MAX_FILE_BYTES {
            return Err(invalid("member exceeds the 8 GiB per-file limit"));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("aggregate size overflow"))?;
        sizes.push(size);
    }
    if total > MAX_TOTAL_BYTES {
        return Err(invalid("aggregate member bytes exceed the 64 GiB limit"));
    }

    // INDX payload size and member offsets (single pass, no backpatching).
    let mut indx_payload: u64 = 4;
    for member in members {
        indx_payload = indx_payload
            .checked_add(2 + member.path.len() as u64 + 48)
            .ok_or_else(|| invalid("INDX size overflow"))?;
    }
    let mut offsets: Vec<u64> = Vec::with_capacity(members.len());
    let mut pos: u64 = HEADER_LEN as u64
        + BLOCK_HEADER_LEN as u64
        + indx_payload
        + BLOCK_HEADER_LEN as u64
        + manifest.len() as u64;
    for (member, &size) in members.iter().zip(sizes.iter()) {
        pos = pos
            .checked_add(BLOCK_HEADER_LEN as u64 + 2 + member.path.len() as u64)
            .ok_or_else(|| invalid("member offset overflow"))?;
        offsets.push(pos);
        pos = pos
            .checked_add(size)
            .ok_or_else(|| invalid("member end overflow"))?;
    }
    let mut w = HashingWriter::new(out);

    // ---- header ----
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(&MAGIC);
    header[4] = MAJOR_VERSION;
    header[5] = MINOR_VERSION;
    header[6..8].copy_from_slice(&super::FLAG_INDX_PRESENT.to_be_bytes());
    // reserved (bytes 8..16) stays zero
    w.write_all(&header).map_err(io_error)?;

    // ---- INDX (entries sorted lexicographically by path bytes) ----
    let mut sorted: Vec<usize> = (0..members.len()).collect();
    sorted.sort_by(|&a, &b| members[a].path.as_bytes().cmp(members[b].path.as_bytes()));
    w.write_all(&block_header(b"INDX", indx_payload))
        .map_err(io_error)?;
    w.write_all(&(members.len() as u32).to_be_bytes())
        .map_err(io_error)?;
    for &i in &sorted {
        let path = members[i].path.as_bytes();
        w.write_all(&(path.len() as u16).to_be_bytes())
            .map_err(io_error)?;
        w.write_all(path).map_err(io_error)?;
        w.write_all(&offsets[i].to_be_bytes()).map_err(io_error)?;
        w.write_all(&sizes[i].to_be_bytes()).map_err(io_error)?;
        let digest = checksum::sha256_hex_to_bytes(&members[i].sha256_hex)
            .ok_or_else(|| invalid("member sha256 is not valid hex"))?;
        w.write_all(&digest).map_err(io_error)?;
    }

    // ---- MANF (exact bytes) ----
    w.write_all(&block_header(b"MANF", manifest.len() as u64))
        .map_err(io_error)?;
    w.write_all(manifest).map_err(io_error)?;

    // ---- DATA (canonical traversal order) ----
    for (i, member) in members.iter().enumerate() {
        let payload = 2 + member.path.len() as u64 + sizes[i];
        w.write_all(&block_header(b"DATA", payload))
            .map_err(io_error)?;
        w.write_all(&(member.path.len() as u16).to_be_bytes())
            .map_err(io_error)?;
        w.write_all(member.path.as_bytes()).map_err(io_error)?;

        // Stream + verify the member content against its declaration.
        let mut reader = source.read_member(&member.path)?;
        let mut hasher = Sha256::new();
        let mut copied: u64 = 0;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| Error::Io {
                detail: e.to_string(),
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            w.write_all(&buf[..n]).map_err(io_error)?;
            copied = copied
                .checked_add(n as u64)
                .ok_or_else(|| invalid("member size overflow"))?;
        }
        if copied != sizes[i] {
            return Err(Error::Checksum {
                path: member.path.clone(),
                expected: member.sha256_hex.clone(),
                actual: format!("{copied}-byte member changed during pack"),
            });
        }
        let digest: [u8; 32] = hasher.finalize().into();
        if checksum::to_hex(&digest) != member.sha256_hex {
            return Err(Error::Checksum {
                path: member.path.clone(),
                expected: member.sha256_hex.clone(),
                actual: checksum::to_hex(&digest),
            });
        }
    }

    // ---- TAIL (digest over every preceding byte) ----
    let tail_offset = w.bytes_written;
    let total_size = tail_offset + BLOCK_HEADER_LEN as u64 + TAIL_PAYLOAD_LEN as u64;
    let prefix_digest = w.prefix_digest();
    w.write_all(&block_header(b"TAIL", TAIL_PAYLOAD_LEN as u64))
        .map_err(io_error)?;
    let mut tail = [0u8; TAIL_PAYLOAD_LEN];
    tail[0..8].copy_from_slice(&total_size.to_be_bytes());
    tail[8..12].copy_from_slice(&(members.len() as u32).to_be_bytes());
    tail[12..20].copy_from_slice(&(HEADER_LEN as u64).to_be_bytes());
    tail[20..52].copy_from_slice(&prefix_digest);
    w.write_all(&tail).map_err(io_error)?;

    w.inner.flush().map_err(io_error)?;
    Ok(())
}

fn io_error(e: std::io::Error) -> Error {
    Error::Io {
        detail: e.to_string(),
    }
}

/// A writer wrapper that counts bytes and hashes the prefix written so far
/// (for the `TAIL` package digest).
struct HashingWriter<'a> {
    inner: &'a mut dyn Write,
    bytes_written: u64,
    hasher: Sha256,
}

impl<'a> HashingWriter<'a> {
    fn new(inner: &'a mut dyn Write) -> Self {
        Self {
            inner,
            bytes_written: 0,
            hasher: Sha256::new(),
        }
    }

    /// Digest of everything written so far (cloned state, non-destructive).
    fn prefix_digest(&self) -> [u8; 32] {
        self.hasher.clone().finalize().into()
    }
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
