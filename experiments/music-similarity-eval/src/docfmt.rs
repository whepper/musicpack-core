//! Reference codec for the **MusicPack similarity document v1** — the format
//! specified in [`FORMAT_SPEC.md`](../FORMAT_SPEC.md).
//!
//! ## Status: SPIKE. Nothing here is production.
//!
//! This module exists for exactly three reasons, all of them review
//! purposes:
//!
//! 1. the committed fixtures in `fixtures/similarity-doc/` must be
//!    *reproducible*, not hand-typed hex;
//! 2. "the same logical input serializes to byte-identical output" has to be a
//!    test, not a promise;
//! 3. the validation rules have to be executable, or the specification is only
//!    prose with confident examples in it.
//!
//! It is a *reference* implementation, in the same sense as the frozen C oracle
//! material under `tools/`: it defines no production behaviour, it is not a
//! member of the root workspace, and it must never be moved into
//! `musicpack-core` as-is. A production reader/writer is still to be written
//! against `FORMAT_SPEC.md` and reviewed on its own merits.
//!
//! ## What this module deliberately does not do
//!
//! - It does not run a model, decode audio, read a library, or touch a
//!   database. Every fixture is synthetic.
//! - It does not know what a profile *means*. The profile fingerprint is an
//!   opaque 32-byte value here; [`profile_fingerprint`] exists only so the
//!   fixture fingerprints are recomputable from a documented field list.
//! - It does not decide anything about queries, indexes, or package identity.
//!   See the spec for those boundaries.
//!
//! ## Layering
//!
//! Validation is split so that the two responsibilities stay separable:
//!
//! - [`validate`] is **document structure**: magic, version, reserved bytes,
//!   counts, member references, status values, declared size, element
//!   finiteness. It knows nothing about profiles and never rejects a document
//!   for carrying an unregistered fingerprint.
//! - [`admit`] is the **consumer decision**: does this structurally valid
//!   document belong to the partition a consumer is serving? It is a
//!   comparison against caller-supplied bytes and nothing else.
//!
//! Merging the two would let a profile question masquerade as a format
//! question, which is exactly the confusion ADR 0017 §5.3 forbids.

use std::fmt;

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Constants (FORMAT_SPEC.md §5)
// ---------------------------------------------------------------------------

/// Container magic: ASCII `"MSIM"` (`4D 53 49 4D`).
pub const MAGIC: [u8; 4] = *b"MSIM";

/// Document major version specified here. A reader that does not implement
/// this major MUST reject the document.
pub const FORMAT_MAJOR: u16 = 1;

/// Document minor version specified here.
pub const FORMAT_MINOR: u16 = 0;

/// Fixed header size in bytes.
pub const HEADER_BYTES: usize = 64;

/// Fixed track-table entry size in bytes.
pub const ENTRY_BYTES: usize = 12;

/// Byte offset of the track table (immediately after the header).
pub const TABLE_OFFSET: usize = HEADER_BYTES;

/// `vector_encoding` value: IEEE 754 binary32, little-endian elements.
pub const ENCODING_F32LE: u8 = 1;

/// `vector_encoding` value: IEEE 754 binary16, little-endian elements.
pub const ENCODING_F16LE: u8 = 2;

/// Element size in bytes for a supported encoding.
pub fn element_size(encoding: u8) -> Option<usize> {
    match encoding {
        ENCODING_F32LE => Some(4),
        ENCODING_F16LE => Some(2),
        _ => None,
    }
}

/// Human-readable encoding name, as used by fingerprints and dumps.
pub fn encoding_name(encoding: u8) -> Option<&'static str> {
    match encoding {
        ENCODING_F32LE => Some("f32le"),
        ENCODING_F16LE => Some("f16le"),
        _ => None,
    }
}

/// `flags` bit 0. **Reserved.** It is named so that a reader can say *what* is
/// set, but bit 0 has no v1.0 meaning: a document that sets it is rejected
/// (ADR 0017 D-3's optional album aggregate is deferred; see the spec §9).
pub const FLAG_RESERVED_ALBUM_AGGREGATE: u8 = 0b0000_0001;

/// Maximum `dimensions` accepted by this specification.
pub const MAX_DIMENSIONS: u32 = 4096;

/// Maximum `track_count` accepted by this specification. Reused from the
/// package limits: `MAX_DISCS * MAX_TRACKS_PER_DISC` (`src/limits.rs`).
pub const MAX_TRACKS: u32 = 32 * 512;

