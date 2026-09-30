//! Production reader for MusicPack similarity documents (`.msim` v1).
//!
//! The normative specification is
//! `experiments/music-similarity-eval/FORMAT_SPEC.md`; this module implements
//! its document validation (§10) and vector decoding (§5) for the server's
//! indexing path (ADR 0017 §5.7). It is deliberately independent of the
//! experiment's reference codec: no shared code, same specification.
//!
//! Scope, stated plainly:
//!
//! - Structural validation follows the normative check order (§10.1) with the
//!   §10.2 codes, so a malformed document is identified, never guessed at.
//! - Vector decoding is implemented for `f32le` only. `f16le` documents parse
//!   structurally (the encoding is assigned, and finiteness of stored
//!   elements is still checked) but are never decoded for retrieval: G-6
//!   closed as KEEP F32LE, so the indexer refuses `f16le` for query use.
//! - Profile questions (§10.5: unknown fingerprint, wrong dimensions for the
//!   profile, missing L2 normalisation, zero-norm vectors) are **not** format
//!   errors. The reader reports structure; the indexing layer decides what a
//!   profile means. The single exception the specification assigns to the
//!   consumer also lives in the consumer: zero-norm `ok` vectors are decoded
//!   here and refused at indexing time (FORMAT_SPEC §6.2).
//! - No allocation is sized from an unvalidated field: the declared layout is
//!   proved against the actual byte count (step 10) before any vector is
//!   decoded (step 11). All arithmetic is checked.

use std::fmt;

/// Fixed header size in bytes (FORMAT_SPEC §5.1).
pub const HEADER_LEN: usize = 64;
/// Track-table entry size in bytes (FORMAT_SPEC §5.2).
pub const ENTRY_LEN: usize = 12;
/// Document magic `"MSIM"` (FORMAT_SPEC §5.1, bytes 0..4).
pub const MAGIC: [u8; 4] = *b"MSIM";
/// Accepted format major version (FORMAT_SPEC §5.1, §12.4 rule 1).
pub const FORMAT_MAJOR: u16 = 1;
/// Highest accepted format minor version (FORMAT_SPEC §5.1, §12.4 rule 1).
pub const FORMAT_MINOR_MAX: u16 = 0;
/// Format validation limit on dimensions (FORMAT_SPEC §11, decision C).
pub const MAX_DIMENSIONS: u16 = 4096;
/// Format limit on track count: 32 discs × 512 tracks (FORMAT_SPEC §11).
pub const MAX_TRACK_COUNT: u32 = 16384;
/// Registry value for `f32le` (FORMAT_SPEC §7.1).
pub const ENCODING_F32LE: u8 = 1;
/// Registry value for `f16le` (FORMAT_SPEC §7.1). Assigned, but not
/// implemented for retrieval: G-6 closed as KEEP F32LE.
pub const ENCODING_F16LE: u8 = 2;

/// Member status values (FORMAT_SPEC §6). The byte is the contract.
pub const STATUS_OK: u8 = 0;
pub const STATUS_INSUFFICIENT_AUDIO: u8 = 1;
pub const STATUS_UNSUPPORTED: u8 = 2;
pub const STATUS_FAILED: u8 = 3;

/// Human name for a member status byte. Names exist for logs; the byte is
/// the contract (FORMAT_SPEC §6).
pub fn status_name(status: u8) -> &'static str {
    match status {
        STATUS_OK => "ok",
        STATUS_INSUFFICIENT_AUDIO => "insufficient_audio",
        STATUS_UNSUPPORTED => "unsupported",
        STATUS_FAILED => "failed",
        _ => "unknown",
    }
}

/// A validation failure carrying its FORMAT_SPEC §10.2 code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocError {
    /// One of the §10.2 codes (`magic_invalid`, `duplicate_member`, …).
    pub code: &'static str,
    /// Human detail (offsets, counts); never a track name or path — the
    /// document carries none.
    pub detail: String,
}

