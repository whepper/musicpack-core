//! Model-neutral similarity producer boundary (Slice 0).
//!
//! This module connects the Author pipeline to the signed-off `.msim`
//! document format (FORMAT_SPEC.md in `experiments/music-similarity-eval/`)
//! without coupling MusicPack to any concrete model. It provides:
//!
//! - [`SimilarityProfile`]: the two-identifier profile definition
//!   (`profile_id` for display, `profile_fingerprint` as the partition
//!   key — ADR 0017 §5.4, never derived, never faked);
//! - [`SimilarityProducer`]: the trait a future concrete model implements.
//!   It receives staged audio through the existing safe decode boundary
//!   (`musicpack-core::audio`, no model runtime here) and returns
//!   deterministic per-track results;
//! - [`TrackSimilarity`]: the exact FORMAT_SPEC §6 status vocabulary
//!   (`ok`, `insufficient_audio`, `unsupported`, `failed`);
//! - [`write_msim`]: serialization of a profile plus per-track results
//!   into byte-exact v1 documents (ascending table, exact declared size);
//! - [`SimilarityCache`]: the cache boundary (keyed by audio identity plus
//!   profile fingerprint) with an in-memory implementation. Filesystem
//!   persistence belongs to the host, not to this crate.
//!
//! Deliberately absent: any concrete model, any ML runtime, any download,
//! any `f16` path (G-6 closed as KEEP F32LE — the encoding enum has exactly
//! one variant), any numerical threshold, any playlist/radio concept.

use std::collections::HashMap;
use std::path::Path;

use musicpack_core::format::checksum;

/// Format validation limit on dimensions (FORMAT_SPEC §11, decision C).
/// Duplicated here because the experiment crate is not a dependency; the
/// value is normative spec text, not a tuning choice.
pub const MAX_DIMENSIONS: u16 = 4096;
/// Format limit on track count: 32 discs × 512 tracks (FORMAT_SPEC §11).
pub const MAX_TRACK_COUNT: u32 = 16384;
/// Registry value for `f32le` (FORMAT_SPEC §7.1): the only implemented
/// vector encoding (G-6: KEEP F32LE).
pub const ENCODING_F32LE: u8 = 1;

// ---------------------------------------------------------------------
// profile identity
// ---------------------------------------------------------------------

/// Vector encoding. Exactly one variant exists: `f32le` is the indexed
/// representation (G-6 closed as KEEP F32LE). There is deliberately no
/// `f16le` variant — adding one would be a precision decision, not a
/// mechanical extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorEncoding {
    /// Little-endian IEEE-754 binary32: the indexed representation.
    F32Le,
}

impl VectorEncoding {
    /// Registry value (FORMAT_SPEC §7.1).
    pub const fn as_u8(self) -> u8 {
        match self {
            VectorEncoding::F32Le => ENCODING_F32LE,
        }
    }

    /// Registry name (FORMAT_SPEC §7.1).
    pub const fn name(self) -> &'static str {
        match self {
            VectorEncoding::F32Le => "f32le",
        }
    }

    /// Element size in bytes.
    pub const fn element_size(self) -> usize {
        match self {
            VectorEncoding::F32Le => 4,
        }
    }
}

/// What is wrong with a profile definition supplied to this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// Empty display id.
    EmptyId,
    /// All-zero fingerprint (FORMAT_SPEC rejects it in documents; it is
    /// refused here too, at the boundary).
    ZeroFingerprint,
    /// Dimension count outside `1 ..= 4096`.
    BadDimensions(u16),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::EmptyId => write!(f, "profile_id must not be empty"),
            ProfileError::ZeroFingerprint => {
                write!(f, "profile_fingerprint must not be 32 zero bytes")
            }
            ProfileError::BadDimensions(d) => {
                write!(f, "dimensions {d} outside 1..={MAX_DIMENSIONS}")
            }
        }
    }
}

impl std::error::Error for ProfileError {}