/// `status` value: the track has a vector.
pub const STATUS_OK: u8 = 0;
/// `status` value: too little audio for the profile to say anything.
pub const STATUS_INSUFFICIENT_AUDIO: u8 = 1;
/// `status` value: the input is outside what the profile accepts.
pub const STATUS_UNSUPPORTED: u8 = 2;
/// `status` value: analysis was attempted and did not produce a result.
pub const STATUS_FAILED: u8 = 3;

/// Highest defined `status` value. Everything above is reserved.
pub const STATUS_MAX: u8 = STATUS_FAILED;

/// Human-readable status name, or `None` for an undefined value.
pub fn status_name(status: u8) -> Option<&'static str> {
    match status {
        STATUS_OK => Some("ok"),
        STATUS_INSUFFICIENT_AUDIO => Some("insufficient_audio"),
        STATUS_UNSUPPORTED => Some("unsupported"),
        STATUS_FAILED => Some("failed"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Logical document
// ---------------------------------------------------------------------------

/// One package track's membership row.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// Manifest disc number (`media[].disc`, ≥ 1).
    pub disc: u32,
    /// Manifest track number (`media[].tracks[].track`, ≥ 1).
    pub track: u32,
    /// One of the `STATUS_*` values.
    pub status: u8,
    /// The vector, present **iff** `status == STATUS_OK`.
    pub vector: Option<Vec<f32>>,
}

/// A similarity document as a logical value: what a writer means and a reader
/// recovers. Two documents with equal logical content MUST serialize to equal
/// bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// `format_major`. Always [`FORMAT_MAJOR`] for v1.0.
    pub major: u16,
    /// `format_minor`. Always [`FORMAT_MINOR`] for v1.0.
    pub minor: u16,
    /// `profile_fingerprint`: raw 32 bytes, never all zero.
    pub fingerprint: [u8; 32],
    /// `dimensions`: shared by every vector in the document.
    pub dimensions: u32,
    /// `vector_encoding`.
    pub encoding: u8,
    /// `flags`. Always zero in v1.0.
    pub flags: u8,
    /// Members in canonical ascending `(disc, track)` order.
    pub members: Vec<Member>,
}

impl Document {
    /// Byte offset at which member `index`'s vector starts.
    pub fn vector_offset(&self, index: usize) -> usize {
        TABLE_OFFSET
            + self.members.len() * ENTRY_BYTES
            + index * self.dimensions as usize * element_size(self.encoding).unwrap_or(0)
    }

    /// Total serialized size in bytes.
    pub fn total_bytes(&self) -> usize {
        let element = element_size(self.encoding).unwrap_or(0);
        let vectors = self
            .members
            .iter()
            .filter(|m| m.status == STATUS_OK)
            .count()
            * self.dimensions as usize
            * element;
        TABLE_OFFSET + self.members.len() * ENTRY_BYTES + vectors
    }

    /// Members that carry a vector, in canonical order.
    pub fn contributors(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.status == STATUS_OK)
    }
}

/// A validation failure: a stable code plus a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocError {
    /// Stable machine-readable code (spec §10).
    pub code: &'static str,
    /// Where and what, for a human reading a report.
    pub detail: String,
}

impl DocError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.code, self.detail)
    }
}

impl std::error::Error for DocError {}

// ---------------------------------------------------------------------------
// Validation — normative order (spec §10.1)
// ---------------------------------------------------------------------------

