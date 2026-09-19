//! Differential-comparison primitives for the encoder compatibility oracle.
//!
//! The migration contract is that the Rust encoder reproduces the frozen
//! reference output. The full oracle (established in later phases) compares
//! streams at several levels; this module provides the pure, dependency-free
//! byte comparison every level is built on. It performs no I/O and does not
//! know how to find or run the C reference — locating the reference binary and
//! corpus is the job of the test harness, not the library.
//!
//! The comparison ladder, from coarsest to finest, is:
//!
//! | Level | Compared |
//! |---|---|
//! | [`ComparisonLevel::Container`] | SV8 block framing, stream header, metadata/gapless fields |
//! | [`ComparisonLevel::Frame`] | frame count, boundaries, coded field sequence |
//! | [`ComparisonLevel::Bitstream`] | exact whole-file/whole-payload bytes (and SHA-256) |
//! | [`ComparisonLevel::DecodedPcm`] | PCM decoded from both streams (the Rust decoder is the tool) |
//! | [`ComparisonLevel::Metadata`] | tags, replay gain, encoder info, gapless counts |
//!
//! Only [`compare_bytes`] is implemented here; the remaining levels plug into
//! it (and into the decoder) as the encoder stages land.

/// The level at which two encoder outputs are being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonLevel {
    /// Block framing and container-level fields.
    Container,
    /// Per-frame structure and coded field sequence.
    Frame,
    /// Exact bytes of a payload or complete stream.
    Bitstream,
    /// PCM decoded from both streams.
    DecodedPcm,
    /// Tags, replay gain, encoder info and gapless counts.
    Metadata,
}

/// The result of comparing two byte streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffReport {
    /// The level this comparison was made at.
    pub level: ComparisonLevel,
    /// Length of the left-hand stream in bytes.
    pub left_len: usize,
    /// Length of the right-hand stream in bytes.
    pub right_len: usize,
    /// Number of leading bytes the two streams share.
    pub equal_prefix_len: usize,
    /// Offset of the first differing byte, or the end of the shorter stream
    /// when one stream is a strict prefix of the other. `None` means the two
    /// byte streams are identical.
    pub first_difference: Option<usize>,
}

impl DiffReport {
    /// Returns `true` when the two streams are byte-for-byte identical.
    pub fn is_identical(&self) -> bool {
        self.first_difference.is_none()
    }
}

/// Compares two byte streams and reports the first difference, if any.
///
/// If one stream is a prefix of the other, the first difference is the offset
/// one past the shared prefix (the end of the shorter stream).
pub fn compare_bytes(level: ComparisonLevel, left: &[u8], right: &[u8]) -> DiffReport {
    let equal_prefix_len = left
        .iter()
        .zip(right.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let first_difference = if equal_prefix_len == left.len() && equal_prefix_len == right.len() {
        None
    } else {
        Some(equal_prefix_len)
    };
    DiffReport {
        level,
        left_len: left.len(),
        right_len: right.len(),
        equal_prefix_len,
        first_difference,
    }
}

#[cfg(test)]
mod tests {
    use super::{ComparisonLevel, compare_bytes};

    #[test]
    fn identical_streams_report_no_difference() {
        let report = compare_bytes(ComparisonLevel::Bitstream, b"MPCK\x08\x00", b"MPCK\x08\x00");
        assert!(report.is_identical());
        assert_eq!(report.first_difference, None);
        assert_eq!(report.equal_prefix_len, 6);
        assert_eq!(report.left_len, 6);
        assert_eq!(report.right_len, 6);
    }

    #[test]
    fn differing_byte_reports_its_offset() {
        let report = compare_bytes(ComparisonLevel::Container, b"AP\x00\x01", b"AP\x00\x02");
        assert!(!report.is_identical());
        assert_eq!(report.first_difference, Some(3));
        assert_eq!(report.equal_prefix_len, 3);
        assert_eq!(report.level, ComparisonLevel::Container);
    }

    #[test]
    fn first_byte_difference_is_offset_zero() {
        let report = compare_bytes(ComparisonLevel::Frame, b"\x00\x01", b"\xFF\x01");
        assert_eq!(report.first_difference, Some(0));
        assert_eq!(report.equal_prefix_len, 0);
    }

    #[test]
    fn shorter_prefix_reports_end_of_shared_prefix() {
        let report = compare_bytes(ComparisonLevel::Bitstream, b"MPCK", b"MPCK\x08");
        assert!(!report.is_identical());
        assert_eq!(report.first_difference, Some(4));
        assert_eq!(report.equal_prefix_len, 4);
        assert_eq!(report.left_len, 4);
        assert_eq!(report.right_len, 5);
    }

    #[test]
    fn empty_streams_are_identical() {
        let report = compare_bytes(ComparisonLevel::DecodedPcm, &[], &[]);
        assert!(report.is_identical());
        assert_eq!(report.equal_prefix_len, 0);
    }

    #[test]
    fn empty_versus_nonempty_differs_at_zero() {
        let report = compare_bytes(ComparisonLevel::Metadata, &[], &[0u8]);
        assert!(!report.is_identical());
        assert_eq!(report.first_difference, Some(0));
    }
}