/// A similarity profile as the Author boundary knows it: the two
/// identifiers of ADR 0017 §5.4 plus the layout facts the `.msim` writer
/// needs. A future concrete producer supplies the exact fingerprint; this
/// type never derives or invents one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarityProfile {
    /// Stable, human-facing name (manifest `profile` display text; never a
    /// comparison, partition, or cache key).
    pub profile_id: String,
    /// Exact implementation identity: the SHA-256 over the canonical tagged
    /// field list (FORMAT_SPEC Appendix A). The partition and cache key.
    pub fingerprint: [u8; 32],
    /// Vector dimension count shared by every vector (`1 ..= 4096`).
    pub dimensions: u16,
    /// Vector encoding (always `F32Le` in Slice 0).
    pub encoding: VectorEncoding,
}

impl SimilarityProfile {
    /// Validates a caller-supplied profile definition. Fails closed on
    /// empty ids, zero fingerprints, and out-of-range dimensions.
    pub fn new(
        profile_id: String,
        fingerprint: [u8; 32],
        dimensions: u16,
    ) -> Result<Self, ProfileError> {
        if profile_id.is_empty() {
            return Err(ProfileError::EmptyId);
        }
        if fingerprint == [0u8; 32] {
            return Err(ProfileError::ZeroFingerprint);
        }
        if dimensions == 0 || dimensions > MAX_DIMENSIONS {
            return Err(ProfileError::BadDimensions(dimensions));
        }
        Ok(Self {
            profile_id,
            fingerprint,
            dimensions,
            encoding: VectorEncoding::F32Le,
        })
    }
}

// ---------------------------------------------------------------------
// producer abstraction
// ---------------------------------------------------------------------

/// Input to one producer call: track identity plus the staged audio path.
/// The producer decodes through `musicpack-core::audio` itself — the
/// existing safe boundary — so no decoded buffers cross this interface and
/// no model runtime leaks into the pipeline.
#[derive(Debug, Clone)]
pub struct ProducerInput<'a> {
    /// Manifest disc number (≥ 1).
    pub disc: i32,
    /// Manifest track number (≥ 1).
    pub track: i32,
    /// Staged audio file: the bytes the package actually ships, so the
    /// vector describes shipped content (ADR 0017 §5.6).
    pub audio_path: &'a Path,
}

/// One track's producer outcome, in exactly the FORMAT_SPEC §6 vocabulary.
/// No other states exist on disk; richer internal errors stay inside the
/// producer implementation.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSimilarity {
    /// The profile produced a vector for this track.
    Ok {
        /// Exactly `profile.dimensions` finite elements, non-zero norm
        /// (checked at the stage and writer boundaries, FORMAT_SPEC §6.2).
        vector: Vec<f32>,
    },
    /// The audio was too short or otherwise below the profile's minimum.
    /// A fact about the input, not a failure.
    InsufficientAudio,
    /// The input is outside what the profile accepts.
    Unsupported,
    /// Analysis was attempted and did not produce a result.
    Failed,
}

impl TrackSimilarity {
    /// The on-disk status byte (FORMAT_SPEC §6: the byte is the contract).
    pub const fn status_byte(&self) -> u8 {
        match self {
            TrackSimilarity::Ok { .. } => 0,
            TrackSimilarity::InsufficientAudio => 1,
            TrackSimilarity::Unsupported => 2,
            TrackSimilarity::Failed => 3,
        }
    }

    /// The vector for `Ok`, else `None` (non-`ok` members carry zero bytes —
    /// FORMAT_SPEC §6.1).
    pub fn vector(&self) -> Option<&[f32]> {
        match self {
            TrackSimilarity::Ok { vector } => Some(vector),
            _ => None,
        }
    }
}

/// Per-track analysis outcomes keyed by `(disc, track)`: the shared shape
/// returned by the analysis core, consumed by the `.msim` writer and the
/// split-stage entries.
pub type TrackResults = Vec<((i32, i32), TrackSimilarity)>;

