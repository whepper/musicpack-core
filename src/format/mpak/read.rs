//! Scan-oriented MPAK v1 reader — port of the reader half of `mpak.c`.
//!
//! [`scan`] walks the block stream from byte 0 and never depends on block
//! positions: every block is located by scanning, CRC-validated, and only
//! then has its declared length trusted. `INDX` is parsed as acceleration
//! and **discarded** whenever it disagrees with the scan
//! (`reconcile_indx`), so an INDX-based lookup can never point anywhere the
//! byte stream does not.
//!
//! Recovery mode (`recovery = true`, the reference's unpack policy) turns
//! structurally invalid `DATA` preambles into counted skips instead of hard
//! errors; normal loading keeps every preamble failure fatal. Broken block
//! framing resynchronizes by advancing one byte and retrying — best-effort
//! structure recovery only, never an integrity guarantee (recovered members
//! are still SHA-256-verified by the caller).

use std::collections::HashMap;

use crate::error::Error;
use crate::format::path;
use crate::limits::{MAX_FILE_BYTES, PATH_MAX_BYTES};

use super::{
    BLOCK_HEADER_LEN, ByteSource, MAJOR_VERSION, MAX_BLOCK_LENGTH, MAX_INDX_PAYLOAD,
    MAX_MANIFEST_LEN, MAX_MEMBERS, ReadError, TAIL_PAYLOAD_LEN,
};

/// A `DATA` member as discovered by the scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// Canonical package-relative path (validated at scan time).
    pub path: String,
    /// Absolute offset of the member's first byte (past the DATA header and
    /// path preamble).
    pub offset: u64,
    /// Member byte length.
    pub length: u64,
}

/// One parsed, structurally valid `INDX` entry (sorted by path bytes).
#[derive(Debug, Clone, PartialEq)]
struct IndxEntry {
    path: String,
    offset: u64,
    length: u64,
    sha256: [u8; 32],
}

/// The parsed container.
///
/// Facts retained for verification (port of `musicpack_mpak`):
/// header version/reserved, scan results, INDX validity, TAIL contents,
/// recovery counters.
#[derive(Debug)]
pub struct MpakReader {
    file_size: u64,
    minor: u8,
    reserved_nonzero: bool,
    resynced: bool,

    members: Vec<Member>,
    member_index: HashMap<String, usize>,
    duplicate_members: usize,
    duplicate_example: Option<String>,
    skipped_members: usize,

    manifest: Option<Vec<u8>>,
    manifest_count: usize,

    indx_present: bool,
    indx_offset: u64,
    indx_length: u64,
    indx_extra: bool,
    indx_duplicate: bool,
    indx_entries: Vec<IndxEntry>,
    indx_valid: bool,

    tail_present: bool,
    tail_offset: u64,
    tail_extra: bool,
    tail_malformed: bool,
    tail_total_size: u64,
    tail_objects: u32,
    tail_indx_offset: u64,
    tail_digest: [u8; 32],
}

impl MpakReader {
    /// Container size in bytes.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Members in `DATA` (scan) order; a duplicated path keeps its first
    /// occurrence.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// Looks up a member by canonical path.
    pub fn member(&self, rel: &str) -> Option<&Member> {
        self.member_index.get(rel).map(|&i| &self.members[i])
    }

    /// The exact `MANF` payload bytes, when present.
    pub fn manifest_bytes(&self) -> Option<&[u8]> {
        self.manifest.as_deref()
    }

    /// Container minor version.
    pub fn minor(&self) -> u8 {
        self.minor
    }

    /// Whether the reserved header bytes were non-zero (tolerated).
    pub fn reserved_nonzero(&self) -> bool {
        self.reserved_nonzero
    }

    /// Whether the scan had to resynchronize past damaged framing.
    pub fn resynced(&self) -> bool {
        self.resynced
    }

    /// Number of `DATA` members with a duplicate path (first kept).
    pub fn duplicate_members(&self) -> usize {
        self.duplicate_members
    }

    /// First duplicated path, for the verification message.
    pub fn duplicate_example(&self) -> Option<&str> {
        self.duplicate_example.as_deref()
    }

    /// Recovery-mode skipped `DATA` members.
    pub fn skipped_members(&self) -> usize {
        self.skipped_members
    }

