//! MSB-first bit writer for the Musepack SV8 encoder.
//!
//! Every SV8 field the encoder emits — block headers, stream info, seek
//! tables, resolution/scale-factor/Huffman-coded samples — is written through
//! this writer. It reproduces the reference bit layout (`writeBits` /
//! `emptyBits` in `codec/libmpcenc`): the **first bit written becomes the most
//! significant bit of the first output byte**, and fields are written high bit
//! first. Multi-byte numeric fields are therefore big-endian on the wire.
//!
//! The writer is deliberately small. It owns a byte buffer and exposes
//! bit-level operations plus explicit byte alignment; it does not know about
//! Musepack block framing, variable-length sizes or Huffman tables. Those are
//! documented in the crate `README.md` and implemented in later phases on top
//! of this primitive.
//!
//! ```
//! use musicpack_musepack_encoder::bitwriter::BitWriter;
//!
//! let mut w = BitWriter::new();
//! w.write_bits(0b101, 3);
//! w.write_bits(0b11010, 5);
//! assert_eq!(w.finish(), [0b1011_1010]);
//! ```
//!
//! # Determinism
//!
//! The writer has no ambient state, no allocation policy beyond `Vec`, and no
//! dependence on the platform: the same sequence of calls always yields the
//! same bytes.

/// A growable, MSB-first bit writer.
///
/// Bits are accumulated into a partially-filled byte. Complete bytes are
/// appended to an internal buffer as soon as eight bits are present; the
/// trailing partial byte is only materialised by [`BitWriter::align_to_byte`]
/// or [`BitWriter::finish`].
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    /// Complete output bytes, in write order.
    bytes: Vec<u8>,
    /// Bits of the current (not yet complete) byte, right-aligned: the first
    /// bit written sits at bit `partial_bits - 1`.
    partial: u8,
    /// Number of valid bits in `partial`, `0..8`.
    partial_bits: u8,
    /// Total bits written, including zero padding added by alignment.
    bit_len: u64,
}