/// Validate a **logical** document (what a writer is about to emit).
///
/// A writer MUST call this before serializing, so an invalid document cannot be
/// produced by the reference encoder. It reports the first violation in the
/// normative check order.
pub fn validate_logical(doc: &Document) -> Result<(), DocError> {
    if doc.major != FORMAT_MAJOR {
        return Err(DocError::new(
            "unsupported_version",
            format!("major {} (reader implements {FORMAT_MAJOR})", doc.major),
        ));
    }
    if doc.minor != FORMAT_MINOR {
        return Err(DocError::new(
            "unsupported_version",
            format!("minor {} (reader implements {FORMAT_MINOR})", doc.minor),
        ));
    }
    if doc.flags != 0 {
        return Err(DocError::new(
            "reserved_nonzero",
            format!("flags=0x{:02x}", doc.flags),
        ));
    }
    if doc.fingerprint == [0u8; 32] {
        return Err(DocError::new("zero_fingerprint", "all 32 bytes are zero"));
    }
    if doc.dimensions == 0 {
        return Err(DocError::new("dimensions_zero", "dimensions=0"));
    }
    if doc.dimensions > MAX_DIMENSIONS {
        return Err(DocError::new(
            "dimensions_too_large",
            format!("dimensions={} (max {MAX_DIMENSIONS})", doc.dimensions),
        ));
    }
    if element_size(doc.encoding).is_none() {
        return Err(DocError::new(
            "unsupported_encoding",
            format!("vector_encoding={} (undefined)", doc.encoding),
        ));
    }
    if doc.members.is_empty() {
        return Err(DocError::new("track_count_zero", "no members"));
    }
    if doc.members.len() as u32 > MAX_TRACKS {
        return Err(DocError::new(
            "track_count_too_large",
            format!("{} members (max {MAX_TRACKS})", doc.members.len()),
        ));
    }
    for (index, member) in doc.members.iter().enumerate() {
        if member.disc == 0 || member.track == 0 {
            return Err(DocError::new(
                "invalid_member",
                format!(
                    "entry[{index}] disc={} track={} (both must be >= 1)",
                    member.disc, member.track
                ),
            ));
        }
        if status_name(member.status).is_none() {
            return Err(DocError::new(
                "unknown_status",
                format!(
                    "entry[{index}] status={} (max defined {STATUS_MAX})",
                    member.status
                ),
            ));
        }
        if member.status == STATUS_OK {
            let Some(vector) = &member.vector else {
                return Err(DocError::new(
                    "vector_missing_for_ok",
                    format!("entry[{index}] status=ok without a vector"),
                ));
            };
            if vector.len() != doc.dimensions as usize {
                return Err(DocError::new(
                    "vector_length_invalid",
                    format!(
                        "entry[{index}] has {} values, dimensions={}",
                        vector.len(),
                        doc.dimensions
                    ),
                ));
            }
            for (position, value) in vector.iter().enumerate() {
                if !value.is_finite() {
                    return Err(DocError::new(
                        "non_finite_vector",
                        format!("entry[{index}] element {position} is not finite"),
                    ));
                }
                if doc.encoding == ENCODING_F16LE && f32_to_f16_bits(*value).is_none() {
                    return Err(DocError::new(
                        "vector_element_not_representable",
                        format!("entry[{index}] element {position} overflows binary16"),
                    ));
                }
            }
        } else if member.vector.is_some() {
            return Err(DocError::new(
                "vector_present_for_non_ok",
                format!(
                    "entry[{index}] status={} carries a vector",
                    status_name(member.status).unwrap_or("?")
                ),
            ));
        }
    }
    for pair in doc.members.windows(2) {
        if (pair[0].disc, pair[0].track) == (pair[1].disc, pair[1].track) {
            return Err(DocError::new(
                "duplicate_member",
                format!(
                    "disc={} track={} appears twice",
                    pair[0].disc, pair[0].track
                ),
            ));
        }
    }
    for pair in doc.members.windows(2) {
        if (pair[0].disc, pair[0].track) >= (pair[1].disc, pair[1].track) {
            return Err(DocError::new(
                "unsorted_table",
                format!(
                    "disc={} track={} precedes disc={} track={}",
                    pair[0].disc, pair[0].track, pair[1].disc, pair[1].track
                ),
            ));
        }
    }
    Ok(())
}

/// Serialize a logical document.
///
/// Deterministic by construction: no timestamps, no paths, no iteration over a
/// hash map, no padding whose content is undefined. The same logical value
/// always produces the same bytes.
pub fn encode(doc: &Document) -> Result<Vec<u8>, DocError> {
    validate_logical(doc)?;
    let element = element_size(doc.encoding).unwrap_or(0);
    let mut out = vec![0u8; doc.total_bytes()];
    out[0..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&doc.major.to_be_bytes());
    out[6..8].copy_from_slice(&doc.minor.to_be_bytes());
    out[8..40].copy_from_slice(&doc.fingerprint);
    out[40..42].copy_from_slice(&(doc.dimensions as u16).to_be_bytes());
    out[42] = doc.encoding;
    out[43] = doc.flags;
    out[44..48].copy_from_slice(&(doc.members.len() as u32).to_be_bytes());
    // out[48..64] stays zero: the reserved region.
    for (index, member) in doc.members.iter().enumerate() {
        let base = TABLE_OFFSET + index * ENTRY_BYTES;
        out[base..base + 4].copy_from_slice(&member.disc.to_be_bytes());
        out[base + 4..base + 8].copy_from_slice(&member.track.to_be_bytes());
        out[base + 8] = member.status;
        // out[base + 9..base + 12] stays zero: the reserved region.
        if member.status == STATUS_OK {
            let start = doc.vector_offset(index);
            let vector = member.vector.as_ref().expect("validated above");
            for (position, value) in vector.iter().enumerate() {
                let at = start + position * element;
                match doc.encoding {
                    ENCODING_F32LE => out[at..at + 4].copy_from_slice(&value.to_le_bytes()),
                    ENCODING_F16LE => {
                        let bits = f32_to_f16_bits(*value).expect("validated above");
                        out[at..at + 2].copy_from_slice(&bits.to_le_bytes());
                    }
                    _ => unreachable!("validated above"),
                }
            }
        }
    }
    Ok(out)
}