    /// Number of `MANF` blocks seen (exactly one is required by `open`).
    pub fn manifest_count(&self) -> usize {
        self.manifest_count
    }

    /// Whether an `INDX` block was present.
    pub fn indx_present(&self) -> bool {
        self.indx_present
    }

    /// Absolute `INDX` block offset.
    pub fn indx_offset(&self) -> u64 {
        self.indx_offset
    }

    /// Whether more than one `INDX` block was present.
    pub fn indx_extra(&self) -> bool {
        self.indx_extra
    }

    /// Whether the `INDX` was discarded.
    pub fn indx_invalid(&self) -> bool {
        self.indx_present && !self.indx_valid
    }

    /// Whether the `INDX` was discarded specifically for a duplicate path.
    pub fn indx_duplicate(&self) -> bool {
        self.indx_duplicate
    }

    /// Whether a parsed `INDX` agreed with the scan.
    pub fn indx_valid(&self) -> bool {
        self.indx_valid
    }

    /// SHA-256 declared by the `INDX` for `rel`, when the index is valid.
    pub fn indx_sha256(&self, rel: &str) -> Option<&[u8; 32]> {
        if !self.indx_valid {
            return None;
        }
        self.indx_entries
            .binary_search_by(|e| e.path.as_str().cmp(rel))
            .ok()
            .map(|i| &self.indx_entries[i].sha256)
    }

    /// Whether a `TAIL` block was present.
    pub fn tail_present(&self) -> bool {
        self.tail_present
    }

    /// Absolute `TAIL` block offset.
    pub fn tail_offset(&self) -> u64 {
        self.tail_offset
    }

    /// Whether more than one `TAIL` block was present.
    pub fn tail_extra(&self) -> bool {
        self.tail_extra
    }

    /// Whether the `TAIL` payload length was wrong (treated as absent).
    pub fn tail_malformed(&self) -> bool {
        self.tail_malformed
    }

    /// `TAIL.total_file_size`.
    pub fn tail_total_size(&self) -> u64 {
        self.tail_total_size
    }

    /// `TAIL.object_count`.
    pub fn tail_objects(&self) -> u32 {
        self.tail_objects
    }

    /// `TAIL.indx_offset`.
    pub fn tail_indx_offset(&self) -> u64 {
        self.tail_indx_offset
    }

    /// `TAIL.package_digest`.
    pub fn tail_digest(&self) -> &[u8; 32] {
        &self.tail_digest
    }
}