impl BitWriter {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty writer with room for at least `bytes` complete bytes.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(bytes),
            partial: 0,
            partial_bits: 0,
            bit_len: 0,
        }
    }

    /// Writes a single bit (MSB-first).
    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        self.partial = (self.partial << 1) | u8::from(bit);
        self.partial_bits += 1;
        self.bit_len += 1;
        if self.partial_bits == 8 {
            self.bytes.push(self.partial);
            self.partial = 0;
            self.partial_bits = 0;
        }
    }

    /// Writes the low `count` bits of `value`, most significant bit first.
    ///
    /// `count` must be in `0..=32`. `count == 0` is a no-op. Bits of `value`
    /// above `count` are ignored, so `write_bits(0xFFFF, 4)` writes `0b1111`.
    ///
    /// # Panics
    ///
    /// Panics if `count > 32`. This is a programming error (a malformed field
    /// width), not untrusted input, so it is a hard failure rather than a
    /// silent truncation.
    #[inline]
    pub fn write_bits(&mut self, value: u32, count: u32) {
        assert!(
            count <= 32,
            "BitWriter fields are at most 32 bits wide, got {count}"
        );
        let mut remaining = count;
        while remaining > 0 {
            remaining -= 1;
            // `remaining` is < 32, so the shift is well defined.
            self.write_bit((value >> remaining) & 1 == 1);
        }
    }

    /// Returns the total number of bits written, including zero padding added
    /// by [`BitWriter::align_to_byte`].
    pub fn bit_len(&self) -> u64 {
        self.bit_len
    }

    /// Returns `true` when the next bit written starts a new byte.
    pub fn is_byte_aligned(&self) -> bool {
        self.partial_bits == 0
    }

    /// Pads the current partial byte with zero bits up to the next byte
    /// boundary. Does nothing when already aligned.
    pub fn align_to_byte(&mut self) {
        if self.partial_bits == 0 {
            return;
        }
        let shift = 8 - self.partial_bits;
        self.bytes.push(self.partial << shift);
        self.bit_len += u64::from(shift);
        self.partial = 0;
        self.partial_bits = 0;
    }

    /// Returns the complete bytes written so far.
    ///
    /// A trailing partial byte is **not** included; call
    /// [`BitWriter::align_to_byte`] or [`BitWriter::finish`] first when a
    /// zero-padded final byte is required (as when flushing a Musepack block).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the writer and returns the complete bytes written so far.
    ///
    /// Like [`BitWriter::as_bytes`], a trailing partial byte is dropped. Use
    /// [`BitWriter::finish`] to zero-pad it first.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Aligns to the next byte boundary (zero padding) and returns the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        self.align_to_byte();
        self.bytes
    }

    /// Discards all written bits and reuses the allocation.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.partial = 0;
        self.partial_bits = 0;
        self.bit_len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::BitWriter;

    #[test]
    fn empty_writer_has_no_bits_or_bytes() {
        let w = BitWriter::new();
        assert_eq!(w.bit_len(), 0);
        assert!(w.is_byte_aligned());
        assert!(w.as_bytes().is_empty());
        assert!(w.finish().is_empty());
    }

    #[test]
    fn with_capacity_behaves_like_new() {
        let mut w = BitWriter::with_capacity(64);
        assert_eq!(w.bit_len(), 0);
        assert!(w.as_bytes().is_empty());
        w.write_bits(0xAB, 8);
        assert_eq!(w.as_bytes(), [0xAB]);
    }

    #[test]
    fn single_bit_is_the_most_significant_bit() {
        let mut w = BitWriter::new();
        w.write_bit(true);
        assert_eq!(w.bit_len(), 1);
        assert!(!w.is_byte_aligned());
        assert!(w.as_bytes().is_empty(), "partial byte is not materialised");
        assert_eq!(w.finish(), [0b1000_0000]);
    }

    #[test]
    fn zero_bit_is_written_as_zero() {
        let mut w = BitWriter::new();
        w.write_bit(false);
        assert_eq!(w.finish(), [0x00]);
    }

    #[test]
    fn eight_bit_field_is_one_byte() {
        let mut w = BitWriter::new();
        w.write_bits(0xAB, 8);
        assert_eq!(w.bit_len(), 8);
        assert!(w.is_byte_aligned());
        assert_eq!(w.as_bytes(), [0xAB]);
    }

    #[test]
    fn fields_cross_a_byte_boundary_msb_first() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b11010, 5);
        assert_eq!(w.bit_len(), 8);
        assert_eq!(w.finish(), [0b1011_1010]);
    }

    #[test]
    fn twelve_bit_field_pads_the_low_bits() {
        let mut w = BitWriter::new();
        w.write_bits(0xABC, 12);
        assert_eq!(w.bit_len(), 12);
        assert!(!w.is_byte_aligned());
        assert_eq!(w.as_bytes(), [0xAB], "only the complete byte is visible");
        assert_eq!(w.finish(), [0xAB, 0xC0]);
    }

    #[test]
    fn thirty_two_bit_field_is_four_bytes_big_endian() {
        let mut w = BitWriter::new();
        w.write_bits(0xDEAD_BEEF, 32);
        assert_eq!(w.bit_len(), 32);
        assert_eq!(w.finish(), [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn zero_width_write_is_a_no_op() {
        let mut w = BitWriter::new();
        w.write_bits(0xFFFF_FFFF, 0);
        assert_eq!(w.bit_len(), 0);
        assert!(w.finish().is_empty());
    }

    #[test]
    fn zero_value_writes_zero_bits_at_full_width() {
        let mut w = BitWriter::new();
        w.write_bits(0, 5);
        assert_eq!(w.bit_len(), 5);
        assert_eq!(w.finish(), [0x00]);
    }

    #[test]
    fn value_bits_above_the_width_are_ignored() {
        let mut w = BitWriter::new();
        w.write_bits(0xFFFF_FFFF, 4);
        assert_eq!(w.finish(), [0b1111_0000]);
    }

    #[test]
    fn alignment_pads_with_zero_bits_and_counts_them() {
        let mut w = BitWriter::new();
        w.write_bits(0b1, 1);
        assert!(!w.is_byte_aligned());
        w.align_to_byte();
        assert!(w.is_byte_aligned());
        assert_eq!(w.bit_len(), 8, "padding is part of the bit position");
        assert_eq!(w.as_bytes(), [0b1000_0000]);
    }

    #[test]
    fn alignment_is_idempotent_when_already_aligned() {
        let mut w = BitWriter::new();
        w.write_bits(0x12, 8);
        w.align_to_byte();
        w.align_to_byte();
        assert_eq!(w.bit_len(), 8);
        assert_eq!(w.as_bytes(), [0x12]);
    }

    #[test]
    fn mixed_width_sequence_matches_manual_layout() {
        // 3 + 1 + 4 + 2 + 6 + 16 bits = 32 bits, laid out by hand MSB-first.
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b1, 1);
        w.write_bits(0b0011, 4);
        w.write_bits(0b10, 2);
        w.write_bits(0b011010, 6);
        w.write_bits(0x1234, 16);

        // 101 1 0011 10 011010 = 1011_0011 1001_1010 = 0xB3 0x9A
        assert_eq!(w.finish(), [0xB3, 0x9A, 0x12, 0x34]);
    }

    #[test]
    fn repeated_construction_is_deterministic() {
        fn build() -> Vec<u8> {
            let mut w = BitWriter::new();
            w.write_bits(0, 1);
            w.write_bits(0x5A5A_5A5A, 31);
            w.write_bit(true);
            w.write_bits(0b1010, 4);
            w.align_to_byte();
            w.write_bits(0xCAFE, 16);
            w.finish()
        }
        assert_eq!(build(), build());
    }

    #[test]
    fn clear_resets_all_state_and_reuses_the_writer() {
        let mut w = BitWriter::new();
        w.write_bits(0b111, 3);
        w.clear();
        assert_eq!(w.bit_len(), 0);
        assert!(w.is_byte_aligned());
        assert!(w.as_bytes().is_empty());
        w.write_bits(0x5, 4);
        assert_eq!(w.finish(), [0b0101_0000]);
    }

    #[test]
    fn into_bytes_drops_an_unaligned_tail_like_as_bytes() {
        let mut w = BitWriter::new();
        w.write_bits(0xFF, 8);
        w.write_bits(0b1, 1);
        assert_eq!(w.into_bytes(), [0xFF]);
    }
}
