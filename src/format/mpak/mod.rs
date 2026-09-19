//! MPAK v1 single-file container: framing, CRC-16, scan-oriented reader,
//! and deterministic writer.
//!
//! `specs/mpak-v1.md` is the normative reference; the reference C
//! implementation (`core/libmusicpack/src/mpak.c`) is the behavioural
//! authority where the two disagree (documented in `docs/architecture.md`).
//!
//! ## Layout (normative for writers; readers scan)
//!
//! ```text
//! header (16) | INDX? | MANF | DATA… | TAIL?
//! ```
//!
//! - 16-byte fixed header: `"MPAK"` magic, u8 major/minor, big-endian u16
//!   flags, 8 reserved bytes (readers tolerate non-zero at warning level;
//!   writers emit zero).
//! - Uniform 14-byte block framing: 4-byte type code, big-endian u64
//!   payload length, big-endian CRC-16 over bytes 0..11.
//! - Block types `INDX`/`MANF`/`DATA`/`TAIL`; unknown types are skipped by
//!   their declared length; any code containing a byte outside uppercase
//!   `A`–`Z` is the private/experimental namespace.
//! - Readers locate blocks by scanning, never by assumed position; INDX is
//!   acceleration only and is discarded whenever it disagrees with the scan.
//!
//! ## Trust order (security invariant)
//!
//! For every candidate block:
//!
//! ```text
//! frame (14 bytes present)
//!   → CRC-16 validation
//!   → length trust
//!   → overflow/bounds validation (pos+14+length ≤ file size, length ≤ 2^63−1)
//!   → member consumption
//! ```
//!
//! No allocation is sized from a declared length before those checks, all
//! offset/length arithmetic is checked, and a malformed container can
//! neither panic nor scan without making progress.
//!
//! ## Module layout
//!
//! - the reader ([`MpakReader`], [`scan`]) and the [`ByteSource`] input seam;
//! - the deterministic writer ([`write_mpak`], [`canonical_pack_order`]).

mod read;
mod write;

pub use read::{Member, MemberReader, MemorySource, MpakReader, scan};
pub use write::{PackMember, PackSource, canonical_pack_order, write_mpak};

/// The container magic: ASCII `"MPAK"` (`4D 50 41 4B`).
pub const MAGIC: [u8; 4] = *b"MPAK";

/// MPAK major version understood (and written) by this crate.
pub const MAJOR_VERSION: u8 = 1;

/// MPAK minor version understood (and written) by this crate.
pub const MINOR_VERSION: u8 = 0;

/// Size of the fixed container preamble in bytes.
pub const HEADER_LEN: usize = 16;

/// Header flag bit 0: an `INDX` block is present.
pub const FLAG_INDX_PRESENT: u16 = 0x0001;

/// Size of the uniform per-block framing header (type, length, CRC-16).
pub const BLOCK_HEADER_LEN: usize = 14;

/// Exact `TAIL` payload size (8 + 4 + 8 + 32 bytes).
pub const TAIL_PAYLOAD_LEN: usize = 52;

/// Wire-level maximum block payload: `2^63 − 1`.
///
/// A declared length above this is rejected before it is trusted
/// (`MPAK_MAX_BLOCK_LENGTH` in the reference).
pub const MAX_BLOCK_LENGTH: u64 = u64::MAX >> 1;

/// Maximum number of `DATA` members (`MPAK_MAX_MEMBERS`).
pub const MAX_MEMBERS: usize = 4096;

/// Maximum `INDX` payload accepted: twice the manifest budget, matching the
/// reference's `MPAK_MANIFEST_MAX * 2` bound (≈ 32 MiB).
pub const MAX_INDX_PAYLOAD: u64 = 2 * crate::limits::MANIFEST_MAX_BYTES as u64;

/// Maximum accepted `MANF` payload (the manifest budget).
pub const MAX_MANIFEST_LEN: u64 = crate::limits::MANIFEST_MAX_BYTES as u64;

/// A registered v1 public block type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    /// Physical acceleration index (recommended, optional, rebuildable).
    Indx,
    /// The exact canonical `manifest.json` bytes (required, exactly one).
    Manf,
    /// One member object (path preamble + byte-exact payload).
    Data,
    /// Fixity and identity trailer (optional in v1).
    Tail,
}

impl BlockType {
    /// The 4-byte on-disk code for this block type.
    pub fn code(self) -> [u8; 4] {
        match self {
            BlockType::Indx => *b"INDX",
            BlockType::Manf => *b"MANF",
            BlockType::Data => *b"DATA",
            BlockType::Tail => *b"TAIL",
        }
    }