/// Scans a container.
///
/// `recovery` mirrors the reference's unpack policy: invalid `DATA`
/// preambles are skipped and counted instead of failing the scan.
pub fn scan(source: &dyn ByteSource, recovery: bool) -> Result<MpakReader, Error> {
    let file_size = source.size();
    if file_size < super::HEADER_LEN as u64 {
        return Err(Error::Invalid {
            detail: "container is smaller than the 16-byte header".into(),
        });
    }

    let mut header = [0u8; super::HEADER_LEN];
    read_exact(source, 0, &mut header)?;
    if header[0..4] != super::MAGIC {
        return Err(Error::Invalid {
            detail: "not an MPAK container (bad magic)".into(),
        });
    }
    let major = header[4];
    let minor = header[5];
    if major != MAJOR_VERSION {
        return Err(Error::Version {
            found: major.to_string(),
            supported: MAJOR_VERSION as u64,
        });
    }
    let _flags = u16::from_be_bytes([header[6], header[7]]);
    let reserved_nonzero = u64::from_be_bytes([
        header[8], header[9], header[10], header[11], header[12], header[13], header[14],
        header[15],
    ]) != 0;

    let mut reader = MpakReader {
        file_size,
        minor,
        reserved_nonzero,
        resynced: false,
        members: Vec::new(),
        member_index: HashMap::new(),
        duplicate_members: 0,
        duplicate_example: None,
        skipped_members: 0,
        manifest: None,
        manifest_count: 0,
        indx_present: false,
        indx_offset: 0,
        indx_length: 0,
        indx_extra: false,
        indx_duplicate: false,
        indx_entries: Vec::new(),
        indx_valid: false,
        tail_present: false,
        tail_offset: 0,
        tail_extra: false,
        tail_malformed: false,
        tail_total_size: 0,
        tail_objects: 0,
        tail_indx_offset: 0,
        tail_digest: [0u8; 32],
    };

    let mut pos: u64 = super::HEADER_LEN as u64;
    while pos
        .checked_add(BLOCK_HEADER_LEN as u64)
        .is_some_and(|end| end <= file_size)
    {
        let mut bhdr = [0u8; BLOCK_HEADER_LEN];
        read_exact(source, pos, &mut bhdr)?;

        // Trust order: frame → CRC → length → bounds → consumption.
        if !framing_ok(&bhdr) {
            pos += 1;
            reader.resynced = true;
            continue;
        }
        let length = super::rd_u64(&bhdr[4..12]);
        let Some(payload_pos) = pos.checked_add(BLOCK_HEADER_LEN as u64) else {
            pos += 1;
            reader.resynced = true;
            continue;
        };
        let Some(next) = payload_pos.checked_add(length) else {
            pos += 1;
            reader.resynced = true;
            continue;
        };
        if length > MAX_BLOCK_LENGTH || next > file_size {
            pos += 1;
            reader.resynced = true;
            continue;
        }

        let mut block_type = [0u8; 4];
        block_type.copy_from_slice(&bhdr[0..4]);
        match &block_type {
            b"DATA" => {
                scan_data_payload(source, &mut reader, payload_pos, length, recovery)?;
            }
            b"MANF" => {
                if length > MAX_MANIFEST_LEN {
                    return Err(Error::Invalid {
                        detail: "MANF payload exceeds the manifest budget".into(),
                    });
                }
                if reader.manifest_count == 0 {
                    let mut bytes = vec![0u8; length as usize];
                    read_exact(source, payload_pos, &mut bytes)?;
                    reader.manifest = Some(bytes);
                }
                reader.manifest_count += 1;
            }
            b"INDX" => {
                if !reader.indx_present {
                    reader.indx_present = true;
                    reader.indx_offset = pos;
                    reader.indx_length = length;
                } else {
                    reader.indx_extra = true;
                }
            }
            b"TAIL" => {
                if !reader.tail_present {
                    reader.tail_present = true;
                    reader.tail_offset = pos;
                    if length != TAIL_PAYLOAD_LEN as u64 {
                        reader.tail_malformed = true;
                    } else {
                        let mut payload = [0u8; TAIL_PAYLOAD_LEN];
                        read_exact(source, payload_pos, &mut payload)?;
                        reader.tail_total_size = super::rd_u64(&payload[0..8]);
                        reader.tail_objects = super::rd_u32(&payload[8..12]);
                        reader.tail_indx_offset = super::rd_u64(&payload[12..20]);
                        reader.tail_digest.copy_from_slice(&payload[20..52]);
                    }
                } else {
                    reader.tail_extra = true;
                }
            }
            // Unknown public and private/experimental types are skipped by
            // the declared length (already bounds-checked above).
            _ => {}
        }

        pos = next;
    }

    if reader.indx_present {
        match parse_indx(source, &reader) {
            Ok(entries) => {
                reader.indx_entries = entries;
                reader.indx_valid = true;
                reconcile_indx(&mut reader);
            }
            Err(failure) if failure.io => {
                return Err(Error::Io {
                    detail: "cannot read INDX payload".into(),
                });
            }
            Err(failure) => {
                // The index is optional: fall back to the scan. A duplicate
                // path is recorded so verification can report it as an
                // error (everything else is a warning).
                reader.indx_duplicate = failure.duplicate;
                reader.indx_valid = false;
            }
        }
    }

    Ok(reader)
}

/// Validates a candidate block header: CRC first (bytes 0..11), then the
/// declared length. Returns `false` to trigger one-byte resynchronization.
fn framing_ok(bhdr: &[u8; BLOCK_HEADER_LEN]) -> bool {
    let stored = super::rd_u16(&bhdr[12..14]);
    if stored != super::crc16_buypass(&bhdr[0..12]) {
        return false;
    }
    let length = super::rd_u64(&bhdr[4..12]);
    length <= MAX_BLOCK_LENGTH
}