/// Parse and validate a document, in the normative check order.
///
/// The order is load-bearing: it is what makes "the expected validation
/// result" in the fixture manifest a single, stable answer, and it guarantees no
/// allocation is sized from an unvalidated field. Steps 1–9 use only the header
/// and the table; the vector buffer is not allocated until step 10 has proved
/// the declared size fits inside the actual file.
pub fn validate(bytes: &[u8]) -> Result<Document, DocError> {
    // 1. header present
    if bytes.len() < HEADER_BYTES {
        return Err(DocError::new(
            "truncated_header",
            format!("{} bytes, need at least {HEADER_BYTES}", bytes.len()),
        ));
    }
    // 2. magic
    if bytes[0..4] != MAGIC {
        return Err(DocError::new(
            "magic_invalid",
            format!("{:02x?}", &bytes[0..4]),
        ));
    }
    // 3. version
    let major = u16::from_be_bytes([bytes[4], bytes[5]]);
    let minor = u16::from_be_bytes([bytes[6], bytes[7]]);
    if major != FORMAT_MAJOR {
        return Err(DocError::new(
            "unsupported_version",
            format!("format_major={major} (reader implements {FORMAT_MAJOR})"),
        ));
    }
    if minor > FORMAT_MINOR {
        return Err(DocError::new(
            "unsupported_version",
            format!("format_minor={minor} (reader implements up to {FORMAT_MINOR})"),
        ));
    }
    // 4. reserved
    let flags = bytes[43];
    if flags != 0 {
        return Err(DocError::new(
            "reserved_nonzero",
            format!("header.flags=0x{flags:02x}"),
        ));
    }
    if let Some(at) = bytes[48..HEADER_BYTES].iter().position(|b| *b != 0) {
        return Err(DocError::new(
            "reserved_nonzero",
            format!("header.reserved byte {}", 48 + at),
        ));
    }
    // 5. fingerprint
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&bytes[8..40]);
    if fingerprint == [0u8; 32] {
        return Err(DocError::new("zero_fingerprint", "all 32 bytes are zero"));
    }
    // 6. dimensions
    let dimensions = u16::from_be_bytes([bytes[40], bytes[41]]);
    if dimensions == 0 {
        return Err(DocError::new("dimensions_zero", "dimensions=0"));
    }
    if u32::from(dimensions) > MAX_DIMENSIONS {
        return Err(DocError::new(
            "dimensions_too_large",
            format!("dimensions={dimensions} (max {MAX_DIMENSIONS})"),
        ));
    }
    // 7. encoding
    let encoding = bytes[42];
    let Some(element) = element_size(encoding) else {
        return Err(DocError::new(
            "unsupported_encoding",
            format!("vector_encoding={encoding} (undefined; reader implements 1, 2)"),
        ));
    };
    // 8. track count
    let track_count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]);
    if track_count == 0 {
        return Err(DocError::new("track_count_zero", "track_count=0"));
    }
    if track_count > MAX_TRACKS {
        return Err(DocError::new(
            "track_count_too_large",
            format!("track_count={track_count} (max {MAX_TRACKS})"),
        ));
    }
    // 9. track table
    let table_bytes = track_count as usize * ENTRY_BYTES;
    let table_end = TABLE_OFFSET + table_bytes;
    if bytes.len() < table_end {
        return Err(DocError::new(
            "truncated_table",
            format!(
                "{} bytes, table for {track_count} members ends at {table_end}",
                bytes.len()
            ),
        ));
    }
    let mut members = Vec::with_capacity(track_count as usize);
    for index in 0..track_count as usize {
        let base = TABLE_OFFSET + index * ENTRY_BYTES;
        if let Some(at) = bytes[base + 9..base + ENTRY_BYTES]
            .iter()
            .position(|b| *b != 0)
        {
            return Err(DocError::new(
                "reserved_nonzero",
                format!("entry[{index}].reserved byte {}", base + 9 + at),
            ));
        }
        let disc = u32::from_be_bytes([
            bytes[base],
            bytes[base + 1],
            bytes[base + 2],
            bytes[base + 3],
        ]);
        let track = u32::from_be_bytes([
            bytes[base + 4],
            bytes[base + 5],
            bytes[base + 6],
            bytes[base + 7],
        ]);
        let status = bytes[base + 8];
        if disc == 0 || track == 0 {
            return Err(DocError::new(
                "invalid_member",
                format!("entry[{index}] disc={disc} track={track} (both must be >= 1)"),
            ));
        }
        if status_name(status).is_none() {
            return Err(DocError::new(
                "unknown_status",
                format!("entry[{index}] status={status} (max defined {STATUS_MAX})"),
            ));
        }
        members.push(Member {
            disc,
            track,
            status,
            vector: None,
        });
    }
    for pair in members.windows(2) {
        if (pair[0].disc, pair[0].track) == (pair[1].disc, pair[1].track) {
            return Err(DocError::new(
                "duplicate_member",
                format!(
                    "disc={} track={} appears twice",
                    pair[0].disc, pair[0].track
                ),
            ));
        }
    }
    for pair in members.windows(2) {
        if (pair[0].disc, pair[0].track) >= (pair[1].disc, pair[1].track) {
            return Err(DocError::new(
                "unsorted_table",
                format!(
                    "disc={} track={} precedes disc={} track={}",
                    pair[0].disc, pair[0].track, pair[1].disc, pair[1].track
                ),
            ));
        }
    }
    // 10. declared size vs actual size
    let vector_bytes = dimensions as usize * element;
    let contributor_count = members.iter().filter(|m| m.status == STATUS_OK).count();
    let expected = table_end + contributor_count * vector_bytes;
    if bytes.len() < expected {
        return Err(DocError::new(
            "truncated",
            format!("{} bytes, declared layout needs {expected}", bytes.len()),
        ));
    }
    if bytes.len() > expected {
        return Err(DocError::new(
            "trailing_bytes",
            format!("{} bytes, declared layout needs {expected}", bytes.len()),
        ));
    }
    // 11. vectors
    for (index, member) in members.iter_mut().enumerate() {
        if member.status != STATUS_OK {
            continue;
        }
        let start = table_end + index * vector_bytes;
        let mut vector = Vec::with_capacity(dimensions as usize);
        for position in 0..dimensions as usize {
            let at = start + position * element;
            let value = match encoding {
                ENCODING_F32LE => {
                    f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
                }
                _ => f16_bits_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]])),
            };
            if !value.is_finite() {
                return Err(DocError::new(
                    "non_finite_vector",
                    format!("entry[{index}] element {position} is not finite"),
                ));
            }
            vector.push(value);
        }
        member.vector = Some(vector);
    }
    Ok(Document {
        major,
        minor,
        fingerprint,
        dimensions: u32::from(dimensions),
        encoding,
        flags,
        members,
    })
}