    /// Classifies a 4-byte code. Returns `None` for anything outside the
    /// v1 public registry — which callers must then treat as an
    /// unknown-but-skippable block (after framing CRC passes), never as an
    /// error.
    pub fn from_code(code: [u8; 4]) -> Option<Self> {
        Some(match &code {
            b"INDX" => BlockType::Indx,
            b"MANF" => BlockType::Manf,
            b"DATA" => BlockType::Data,
            b"TAIL" => BlockType::Tail,
            _ => return None,
        })
    }
}

/// Returns `true` when a block code is in the private/experimental
/// namespace: any byte outside uppercase ASCII `A`–`Z` (lowercase letters,
/// digits, non-ASCII). Conforming readers skip these without inspection.
pub fn is_private_code(code: &[u8; 4]) -> bool {
    code.iter().any(|&b| !b.is_ascii_uppercase())
}

/// CRC-16/BUYPASS: polynomial `0x8005`, init `0xFFFF`, no reflection,
/// xorout `0x0000`.
///
/// Used for MPAK block framing (CRC over bytes 0..11 of each block header).
/// Verifying the CRC must precede trusting the declared `length`.
pub fn crc16_buypass(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            let msb = crc & 0x8000 != 0;
            crc <<= 1;
            if msb {
                crc ^= 0x8005;
            }
        }
    }
    crc
}

/// Big-endian wire readers/writers shared by the reader and the writer.
pub(crate) fn rd_u16(p: &[u8]) -> u16 {
    u16::from_be_bytes([p[0], p[1]])
}

pub(crate) fn rd_u32(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

pub(crate) fn rd_u64(p: &[u8]) -> u64 {
    u64::from_be_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]])
}

/// A random-access byte source for container parsing.
///
/// The container layer never touches a filesystem: native files, in-memory
/// buffers, and future range/OPFS sources all implement this. `read_at`
/// performs an *exact* read of `out.len()` bytes at `offset`; a range that
/// is not wholly inside the source is an error (callers only ever read
/// within `[0, size())` after the framing checks).
pub trait ByteSource {
    /// Total source size in bytes.
    fn size(&self) -> u64;

    /// Exact read of `out.len()` bytes at `offset`.
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), ReadError>;
}

/// A failed [`ByteSource`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    /// Human-readable failure description.
    pub detail: String,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "container read failed: {}", self.detail)
    }
}

impl std::error::Error for ReadError {}

/// Builds a 14-byte block header (type, big-endian length, CRC-16).
pub(crate) fn block_header(block_type: &[u8; 4], length: u64) -> [u8; BLOCK_HEADER_LEN] {
    let mut hdr = [0u8; BLOCK_HEADER_LEN];
    hdr[0..4].copy_from_slice(block_type);
    hdr[4..12].copy_from_slice(&length.to_be_bytes());
    let crc = crc16_buypass(&hdr[0..12]);
    hdr[12..14].copy_from_slice(&crc.to_be_bytes());
    hdr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_reference_vectors() {
        // Cross-verified against the C reference routine (mpak.c's
        // table-driven CRC-16/BUYPASS) and the spec's worked example.
        // 0xAEE7 is the CRC-16/BUYPASS (poly 0x8005, init 0xFFFF, no
        // reflection, xorout 0) of the ASCII string "123456789".
        assert_eq!(crc16_buypass(b"123456789"), 0xAEE7);
        // Independent double-check: a single byte.
        assert_eq!(crc16_buypass(b"\x31"), 0x7DA7);
    }

    #[test]
    fn crc16_matches_the_spec_example() {
        // mpak-v1.md §12.2: DATA block header for a member with
        // length = 14; bytes 0..11 are type + big-endian u64 length, and
        // the framed CRC-16 is 0x6710.
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(b"DATA");
        bytes[4..12].copy_from_slice(&14u64.to_be_bytes());
        assert_eq!(crc16_buypass(&bytes), 0x6710);
        // The header helper agrees.
        assert_eq!(&block_header(b"DATA", 14)[12..14], &[0x67, 0x10]);
    }

    #[test]
    fn block_codes_round_trip() {
        for t in [
            BlockType::Indx,
            BlockType::Manf,
            BlockType::Data,
            BlockType::Tail,
        ] {
            assert_eq!(BlockType::from_code(t.code()), Some(t));
        }
        assert_eq!(BlockType::from_code(*b"XDUX"), None); // unknown public-style code
    }

    #[test]
    fn private_namespace_detection() {
        assert!(!is_private_code(b"INDX"));
        assert!(!is_private_code(b"XDUX")); // unknown, but public-style: skip as unknown
        assert!(is_private_code(b"mpc1")); // lowercase
        assert!(is_private_code(b"MPC1")); // digit
        assert!(is_private_code(b"MP\xC3\x84")); // non-ASCII
    }

    #[test]
    fn wire_integer_round_trip() {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(rd_u64(&buf), 0x0102_0304_0506_0708);
        assert_eq!(rd_u32(&buf), 0x0102_0304);
        assert_eq!(rd_u16(&buf), 0x0102);
    }
}