/// Port of `scan_data_payload`. `recovery` turns structural preamble
/// failures into counted skips.
fn scan_data_payload(
    source: &dyn ByteSource,
    reader: &mut MpakReader,
    payload_pos: u64,
    length: u64,
    recovery: bool,
) -> Result<(), Error> {
    macro_rules! skip_or {
        ($code:expr) => {
            if recovery {
                reader.skipped_members += 1;
                return Ok(());
            } else {
                return Err($code);
            }
        };
    }

    if length < 2 {
        skip_or!(Error::Invalid {
            detail: "DATA payload shorter than its path preamble".into()
        });
    }
    let mut pbuf = [0u8; 2];
    read_exact(source, payload_pos, &mut pbuf)?;
    let path_len = super::rd_u16(&pbuf) as u64;
    if path_len == 0 || path_len > PATH_MAX_BYTES as u64 {
        skip_or!(Error::Path(crate::format::path::PathError {
            path: String::new(),
            reason: crate::format::path::PathReason::Empty,
        }));
    }
    if path_len + 2 > length {
        skip_or!(Error::Invalid {
            detail: "DATA path exceeds its block payload".into()
        });
    }
    let mut path_bytes = vec![0u8; path_len as usize];
    read_exact(source, payload_pos + 2, &mut path_bytes)?;
    let member_path = match String::from_utf8(path_bytes) {
        Ok(p) => p,
        Err(_) => skip_or!(Error::Path(crate::format::path::PathError {
            path: String::new(),
            reason: crate::format::path::PathReason::Control,
        })),
    };
    if let Err(e) = path::validate(&member_path) {
        skip_or!(Error::Path(e));
    }

    let member_offset = payload_pos + 2 + path_len;
    let member_length = length - 2 - path_len;
    if member_length > MAX_FILE_BYTES {
        skip_or!(Error::Invalid {
            detail: "DATA member exceeds the per-file limit".into()
        });
    }

    // The 4096-member budget is a physical policy limit in both modes.
    if reader.members.len() >= MAX_MEMBERS {
        return Err(Error::Invalid {
            detail: "container has more than 4096 members".into(),
        });
    }
    if reader.member_index.contains_key(&member_path) {
        reader.duplicate_members += 1;
        if reader.duplicate_example.is_none() {
            reader.duplicate_example = Some(member_path);
        }
        return Ok(()); // keep the first occurrence
    }

    reader
        .member_index
        .insert(member_path.clone(), reader.members.len());
    reader.members.push(Member {
        path: member_path,
        offset: member_offset,
        length: member_length,
    });
    Ok(())
}

/// Why an `INDX` was discarded.
#[derive(Debug, Clone, Copy)]
struct IndxFailure {
    /// The rejection was a duplicate path (an error at verification time).
    duplicate: bool,
    /// An I/O failure (fatal) rather than a structural one (discard).
    io: bool,
}

fn indx_malformed() -> IndxFailure {
    IndxFailure {
        duplicate: false,
        io: false,
    }
}