impl DocError {
    fn new(code: &'static str, detail: String) -> Self {
        Self { code, detail }
    }
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for DocError {}

/// One track-table entry: the manifest's `(disc, track)` identity plus the
/// per-track status (FORMAT_SPEC §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    /// Manifest `media[].disc` (u32, ≥ 1).
    pub disc: u32,
    /// Manifest `media[].tracks[].track` (u32, ≥ 1).
    pub track: u32,
    /// Status byte 0..=3 (FORMAT_SPEC §6).
    pub status: u8,
}

/// A validated similarity document borrowing its bytes.
#[derive(Debug, Clone)]
pub struct Document<'a> {
    bytes: &'a [u8],
    fingerprint: [u8; 32],
    dimensions: u16,
    encoding: u8,
    members: Vec<Member>,
}

impl<'a> Document<'a> {
    /// Validates `bytes` in the normative order (FORMAT_SPEC §10.1) and
    /// returns the document on success.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, DocError> {
        // 1. header present.
        if bytes.len() < HEADER_LEN {
            return Err(DocError::new(
                "truncated_header",
                format!("{} bytes, need at least {HEADER_LEN}", bytes.len()),
            ));
        }
        // 2. magic.
        if bytes[0..4] != MAGIC {
            return Err(DocError::new(
                "magic_invalid",
                "bytes 0..4 are not MSIM".to_string(),
            ));
        }
        // 3. version: major, then minor.
        let major = u16::from_be_bytes([bytes[4], bytes[5]]);
        let minor = u16::from_be_bytes([bytes[6], bytes[7]]);
        if major != FORMAT_MAJOR || minor > FORMAT_MINOR_MAX {
            return Err(DocError::new(
                "unsupported_version",
                format!("format_major={major} format_minor={minor}"),
            ));
        }
        // 4. reserved: flags, then header reserved (per-entry reserved is
        // checked with the table in step 9).
        if bytes[43] != 0 {
            return Err(DocError::new(
                "reserved_nonzero",
                format!("header.flags={:#04x}", bytes[43]),
            ));
        }
        if bytes[48..64].iter().any(|b| *b != 0) {
            return Err(DocError::new(
                "reserved_nonzero",
                "header reserved region is not zero".to_string(),
            ));
        }
        // 5. profile fingerprint: not 32 zero bytes.
        let mut fingerprint = [0u8; 32];
        fingerprint.copy_from_slice(&bytes[8..40]);
        if fingerprint == [0u8; 32] {
            return Err(DocError::new(
                "zero_fingerprint",
                "profile_fingerprint is 32 zero bytes".to_string(),
            ));
        }
        // 6. dimensions: zero, then too large.
        let dimensions = u16::from_be_bytes([bytes[40], bytes[41]]);
        if dimensions == 0 {
            return Err(DocError::new(
                "dimensions_zero",
                "dimensions == 0".to_string(),
            ));
        }
        if dimensions > MAX_DIMENSIONS {
            return Err(DocError::new(
                "dimensions_too_large",
                format!("dimensions={dimensions} exceeds {MAX_DIMENSIONS}"),
            ));
        }
        // 7. vector encoding: assigned and implemented-or-recognised.
        let encoding = bytes[42];
        if encoding != ENCODING_F32LE && encoding != ENCODING_F16LE {
            return Err(DocError::new(
                "unsupported_encoding",
                format!("vector_encoding={encoding}"),
            ));
        }
        // 8. track count: zero, then too large.
        let track_count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]);
        if track_count == 0 {
            return Err(DocError::new(
                "track_count_zero",
                "track_count == 0".to_string(),
            ));
        }
        if track_count > MAX_TRACK_COUNT {
            return Err(DocError::new(
                "track_count_too_large",
                format!("track_count={track_count} exceeds {MAX_TRACK_COUNT}"),
            ));
        }
        // 9. track table: the bytes must at least cover it.
        let table_len = (track_count as u64)
            .checked_mul(ENTRY_LEN as u64)
            .expect("track_count is bounded, so the product cannot overflow");
        let table_end = (HEADER_LEN as u64)
            .checked_add(table_len)
            .expect("header plus bounded table cannot overflow");
        if (bytes.len() as u64) < table_end {
            return Err(DocError::new(
                "truncated_table",
                format!("bytes end inside the track table (need {table_end})"),
            ));
        }
        // 9a. per-entry pass: reserved, then member, then status.
        let mut members = Vec::with_capacity(track_count as usize);
        for i in 0..track_count as usize {
            let base = HEADER_LEN + i * ENTRY_LEN;
            if bytes[base + 9..base + 12].iter().any(|b| *b != 0) {
                return Err(DocError::new(
                    "reserved_nonzero",
                    format!("entry {i} reserved bytes are not zero"),
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
            if disc == 0 || track == 0 {
                return Err(DocError::new(
                    "invalid_member",
                    format!("entry {i} has disc={disc} track={track}"),
                ));
            }
            let status = bytes[base + 8];
            if status > STATUS_FAILED {
                return Err(DocError::new(
                    "unknown_status",
                    format!("entry {i} has status={status}"),
                ));
            }
            members.push(Member {
                disc,
                track,
                status,
            });
        }
        // 9b. duplicate pass, then ascending pass.
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                if members[i].disc == members[j].disc && members[i].track == members[j].track {
                    return Err(DocError::new(
                        "duplicate_member",
                        format!(
                            "disc={} track={} appears twice",
                            members[i].disc, members[i].track
                        ),
                    ));
                }
            }
        }
        for pair in members.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if (b.disc, b.track) <= (a.disc, a.track) {
                return Err(DocError::new(
                    "unsorted_table",
                    format!(
                        "entry (disc={} track={}) does not ascend after (disc={} track={})",
                        b.disc, b.track, a.disc, a.track
                    ),
                ));
            }
        }
        // 10. declared size: computed layout against the actual length. No
        // allocation has been sized from these fields until this check.
        let contributors = members.iter().filter(|m| m.status == STATUS_OK).count() as u64;
        let element_size: u64 = if encoding == ENCODING_F32LE { 4 } else { 2 };
        let declared = (HEADER_LEN as u64).checked_add(table_len).and_then(|v| {
            (contributors)
                .checked_mul(dimensions as u64)
                .and_then(|v2| v2.checked_mul(element_size))
                .and_then(|v3| v.checked_add(v3))
        });
        let Some(declared) = declared else {
            // Unreachable given the validated bounds, but a hostile document
            // must never reach an arithmetic panic: fail closed instead.
            return Err(DocError::new(
                "truncated",
                "declared layout does not fit addressable bytes".to_string(),
            ));
        };
        let actual = bytes.len() as u64;
        if actual < declared {
            return Err(DocError::new(
                "truncated",
                format!("{actual} bytes, declared layout needs {declared}"),
            ));
        }
        if actual > declared {
            return Err(DocError::new(
                "trailing_bytes",
                format!("{actual} bytes, declared layout needs {declared}"),
            ));
        }
        let doc = Self {
            bytes,
            fingerprint,
            dimensions,
            encoding,
            members,
        };
        // 11. vectors: every element of every `status = ok` vector finite.
        // Decoded eagerly so a corrupt payload is identified here, once.
        doc.check_vectors()?;
        Ok(doc)
    }

    /// The 32 raw profile-fingerprint bytes (FORMAT_SPEC §5.1, §8).
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Vector dimension count shared by every vector in the document.
    pub fn dimensions(&self) -> u16 {
        self.dimensions
    }

    /// Registry encoding value (`ENCODING_F32LE` or `ENCODING_F16LE`).
    pub fn encoding(&self) -> u8 {
        self.encoding
    }

    /// Track-table members in canonical ascending `(disc, track)` order.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// Number of members with `status = ok` (the vector contributors).
    pub fn contributor_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status == STATUS_OK)
            .count()
    }

    /// Decodes every contributor vector to `f32`.
    ///
    /// Only `f32le` documents are decodable for retrieval: `f16le` returns
    /// [`DocError`] `unsupported_encoding` (G-6 closed as KEEP F32LE).
    /// Returned in table order, parallel to `members()` with `None` for
    /// non-`ok` members (which occupy zero bytes — FORMAT_SPEC §5.3).
    pub fn decode_f32le(&self) -> Result<Vec<Option<Vec<f32>>>, DocError> {
        if self.encoding != ENCODING_F32LE {
            return Err(DocError::new(
                "unsupported_encoding",
                format!(
                    "vector_encoding={} is not implemented for retrieval",
                    self.encoding
                ),
            ));
        }
        let dim = self.dimensions as usize;
        let mut region = &self.bytes[HEADER_LEN + self.members.len() * ENTRY_LEN..];
        let mut out = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if member.status != STATUS_OK {
                out.push(None);
                continue;
            }
            let mut vector = Vec::with_capacity(dim);
            for _ in 0..dim {
                let element = f32::from_le_bytes([region[0], region[1], region[2], region[3]]);
                vector.push(element);
                region = &region[4..];
            }
            out.push(Some(vector));
        }
        Ok(out)
    }

    /// Step 11 of the normative order: every stored element finite.
    ///
    /// `f16le` elements are widened exactly for this check (subnormals,
    /// signed zeros and the normal range all widen exactly; only NaN and
    /// infinities fail). The widened values are never used for retrieval.
    fn check_vectors(&self) -> Result<(), DocError> {
        let dim = self.dimensions as usize;
        if self.encoding == ENCODING_F32LE {
            let mut region = &self.bytes[HEADER_LEN + self.members.len() * ENTRY_LEN..];
            for member in &self.members {
                if member.status != STATUS_OK {
                    continue;
                }
                for _ in 0..dim {
                    let element = f32::from_le_bytes([region[0], region[1], region[2], region[3]]);
                    if !element.is_finite() {
                        return Err(DocError::new(
                            "non_finite_vector",
                            format!(
                                "non-finite element in vector for disc={} track={}",
                                member.disc, member.track
                            ),
                        ));
                    }
                    region = &region[4..];
                }
            }
            return Ok(());
        }
        let mut region = &self.bytes[HEADER_LEN + self.members.len() * ENTRY_LEN..];
        for member in &self.members {
            if member.status != STATUS_OK {
                continue;
            }
            for _ in 0..dim {
                let bits = u16::from_le_bytes([region[0], region[1]]);
                if !f16_is_finite(bits) {
                    return Err(DocError::new(
                        "non_finite_vector",
                        format!(
                            "non-finite element in vector for disc={} track={}",
                            member.disc, member.track
                        ),
                    ));
                }
                region = &region[2..];
            }
        }
        Ok(())
    }
}