// ---------------------------------------------------------------------------
// Consumer admission — profile identity, not document structure
// ---------------------------------------------------------------------------

/// What a consumer decided about a structurally valid document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The document belongs to the partition the consumer is serving.
    Admitted,
    /// The document parses but names a different profile. Not an error: it is
    /// retained, ignored for queries, and reported (ADR 0017 §5.3).
    ProfileMismatch,
}

/// Decide whether a document may contribute to `serving`.
///
/// This is a byte comparison against bytes the caller already had. A consumer
/// that does not know any profile gets [`Admission::ProfileMismatch`] for every
/// document, which is the safe answer: never mix, never guess, never convert.
pub fn admit(doc: &Document, serving: &[u8; 32]) -> Admission {
    if doc.fingerprint == *serving {
        Admission::Admitted
    } else {
        Admission::ProfileMismatch
    }
}

// ---------------------------------------------------------------------------
// Package-coherence checks (need the manifest; the document alone cannot do it)
// ---------------------------------------------------------------------------

/// A manifest-side membership, as far as the document is concerned.
pub trait ManifestTracks {
    /// The package's `(disc, track)` pairs, in any order.
    fn pairs(&self) -> Vec<(u32, u32)>;
}

/// Check a document against the package that references it.
///
/// These are *not* document-structure checks: a document that fails them is
/// still a well-formed document, it just does not describe this package. The
/// document carries no manifest digest, so a reader without the manifest cannot
/// perform them.
pub fn validate_coherence(doc: &Document, manifest: &impl ManifestTracks) -> Result<(), DocError> {
    let expected = manifest.pairs();
    if expected.len() != doc.members.len() {
        return Err(DocError::new(
            "member_count_mismatch",
            format!(
                "document has {} members, package has {} tracks",
                doc.members.len(),
                expected.len()
            ),
        ));
    }
    for member in &doc.members {
        if !expected.contains(&(member.disc, member.track)) {
            return Err(DocError::new(
                "unknown_member",
                format!(
                    "disc={} track={} is not a track of this package",
                    member.disc, member.track
                ),
            ));
        }
    }
    for pair in &expected {
        if !doc.members.iter().any(|m| (m.disc, m.track) == *pair) {
            return Err(DocError::new(
                "missing_member",
                format!(
                    "package track disc={} track={} has no entry",
                    pair.0, pair.1
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// IEEE 754 binary16
// ---------------------------------------------------------------------------

/// Round an `f32` to the nearest binary16, returning its bit pattern.
///
/// `None` means "not representable": a non-finite input, or a magnitude that
/// would overflow to infinity. Ties round to even, subnormals are produced and
/// supported, and the smallest subnormal (`2^-24`) and the smallest normal
/// (`2^-14`) both fall out of the same code path.
///
/// Specified rather than borrowed from a language conversion so that two
/// independent writers produce identical bytes.
pub fn f32_to_f16_bits(value: f32) -> Option<u16> {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        return None; // NaN or infinity
    }
    if exponent == 0 {
        // Zero, or an f32 subnormal: far below the smallest binary16 subnormal.
        return Some(sign);
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return None; // overflows to infinity
    }
    if unbiased >= -14 {
        // Normal binary16: 10 explicit significand bits, round to nearest even.
        let truncated = mantissa >> 13;
        let remainder = mantissa & 0x1fff;
        let mut significand = truncated;
        if remainder > 0x1000 || (remainder == 0x1000 && (truncated & 1) == 1) {
            significand += 1;
        }
        let mut result_exponent = unbiased;
        if significand == 0x400 {
            significand = 0;
            result_exponent += 1;
            if result_exponent > 15 {
                return None;
            }
        }
        Some(sign | (((result_exponent + 15) as u16) << 10) | significand as u16)
    } else if unbiased >= -25 {
        // Subnormal binary16: k * 2^-24 for k in 0..=1024. k == 1024 is the
        // smallest normal, and its bit pattern is exactly the exponent field
        // below, so no special case is needed.
        let significand: u64 = (1u64 << 23) | u64::from(mantissa);
        let shift = (-(unbiased + 1)) as u32; // 14..=24
        let quotient = significand >> shift;
        let remainder = significand & ((1u64 << shift) - 1);
        let half = 1u64 << (shift - 1);
        let rounded = if remainder > half || (remainder == half && (quotient & 1) == 1) {
            quotient + 1
        } else {
            quotient
        };
        Some(sign | rounded as u16)
    } else {
        Some(sign) // below half of the smallest subnormal
    }
}

/// Widen a binary16 bit pattern to `f32`. Exact for every input, including
/// subnormals, infinities and NaN.
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = ((bits >> 10) & 0x1f) as u32;
    let mantissa = u32::from(bits & 0x03ff);
    let out = if exponent == 0 {
        if mantissa == 0 {
            sign
        } else {
            // Subnormal: renormalize.
            let mut value = mantissa;
            let mut shift = -1i32;
            while value & 0x400 == 0 {
                value <<= 1;
                shift += 1;
            }
            let exponent = (127 - 15 - shift) as u32;
            sign | (exponent << 23) | ((value & 0x3ff) << 13)
        }
    } else if exponent == 0x1f {
        sign | 0x7f80_0000 | (mantissa << 13)
    } else {
        sign | ((exponent + 127 - 15) << 23) | (mantissa << 13)
    };
    f32::from_bits(out)
}

// ---------------------------------------------------------------------------
// Profile identity (spec appendix A)
// ---------------------------------------------------------------------------

/// The profile-defining fields of ADR 0017 §5.4, in tag order.
///
/// This is the *registry* view. The document carries only the resulting 32 bytes;
/// a document reader cannot and does not check that a fingerprint corresponds
/// to any field list, because that would require the profile definition to be
/// part of the document.
#[derive(Debug, Clone)]
pub struct ProfileFields {
    /// Tag 1.
    pub profile_id: String,
    /// Tag 2.
    pub model_family: String,
    /// Tag 3.
    pub model_variant: String,
    /// Tag 4. Absent (not emitted) for a model-free profile.
    pub model_sha256: Option<[u8; 32]>,
    /// Tag 5.
    pub preprocessing_version: String,
    /// Tag 6.
    pub patch_hop: u32,
    /// Tag 7.
    pub pooling: String,
    /// Tag 8.
    pub normalization: String,
    /// Tag 9.
    pub metric: String,
    /// Tag 10.
    pub dimensions: u32,
    /// Tag 11.
    pub output_encoding: String,
    /// Tag 12. Absent in v1.0: the document has no album aggregate.
    pub album_aggregation: Option<String>,
    /// Tag 13, first half: the inference runtime identity. Absent for a
    /// model-free profile that runs no runtime.
    pub runtime_id: Option<String>,
    /// Tag 13, second half: the runtime version. Never emitted alone.
    pub runtime_version: Option<String>,
    /// Tag 14.
    pub numeric_policy: String,
}

/// The canonical tagged encoding: `tag:u8 || len:u32be || bytes`, tags in
/// ascending order, absent fields emitting nothing.
///
/// This is the `src/identity.rs:41-89` TLV precedent with one addition the
/// precedent does not need: an integer field is four big-endian bytes.
pub fn profile_tlv(fields: &ProfileFields) -> Vec<u8> {
    fn push(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
        out.push(tag);
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
    }
    let mut out = Vec::new();
    push(&mut out, 1, fields.profile_id.as_bytes());
    push(&mut out, 2, fields.model_family.as_bytes());
    push(&mut out, 3, fields.model_variant.as_bytes());
    if let Some(digest) = fields.model_sha256 {
        push(&mut out, 4, &digest);
    }
    push(&mut out, 5, fields.preprocessing_version.as_bytes());
    push(&mut out, 6, &fields.patch_hop.to_be_bytes());
    push(&mut out, 7, fields.pooling.as_bytes());
    push(&mut out, 8, fields.normalization.as_bytes());
    push(&mut out, 9, fields.metric.as_bytes());
    push(&mut out, 10, &fields.dimensions.to_be_bytes());
    push(&mut out, 11, fields.output_encoding.as_bytes());
    if let Some(rule) = &fields.album_aggregation {
        push(&mut out, 12, rule.as_bytes());
    }
    // Half a tag 13 is a registry authoring error, not a document concern;
    // emitting nothing keeps the encoding total.
    if let (Some(runtime), Some(version)) = (&fields.runtime_id, &fields.runtime_version) {
        let mut value = runtime.clone();
        value.push('\0');
        value.push_str(version);
        push(&mut out, 13, value.as_bytes());
    }
    push(&mut out, 14, fields.numeric_policy.as_bytes());
    out
}

/// SHA-256 over [`profile_tlv`].
pub fn profile_fingerprint(fields: &ProfileFields) -> [u8; 32] {
    let tlv = profile_tlv(fields);
    let mut hasher = Sha256::new();
    hasher.update(&tlv);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

// ---------------------------------------------------------------------------
// Hashing / formatting helpers
// ---------------------------------------------------------------------------

/// Lowercase hex of arbitrary bytes. This is an *encoding*, not a digest.
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hex SHA-256 of arbitrary bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex_encode(&digest)
}

/// Lowercase hex of a profile fingerprint. The fingerprint *is* a digest; this
/// only renders it, so it MUST NOT hash anything again.
pub fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    hex_encode(fingerprint)
}

/// A `0x`-free, space-separated list of bytes, for dumps.
pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse lowercase hex into bytes. Test/dump helper.
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    // `as_chunks` (not `chunks_exact`): the length is already a multiple of 2,
    // so the remainder is always empty. The newer stable clippy lint
    // `chunks_exact_to_as_chunks` requires this form.
    for pair in bytes.as_chunks::<2>().0 {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}