/// A similarity producer: a future concrete model behind a model-neutral
/// interface. Implementations must be deterministic — identical input audio,
/// preprocessing configuration, implementation, and profile identity
/// produce identical vectors — and must never fabricate output for the
/// non-`ok` states. The interface names no model family, no runtime, and no
/// download mechanism.
pub trait SimilarityProducer {
    /// The exact profile this producer implements (identity + layout).
    fn profile(&self) -> &SimilarityProfile;

    /// Analyzes one track's staged audio. Per-track outcomes only; a
    /// producer-wide failure is reported per track as `Failed` so one bad
    /// track can never invalidate its neighbours' results or the package.
    fn analyze(&self, input: &ProducerInput<'_>) -> TrackSimilarity;
}

// ---------------------------------------------------------------------
// `.msim` writer (FORMAT_SPEC §5)
// ---------------------------------------------------------------------

/// What is wrong with a document the writer was asked to emit. These mirror
/// the writer-side codes (FORMAT_SPEC §10.3) so a producer bug fails loudly
/// instead of emitting bytes that mean what they do not mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsimWriteError {
    /// No members at all (`track_count == 0` is unrepresentable).
    EmptyDocument,
    /// A `(disc, track)` outside the manifest domain.
    InvalidMember {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
    },
    /// Two entries for one `(disc, track)`.
    DuplicateMember {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
    },
    /// An `ok` vector whose length differs from the profile dimensions.
    DimensionMismatch {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
        /// Profile-declared dimension count.
        expected: usize,
        /// Offered vector length.
        found: usize,
    },
    /// A non-finite element offered for writing (a producer bug, never
    /// clamped — FORMAT_SPEC §5.3).
    NonFiniteVector {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
    },
    /// An all-zero `ok` vector (consumer rule FORMAT_SPEC §6.2, enforced
    /// at the producing layer too: a zero vector would poison queries).
    ZeroNormVector {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
    },
    /// `ok` status with no vector, or a vector for a non-`ok` status
    /// (both unrepresentable — FORMAT_SPEC §6.1).
    InconsistentEntry {
        /// Manifest disc number.
        disc: i32,
        /// Manifest track number.
        track: i32,
    },
    /// More members than the format carries (`track_count ≤ 16384`).
    TooManyMembers {
        /// Offered member count.
        found: usize,
    },
}

impl std::fmt::Display for MsimWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsimWriteError::EmptyDocument => write!(f, "document has no members"),
            MsimWriteError::InvalidMember { disc, track } => {
                write!(f, "invalid member disc={disc} track={track}")
            }
            MsimWriteError::DuplicateMember { disc, track } => {
                write!(f, "duplicate member disc={disc} track={track}")
            }
            MsimWriteError::DimensionMismatch {
                disc,
                track,
                expected,
                found,
            } => write!(
                f,
                "disc={disc} track={track}: vector has {found} elements, profile declares {expected}"
            ),
            MsimWriteError::NonFiniteVector { disc, track } => {
                write!(f, "disc={disc} track={track}: non-finite vector element")
            }
            MsimWriteError::ZeroNormVector { disc, track } => {
                write!(f, "disc={disc} track={track}: zero-norm ok vector")
            }
            MsimWriteError::InconsistentEntry { disc, track } => {
                write!(f, "disc={disc} track={track}: ok/vector presence mismatch")
            }
            MsimWriteError::TooManyMembers { found } => {
                write!(
                    f,
                    "document has {found} members, format carries at most {MAX_TRACK_COUNT}"
                )
            }
        }
    }
}

impl std::error::Error for MsimWriteError {}