/// Whether a binary16 bit pattern is finite (zero, subnormal, and normal
/// values are; exponent `0x1f` — infinities and NaN — is not).
fn f16_is_finite(bits: u16) -> bool {
    (bits >> 10) & 0x1f != 0x1f
}

/// Lowercase hex of raw bytes (fingerprints on the wire and in the index).
pub fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a document with full control over every byte the validation
    /// order inspects. `statuses` drives the table; `vectors` supplies one
    /// f32 vector per `ok` member, in table order.
    fn build_doc(
        fingerprint: [u8; 32],
        dimensions: u16,
        encoding: u8,
        members: &[(u32, u32, u8)],
        vectors: &[Vec<f32>],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_MAJOR.to_be_bytes());
        out.extend_from_slice(&FORMAT_MINOR_MAX.to_be_bytes());
        out.extend_from_slice(&fingerprint);
        out.extend_from_slice(&dimensions.to_be_bytes());
        out.push(encoding);
        out.push(0u8); // flags: reserved, zero in v1.0
        out.extend_from_slice(&(members.len() as u32).to_be_bytes());
        out.extend_from_slice(&[0u8; 16]); // header reserved
        for (disc, track, status) in members {
            out.extend_from_slice(&disc.to_be_bytes());
            out.extend_from_slice(&track.to_be_bytes());
            out.push(*status);
            out.extend_from_slice(&[0u8; 3]); // entry reserved
        }
        let elem = if encoding == ENCODING_F32LE { 4 } else { 2 };
        let mut contributor = 0usize;
        for (disc, track, status) in members {
            let _ = (disc, track);
            if *status != STATUS_OK {
                continue;
            }
            let vector = &vectors[contributor];
            contributor += 1;
            assert_eq!(vector.len(), dimensions as usize);
            for value in vector {
                if elem == 4 {
                    out.extend_from_slice(&value.to_le_bytes());
                } else {
                    out.extend_from_slice(&[0u8; 2]); // f16 payload unused here
                }
            }
        }
        out
    }

    fn fingerprint(fill: u8) -> [u8; 32] {
        [fill; 32]
    }

    #[test]
    fn valid_minimal_document_parses() {
        let bytes = build_doc(
            fingerprint(0x51),
            4,
            ENCODING_F32LE,
            &[(1, 1, 0)],
            &[vec![1.0, 0.0, 0.0, 0.0]],
        );
        let doc = Document::parse(&bytes).expect("valid");
        assert_eq!(doc.fingerprint(), fingerprint(0x51));
        assert_eq!(doc.dimensions(), 4);
        assert_eq!(doc.encoding(), ENCODING_F32LE);
        assert_eq!(
            doc.members(),
            &[Member {
                disc: 1,
                track: 1,
                status: STATUS_OK
            }]
        );
        assert_eq!(doc.contributor_count(), 1);
        let decoded = doc.decode_f32le().expect("decodable");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].as_deref(), Some([1.0, 0.0, 0.0, 0.0].as_slice()));
    }

    #[test]
    fn non_ok_members_carry_no_bytes_and_decode_to_none() {
        let bytes = build_doc(
            fingerprint(0x52),
            2,
            ENCODING_F32LE,
            &[(1, 1, 1), (1, 2, 0), (1, 3, 3)],
            &[vec![0.5, -0.5]],
        );
        let doc = Document::parse(&bytes).expect("valid");
        assert_eq!(doc.contributor_count(), 1);
        let decoded = doc.decode_f32le().expect("decodable");
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0], None);
        assert_eq!(decoded[1].as_deref(), Some([0.5, -0.5].as_slice()));
        assert_eq!(decoded[2], None);
        assert_eq!(status_name(1), "insufficient_audio");
        assert_eq!(status_name(3), "failed");
    }

    #[test]
    fn committed_experiment_fixture_parses() {
        // The signed-off `minimal-ok.msim` fixture, embedded as committed
        // bytes (not regenerated, not read across crate boundaries): one
        // member, one 4-D f32 vector, profile
        // `musicpack-similarity-fixture-v1`. Embedded rather than
        // `include_bytes!` so this crate stays self-contained for packaging.
        let bytes: [u8; 92] = [
            0x4d, 0x53, 0x49, 0x4d, 0x00, 0x01, 0x00, 0x00, 0x51, 0xd0, 0xd4, 0xb1, 0x99, 0x7b,
            0x75, 0x78, 0xfa, 0x29, 0x0d, 0xcc, 0x2b, 0x89, 0x85, 0x8f, 0x25, 0x5b, 0x44, 0x93,
            0x02, 0x22, 0x82, 0xa4, 0x37, 0x3b, 0x73, 0xca, 0x6c, 0x2c, 0xbe, 0xee, 0x00, 0x04,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let doc = Document::parse(&bytes).expect("committed fixture parses");
        assert_eq!(doc.dimensions(), 4);
        assert_eq!(doc.encoding(), ENCODING_F32LE);
        assert_eq!(doc.members().len(), 1);
        assert_eq!(
            hex_lower(&doc.fingerprint()),
            "51d0d4b1997b7578fa290dcc2b89858f255b4493022282a4373b73ca6c2cbeee"
        );
        let decoded = doc.decode_f32le().expect("decodable");
        assert_eq!(decoded[0].as_deref(), Some([1.0, 0.0, 0.0, 0.0].as_slice()));
    }

    #[test]
    fn each_validation_code_fires() {
        // (mutator, expected code), applied to an otherwise-valid document.
        type Mutator = Box<dyn Fn(&mut Vec<u8>)>;
        let valid = || {
            build_doc(
                fingerprint(0x53),
                2,
                ENCODING_F32LE,
                &[(1, 1, 0)],
                &[vec![0.25, 0.75]],
            )
        };
        let cases: Vec<(Mutator, &str)> = vec![
            (
                Box::new(|b: &mut Vec<u8>| b.truncate(40)),
                "truncated_header",
            ),
            (Box::new(|b: &mut Vec<u8>| b[0] = b'X'), "magic_invalid"),
            (Box::new(|b: &mut Vec<u8>| b[4] = 2), "unsupported_version"),
            (Box::new(|b: &mut Vec<u8>| b[6] = 1), "unsupported_version"),
            (Box::new(|b: &mut Vec<u8>| b[43] = 1), "reserved_nonzero"),
            (Box::new(|b: &mut Vec<u8>| b[60] = 1), "reserved_nonzero"),
            (Box::new(|b: &mut Vec<u8>| b[73] = 1), "reserved_nonzero"),
            (
                Box::new(|b: &mut Vec<u8>| b[8..40].fill(0)),
                "zero_fingerprint",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[40..42].copy_from_slice(&0u16.to_be_bytes())),
                "dimensions_zero",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[40..42].copy_from_slice(&4097u16.to_be_bytes())),
                "dimensions_too_large",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[42] = 9),
                "unsupported_encoding",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[44..48].copy_from_slice(&0u32.to_be_bytes())),
                "track_count_zero",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[44..48].copy_from_slice(&20000u32.to_be_bytes())),
                "track_count_too_large",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b.truncate(70)),
                "truncated_table",
            ),
            (
                Box::new(|b: &mut Vec<u8>| b[64..68].copy_from_slice(&0u32.to_be_bytes())),
                "invalid_member",
            ),
            (Box::new(|b: &mut Vec<u8>| b[72] = 9), "unknown_status"),
            (Box::new(|b: &mut Vec<u8>| b.truncate(80)), "truncated"),
            (Box::new(|b: &mut Vec<u8>| b.push(0)), "trailing_bytes"),
            (
                Box::new(|b: &mut Vec<u8>| b[76..80].copy_from_slice(&f32::INFINITY.to_le_bytes())),
                "non_finite_vector",
            ),
        ];
        for (mutate, code) in cases {
            let mut bytes = valid();
            mutate(&mut bytes);
            let err = Document::parse(&bytes).expect_err("must fail");
            assert_eq!(err.code, code, "wrong code");
        }
    }

    #[test]
    fn duplicate_and_unsorted_tables_rejected() {
        let dup = build_doc(
            fingerprint(0x54),
            1,
            ENCODING_F32LE,
            &[(1, 1, 0), (1, 1, 0)],
            &[vec![1.0], vec![0.0]],
        );
        assert_eq!(Document::parse(&dup).unwrap_err().code, "duplicate_member");
        let unsorted = build_doc(
            fingerprint(0x55),
            1,
            ENCODING_F32LE,
            &[(1, 2, 0), (1, 1, 0)],
            &[vec![1.0], vec![0.0]],
        );
        assert_eq!(
            Document::parse(&unsorted).unwrap_err().code,
            "unsorted_table"
        );
    }

    #[test]
    fn f16le_parses_structurally_but_does_not_decode() {
        let bytes = build_doc(
            fingerprint(0x56),
            2,
            ENCODING_F16LE,
            &[(1, 1, 0)],
            &[vec![0.0, 0.0]],
        );
        let doc = Document::parse(&bytes).expect("f16le is assigned, parses");
        assert_eq!(doc.encoding(), ENCODING_F16LE);
        let err = doc.decode_f32le().expect_err("not decodable for retrieval");
        assert_eq!(err.code, "unsupported_encoding");
    }

    #[test]
    fn f16_infinities_are_non_finite() {
        // Exponent 0x1f with zero mantissa: positive infinity in binary16.
        assert!(!f16_is_finite(0x7c00));
        assert!(!f16_is_finite(0xfc00));
        assert!(!f16_is_finite(0x7e00)); // NaN
        assert!(f16_is_finite(0x3c00)); // 1.0
        assert!(f16_is_finite(0x0000)); // +0
        assert!(f16_is_finite(0x0001)); // smallest subnormal
        assert!(f16_is_finite(0x7bff)); // largest finite
    }

    #[test]
    fn fingerprint_hex_is_lowercase() {
        assert_eq!(hex_lower(&[0xABu8, 0x01, 0xF0]), "ab01f0".to_string());
    }
}