/// Port of `parse_indx`. `Err` means the index is discarded (fallback to
/// the scan).
fn parse_indx(source: &dyn ByteSource, reader: &MpakReader) -> Result<Vec<IndxEntry>, IndxFailure> {
    if reader.indx_length < 4 || reader.indx_length > MAX_INDX_PAYLOAD {
        return Err(indx_malformed());
    }
    let payload = read_exact_vec(
        source,
        reader.indx_offset + BLOCK_HEADER_LEN as u64,
        reader.indx_length,
    )
    .map_err(|_| IndxFailure {
        duplicate: false,
        io: true,
    })?;
    let count = super::rd_u32(&payload[0..4]) as usize;
    if count > MAX_MEMBERS {
        return Err(indx_malformed());
    }

    let mut entries: Vec<IndxEntry> = Vec::with_capacity(count);
    let mut p = 4usize;
    for _ in 0..count {
        if p + 2 > payload.len() {
            return Err(indx_malformed());
        }
        let path_len = super::rd_u16(&payload[p..p + 2]) as usize;
        p += 2;
        if path_len == 0 || path_len > PATH_MAX_BYTES || p + path_len > payload.len() {
            return Err(indx_malformed());
        }
        let member_path = match std::str::from_utf8(&payload[p..p + path_len]) {
            Ok(s) => s.to_string(),
            Err(_) => return Err(indx_malformed()),
        };
        p += path_len;
        if path::validate(&member_path).is_err() {
            return Err(indx_malformed());
        }
        if let Some(prev) = entries.last() {
            match prev.path.as_str().cmp(member_path.as_str()) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(IndxFailure {
                        duplicate: true,
                        io: false,
                    });
                }
                std::cmp::Ordering::Greater => return Err(indx_malformed()),
            }
        }
        if p + 48 > payload.len() {
            return Err(indx_malformed());
        }
        let offset = super::rd_u64(&payload[p..p + 8]);
        let length = super::rd_u64(&payload[p + 8..p + 16]);
        let mut sha = [0u8; 32];
        sha.copy_from_slice(&payload[p + 16..p + 48]);
        p += 48;
        if length > MAX_FILE_BYTES {
            return Err(indx_malformed());
        }
        if offset
            .checked_add(length)
            .is_none_or(|end| end > reader.file_size)
        {
            return Err(indx_malformed());
        }
        entries.push(IndxEntry {
            path: member_path,
            offset,
            length,
            sha256: sha,
        });
    }
    if p != payload.len() {
        return Err(indx_malformed());
    }
    Ok(entries)
}

/// Port of `reconcile_indx`: the sorted scan paths and every offset/length
/// must agree with the parsed index exactly, otherwise it is discarded.
fn reconcile_indx(reader: &mut MpakReader) {
    if !reader.indx_valid {
        return;
    }
    if reader.indx_entries.len() != reader.members.len() {
        reader.indx_valid = false;
        return;
    }
    let mut sorted: Vec<&Member> = reader.members.iter().collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    for (scan, entry) in sorted.iter().zip(reader.indx_entries.iter()) {
        if scan.path != entry.path || scan.offset != entry.offset || scan.length != entry.length {
            reader.indx_valid = false;
            return;
        }
    }
}

fn read_exact(source: &dyn ByteSource, offset: u64, out: &mut [u8]) -> Result<(), Error> {
    source
        .read_at(offset, out)
        .map_err(|e: ReadError| Error::Io { detail: e.detail })
}

fn read_exact_vec(source: &dyn ByteSource, offset: u64, len: u64) -> Result<Vec<u8>, Error> {
    let mut out = vec![0u8; len as usize];
    read_exact(source, offset, &mut out)?;
    Ok(out)
}

/// A bounded streaming reader over a member's byte range.
///
/// Holds a shared source so it satisfies the owned-reader shape of the
/// storage layer without buffering the member.
pub struct MemberReader {
    source: std::sync::Arc<dyn ByteSource>,
    pos: u64,
    end: u64,
}

impl MemberReader {
    /// Creates a reader over `[base, base + len)`.
    pub fn new(source: std::sync::Arc<dyn ByteSource>, base: u64, len: u64) -> Self {
        Self {
            source,
            pos: base,
            end: base.saturating_add(len),
        }
    }
}

impl std::io::Read for MemberReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.end || buf.is_empty() {
            return Ok(0);
        }
        let want = std::cmp::min(buf.len() as u64, self.end - self.pos) as usize;
        self.source
            .read_at(self.pos, &mut buf[..want])
            .map_err(|e| std::io::Error::other(e.detail))?;
        self.pos += want as u64;
        Ok(want)
    }
}

/// Convenience: an in-memory container source (tests, embedded use).
#[derive(Debug, Clone)]
pub struct MemorySource {
    bytes: std::sync::Arc<Vec<u8>>,
}

impl MemorySource {
    /// Wraps owned bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: std::sync::Arc::new(bytes),
        }
    }
}

impl ByteSource for MemorySource {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), ReadError> {
        let end = offset
            .checked_add(out.len() as u64)
            .ok_or_else(|| ReadError {
                detail: "read range overflow".into(),
            })?;
        if end > self.bytes.len() as u64 {
            return Err(ReadError {
                detail: "read past end of container".into(),
            });
        }
        out.copy_from_slice(&self.bytes[offset as usize..end as usize]);
        Ok(())
    }
}