/// Serializes a profile plus per-track results into a byte-exact v1
/// document: 64-byte header, ascending 12-byte table, back-to-back `f32le`
/// vectors for `ok` members only. Deterministic: equal logical content
/// yields equal bytes (entries sorted, no timestamps, no paths).
pub fn write_msim(
    profile: &SimilarityProfile,
    entries: &[((i32, i32), TrackSimilarity)],
) -> Result<Vec<u8>, MsimWriteError> {
    if entries.is_empty() {
        return Err(MsimWriteError::EmptyDocument);
    }
    if entries.len() > MAX_TRACK_COUNT as usize {
        return Err(MsimWriteError::TooManyMembers {
            found: entries.len(),
        });
    }
    let dim = profile.dimensions as usize;
    // Validate, then sort into canonical order (FORMAT_SPEC §5.4).
    let mut ordered: Vec<(i32, i32, &TrackSimilarity)> = Vec::with_capacity(entries.len());
    for ((disc, track), result) in entries {
        if *disc < 1 || *track < 1 {
            return Err(MsimWriteError::InvalidMember {
                disc: *disc,
                track: *track,
            });
        }
        if let TrackSimilarity::Ok { vector } = result {
            if vector.len() != dim {
                return Err(MsimWriteError::DimensionMismatch {
                    disc: *disc,
                    track: *track,
                    expected: dim,
                    found: vector.len(),
                });
            }
            if vector.iter().any(|v| !v.is_finite()) {
                return Err(MsimWriteError::NonFiniteVector {
                    disc: *disc,
                    track: *track,
                });
            }
            if vector.iter().all(|v| *v == 0.0) {
                return Err(MsimWriteError::ZeroNormVector {
                    disc: *disc,
                    track: *track,
                });
            }
        }
        ordered.push((*disc, *track, result));
    }
    ordered.sort_unstable_by_key(|(disc, track, _)| (*disc, *track));
    for pair in ordered.windows(2) {
        if (pair[0].0, pair[0].1) == (pair[1].0, pair[1].1) {
            return Err(MsimWriteError::DuplicateMember {
                disc: pair[0].0,
                track: pair[0].1,
            });
        }
    }

    let elem = profile.encoding.element_size();
    let contributors = ordered
        .iter()
        .filter(|(_, _, r)| matches!(r, TrackSimilarity::Ok { .. }))
        .count();
    // Exact: members and dimensions are both bounded above, so plain
    // arithmetic cannot overflow (max ~256 MiB declared, matching the
    // format's own bound).
    let total: usize = 64 + 12 * ordered.len() + contributors * dim * elem;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"MSIM");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&profile.fingerprint);
    out.extend_from_slice(&profile.dimensions.to_be_bytes());
    out.push(profile.encoding.as_u8());
    out.push(0u8); // flags: reserved, zero in v1.0
    out.extend_from_slice(&(ordered.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0u8; 16]); // header reserved
    for (disc, track, result) in &ordered {
        out.extend_from_slice(&(*disc as u32).to_be_bytes());
        out.extend_from_slice(&(*track as u32).to_be_bytes());
        out.push(result.status_byte());
        out.extend_from_slice(&[0u8; 3]); // entry reserved
    }
    for (_, _, result) in &ordered {
        if let TrackSimilarity::Ok { vector } = result {
            for value in vector {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// Member status values (FORMAT_SPEC §6). The byte is the contract.
pub const STATUS_OK: u8 = 0;
/// Audio below the profile minimum (a fact about the input, not a failure).
pub const STATUS_INSUFFICIENT_AUDIO: u8 = 1;
/// Input outside what the profile accepts.
pub const STATUS_UNSUPPORTED: u8 = 2;
/// Analysis attempted without producing a result.
pub const STATUS_FAILED: u8 = 3;

/// Human name for a member status byte (FORMAT_SPEC §6). Names exist for
/// logs and previews; the byte is the contract.
pub fn status_name(status: u8) -> &'static str {
    match status {
        STATUS_OK => "ok",
        STATUS_INSUFFICIENT_AUDIO => "insufficient_audio",
        STATUS_UNSUPPORTED => "unsupported",
        STATUS_FAILED => "failed",
        _ => "unknown",
    }
}

/// Lowercase hex of a fingerprint (logs, previews, directory names).
/// Display only — never an identity input.
pub fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in fingerprint {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Deterministic filename slug for a profile id: ASCII alphanumerics plus
/// `-_.` survive, everything else becomes `_`, empty becomes `"profile"`.
/// Display-derived only; identity always comes from the fingerprint.
pub fn profile_slug(profile_id: &str) -> String {
    let mut slug: String = profile_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if slug.is_empty() {
        slug.push_str("profile");
    }
    slug
}

// ---------------------------------------------------------------------
// cache boundary
// ---------------------------------------------------------------------

/// Cache identity: analyzed-audio bytes plus profile fingerprint.
///
/// The fingerprint already binds preprocessing identity, dimensions, and
/// encoding (ADR 0017 fingerprint tags 5, 10, 11), so no separate fields
/// are needed — and `profile_id` alone must never key the cache (two
/// profiles can share a family name with zero byte-identical vectors).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Lowercase hex SHA-256 of the analyzed audio bytes.
    pub source_sha256: String,
    /// Exact profile fingerprint.
    pub fingerprint: [u8; 32],
}
impl CacheKey {
    /// Deterministic storage name: hex digest over the source digest text
    /// plus the raw fingerprint bytes. Opaque, filesystem-safe, fixed
    /// length; it carries no meaning beyond identity.
    pub fn storage_name(&self) -> String {
        let mut input = Vec::with_capacity(self.source_sha256.len() + 32);
        input.extend_from_slice(self.source_sha256.as_bytes());
        input.extend_from_slice(&self.fingerprint);
        checksum::sha256_hex(&input)
    }
}

/// A cached successful vector with the shape needed to revalidate it.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedVector {
    /// Profile dimension count the vector was stored under.
    pub dimensions: u16,
    /// The `ok` vector itself.
    pub vector: Vec<f32>,
}

/// The similarity cache boundary. Persistence lives with the host (which
/// owns the directory, the bounds, and the deletion UX per ADR 0016 §13);
/// this crate defines the contract plus an in-memory implementation for
/// single-run reuse and tests.
///
/// Only successful vectors are cached: a transient producer failure must
/// never become a permanent row. Invalidation is explicit
/// ([`SimilarityCache::invalidate_source`], [`SimilarityCache::clear`]) and
/// fail-closed — a missing or invalid entry is a normal miss, never an
/// error.
pub trait SimilarityCache {
    /// Looks up one cached vector. `None` is a normal miss.
    fn lookup(&self, key: &CacheKey) -> Option<CachedVector>;
    /// Stores one successful vector, replacing any prior row for the key.
    /// Invalid values (wrong length, non-finite) are refused silently:
    /// refusing to store is always safe, storing garbage never is.
    fn store(&mut self, key: &CacheKey, value: CachedVector);
    /// Drops every row derived from one source audio digest (the deletion
    /// policy hook: removing a source offers to remove its rows).
    fn invalidate_source(&mut self, source_sha256: &str);
    /// Drops everything.
    fn clear(&mut self);
}

/// In-memory cache: single-run reuse and tests. No persistence, no bounds
/// (a bounded persistent cache is host-owned; see the trait docs).
#[derive(Debug, Default)]
pub struct MemSimilarityCache {
    rows: HashMap<String, (CacheKey, CachedVector)>,
}

impl MemSimilarityCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Row count (tests and host bounding).
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether no row is stored.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl SimilarityCache for MemSimilarityCache {
    fn lookup(&self, key: &CacheKey) -> Option<CachedVector> {
        let (stored_key, value) = self.rows.get(&key.storage_name())?;
        if stored_key != key {
            return None;
        }
        if value.vector.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(value.clone())
    }

    fn store(&mut self, key: &CacheKey, value: CachedVector) {
        if value.dimensions as usize != value.vector.len() {
            return;
        }
        if value.vector.iter().any(|v| !v.is_finite()) {
            return;
        }
        if value.vector.iter().all(|v| *v == 0.0) {
            return;
        }
        self.rows.insert(key.storage_name(), (key.clone(), value));
    }

    fn invalidate_source(&mut self, source_sha256: &str) {
        self.rows
            .retain(|_, (key, _)| key.source_sha256 != source_sha256);
    }

    fn clear(&mut self) {
        self.rows.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SimilarityProfile {
        SimilarityProfile::new("test-synthetic-v1".to_string(), [0x5a; 32], 4).unwrap()
    }

    #[test]
    fn profile_rejects_bad_definitions() {
        assert_eq!(
            SimilarityProfile::new(String::new(), [0x5a; 32], 4).unwrap_err(),
            ProfileError::EmptyId
        );
        assert_eq!(
            SimilarityProfile::new("x".to_string(), [0u8; 32], 4).unwrap_err(),
            ProfileError::ZeroFingerprint
        );
        assert_eq!(
            SimilarityProfile::new("x".to_string(), [0x5a; 32], 0).unwrap_err(),
            ProfileError::BadDimensions(0)
        );
        assert_eq!(
            SimilarityProfile::new("x".to_string(), [0x5a; 32], 4097).unwrap_err(),
            ProfileError::BadDimensions(4097)
        );
        assert_eq!(VectorEncoding::F32Le.as_u8(), 1);
        assert_eq!(VectorEncoding::F32Le.name(), "f32le");
    }

    #[test]
    fn status_bytes_match_the_format_vocabulary() {
        assert_eq!(TrackSimilarity::Ok { vector: vec![1.0] }.status_byte(), 0);
        assert_eq!(TrackSimilarity::InsufficientAudio.status_byte(), 1);
        assert_eq!(TrackSimilarity::Unsupported.status_byte(), 2);
        assert_eq!(TrackSimilarity::Failed.status_byte(), 3);
    }

    #[test]
    fn writer_emits_the_signed_off_layout() {
        let entries = vec![
            (
                (1, 2),
                TrackSimilarity::Ok {
                    vector: vec![0.5, -0.5, 0.0, 1.0],
                },
            ),
            ((1, 1), TrackSimilarity::InsufficientAudio),
        ];
        let bytes = write_msim(&profile(), &entries).unwrap();
        // 64 header + 2×12 table + 1×4×4 vectors.
        assert_eq!(bytes.len(), 64 + 24 + 16);
        assert_eq!(&bytes[0..4], b"MSIM");
        assert_eq!(&bytes[8..40], &[0x5a; 32]);
        assert_eq!(u16::from_be_bytes([bytes[40], bytes[41]]), 4);
        assert_eq!(bytes[42], 1); // f32le
        assert_eq!(bytes[43], 0); // flags
        assert_eq!(
            u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]),
            2
        );
        // Table is ascending regardless of input order.
        assert_eq!(
            u32::from_be_bytes([bytes[64], bytes[65], bytes[66], bytes[67]]),
            1
        );
        assert_eq!(
            u32::from_be_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]),
            1
        );
        assert_eq!(bytes[72], 1); // insufficient_audio carries no bytes
        assert_eq!(
            u32::from_be_bytes([bytes[76], bytes[77], bytes[78], bytes[79]]),
            1
        );
        assert_eq!(
            u32::from_be_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]),
            2
        );
        assert_eq!(bytes[84], 0);
        // The ok vector follows, little-endian.
        assert_eq!(&bytes[88..92], &0.5f32.to_le_bytes());
    }

    #[test]
    fn writer_is_deterministic_and_rejects_bad_output() {
        let entries = vec![(
            (2, 3),
            TrackSimilarity::Ok {
                vector: vec![1.0, 0.0, 0.0, 0.0],
            },
        )];
        let a = write_msim(&profile(), &entries).unwrap();
        let b = write_msim(&profile(), &entries).unwrap();
        assert_eq!(a, b);
        assert!(matches!(
            write_msim(&profile(), &[]).unwrap_err(),
            MsimWriteError::EmptyDocument
        ));
        assert!(matches!(
            write_msim(
                &profile(),
                &[
                    ((0, 1), TrackSimilarity::Failed),
                    ((0, 1), TrackSimilarity::Failed)
                ]
            )
            .unwrap_err(),
            MsimWriteError::InvalidMember { .. } | MsimWriteError::DuplicateMember { .. }
        ));
        // Wrong length.
        assert!(matches!(
            write_msim(
                &profile(),
                &[((1, 1), TrackSimilarity::Ok { vector: vec![1.0] })]
            )
            .unwrap_err(),
            MsimWriteError::DimensionMismatch { .. }
        ));
        // Non-finite.
        assert!(matches!(
            write_msim(
                &profile(),
                &[(
                    (1, 1),
                    TrackSimilarity::Ok {
                        vector: vec![f32::NAN, 0.0, 0.0, 0.0]
                    }
                )]
            )
            .unwrap_err(),
            MsimWriteError::NonFiniteVector { .. }
        ));
        // All-zero ok.
        assert!(matches!(
            write_msim(
                &profile(),
                &[(
                    (1, 1),
                    TrackSimilarity::Ok {
                        vector: vec![0.0; 4]
                    }
                )]
            )
            .unwrap_err(),
            MsimWriteError::ZeroNormVector { .. }
        ));
    }

    #[test]
    fn slug_and_hex_are_deterministic_display() {
        assert_eq!(
            profile_slug("musicpack-similarity-fixture-v1"),
            "musicpack-similarity-fixture-v1"
        );
        assert_eq!(profile_slug("a/b@c d"), "a_b_c_d");
        assert_eq!(profile_slug(""), "profile");
        assert_eq!(fingerprint_hex(&[0xabu8; 32]), "ab".repeat(32));
    }

    #[test]
    fn cache_key_binds_audio_and_fingerprint() {
        let a = CacheKey {
            source_sha256: "aa".to_string(),
            fingerprint: [0x11; 32],
        };
        let b = CacheKey {
            source_sha256: "aa".to_string(),
            fingerprint: [0x22; 32],
        };
        let c = CacheKey {
            source_sha256: "bb".to_string(),
            fingerprint: [0x11; 32],
        };
        assert_ne!(a.storage_name(), b.storage_name());
        assert_ne!(a.storage_name(), c.storage_name());
        assert_eq!(a.storage_name(), a.storage_name());
        assert_eq!(a.storage_name().len(), 64);
    }

    #[test]
    fn mem_cache_hits_misses_and_fails_closed() {
        let mut cache = MemSimilarityCache::new();
        let key = CacheKey {
            source_sha256: "aa".to_string(),
            fingerprint: [0x11; 32],
        };
        assert_eq!(cache.lookup(&key), None);
        cache.store(
            &key,
            CachedVector {
                dimensions: 2,
                vector: vec![1.0, 0.0],
            },
        );
        assert_eq!(
            cache.lookup(&key),
            Some(CachedVector {
                dimensions: 2,
                vector: vec![1.0, 0.0]
            })
        );
        // Same audio, different fingerprint: miss.
        let other = CacheKey {
            source_sha256: "aa".to_string(),
            fingerprint: [0x22; 32],
        };
        assert_eq!(cache.lookup(&other), None);
        // Invalid stores are refused silently.
        cache.store(
            &other,
            CachedVector {
                dimensions: 2,
                vector: vec![1.0],
            },
        );
        cache.store(
            &other,
            CachedVector {
                dimensions: 2,
                vector: vec![f32::NAN, 0.0],
            },
        );
        cache.store(
            &other,
            CachedVector {
                dimensions: 2,
                vector: vec![0.0, 0.0],
            },
        );
        assert_eq!(cache.lookup(&other), None);
        // Invalidation drops only the named source.
        cache.invalidate_source("aa");
        assert_eq!(cache.lookup(&key), None);
        assert!(cache.is_empty());
        cache.store(
            &key,
            CachedVector {
                dimensions: 2,
                vector: vec![1.0, 0.0],
            },
        );
        cache.clear();
        assert!(cache.is_empty());
    }
}
