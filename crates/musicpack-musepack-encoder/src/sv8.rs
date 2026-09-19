//! Low-level Musepack SV8 container primitives.
//!
//! This module reproduces the bit-level behaviour of the reference encoder's
//! `encodeSize`, block framing and CRC (`codec/libmpcenc/bitstream.c`,
//! `codec/libmpcenc/libmpcenc.h`, `codec/common/crc32.c`) on top of the crate's
//! [`BitWriter`](crate::bitwriter::BitWriter). It knows nothing about audio:
//! the audio-frame coding and the psychoacoustic model live in later phases.
//!
//! # SV8 size fields
//!
//! An SV8 size is a base-128, big-endian value: seven value bits per byte with
//! the high bit (`0x80`) set on every byte except the last. Block sizes are
//! **self-including**: the encoded number is the block's total size in bytes,
//! including the two-byte key and the size field itself. [`encode_size`]
//! produces the plain form used for sample counts and seek positions;
//! [`encode_size_self_including`] produces the block-size form.
//!
//! ```
//! use musicpack_musepack_encoder::sv8::{encode_size, encode_size_self_including};
//! assert_eq!(encode_size(0).as_bytes(), [0x00]);
//! assert_eq!(encode_size(128).as_bytes(), [0x81, 0x00]);
//! assert_eq!(encode_size_self_including(0).as_bytes(), [0x01]);
//! ```

use crate::bitwriter::BitWriter;
use crate::error::EncoderError;

/// Maximum number of bytes an SV8 size field can occupy.
pub const MAX_SIZE_BYTES: usize = 10;

/// A base-128 SV8 size field, stored inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedSize {
    bytes: [u8; MAX_SIZE_BYTES],
    len: u8,
}

impl EncodedSize {
    /// The encoded bytes, most significant group first.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The number of bytes in the encoding (`1..=10`).
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Always `false`: an SV8 size always occupies at least one byte.
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// The smallest number of 7-bit groups needed to hold `value`.
///
/// With `self_including`, the code length is added to the value first, so the
/// condition is `2^(7i) - i > value`; otherwise it is `2^(7i) > value`.
fn encoded_width(value: u64, self_including: bool) -> usize {
    let mut i = 1usize;
    loop {
        let space = 1u128 << (7 * i);
        let fits = if self_including {
            space - i as u128 > value as u128
        } else {
            space > value as u128
        };
        if fits {
            return i;
        }
        i += 1;
        debug_assert!(i <= MAX_SIZE_BYTES, "u64 always fits ten SV8 size bytes");
    }
}

fn encode_size_inner(value: u64, self_including: bool) -> EncodedSize {
    let len = encoded_width(value, self_including);
    let mut value = u128::from(value) + if self_including { len as u128 } else { 0 };
    let mut bytes = [0u8; MAX_SIZE_BYTES];
    for slot in (0..len).rev() {
        bytes[slot] = (value as u8 & 0x7f) | 0x80;
        value >>= 7;
    }
    bytes[len - 1] &= 0x7f;
    EncodedSize {
        bytes,
        len: len as u8,
    }
}

/// Encodes a plain SV8 base-128 size (sample counts, seek positions).
///
/// This is the reference `encodeSize(value, buf, MPC_FALSE)`.
pub fn encode_size(value: u64) -> EncodedSize {
    encode_size_inner(value, false)
}

/// Encodes a **self-including** SV8 size (`2^i` form used for block sizes).
///
/// This is the reference `encodeSize(value, buf, MPC_TRUE)`: the code length
/// is folded into the value, so `encode_size_self_including(0)` is `[0x01]`.
pub fn encode_size_self_including(value: u64) -> EncodedSize {
    encode_size_inner(value, true)
}

/// Running CRC-32/IEEE (reflected polynomial `0xEDB88320`, init/final
/// `0xFFFF_FFFF`) — the reference `mpc_crc32`.
#[derive(Debug, Clone)]
struct Crc32 {
    state: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { state: 0xffff_ffff }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (self.state & 1).wrapping_neg();
                self.state = (self.state >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    fn update_zeros(&mut self, count: usize) {
        for _ in 0..count {
            self.state ^= 0;
            for _ in 0..8 {
                let mask = (self.state & 1).wrapping_neg();
                self.state = (self.state >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    fn finish(&self) -> u32 {
        !self.state
    }
}

/// CRC-32/IEEE of `bytes`, as written big-endian in the SV8 `SH` block.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

/// Writes a truncated Golomb code (`unary quotient` + `k`-bit remainder),
/// matching the reference `encodeGolomb`.
///
/// The unary part is chunked into 31-bit pieces because the reference writes
/// through a 31-bit-maximum bit field.
///
/// # Panics
///
/// Panics if `k > 31` (a programming error, not caller data).
pub fn encode_golomb(writer: &mut BitWriter, value: u32, k: u32) {
    assert!(
        k <= 31,
        "Golomb remainder width must be at most 31, got {k}"
    );
    let mut unary = (value >> k) + 1;
    let remainder = if k == 0 { 0 } else { value & ((1u32 << k) - 1) };
    while unary > 31 {
        writer.write_bits(0, 31);
        unary -= 31;
    }
    writer.write_bits(1, unary);
    writer.write_bits(remainder, k);
}

/// Maps a signed seek-table second difference to the reference's unsigned
/// code: the magnitude occupies all bits except the least significant, which
/// carries the sign. The magnitude is formed without negating `diff`, and the
/// 64-bit result is truncated to 32 bits exactly as the reference does.
pub fn seek_delta(diff: i64) -> u32 {
    let magnitude = diff.unsigned_abs();
    (magnitude.wrapping_shl(1) | u64::from(diff < 0)) as u32
}

/// A validated two-byte SV8 block key.
///
/// The decoder rejects keys whose bytes are not ASCII uppercase, so the
/// constructor enforces that here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockKey([u8; 2]);

impl BlockKey {
    /// `SH` stream header.
    pub const SH: Self = Self(*b"SH");
    /// `RG` replay gain.
    pub const RG: Self = Self(*b"RG");
    /// `EI` encoder info.
    pub const EI: Self = Self(*b"EI");
    /// `SO` seek-table offset placeholder.
    pub const SO: Self = Self(*b"SO");
    /// `AP` audio packet.
    pub const AP: Self = Self(*b"AP");
    /// `ST` seek table.
    pub const ST: Self = Self(*b"ST");
    /// `SE` end of stream.
    pub const SE: Self = Self(*b"SE");

    /// Validates a two-byte key as two ASCII uppercase letters.
    pub fn new(key: [u8; 2]) -> Result<Self, EncoderError> {
        if key[0].is_ascii_uppercase() && key[1].is_ascii_uppercase() {
            Ok(Self(key))
        } else {
            Err(EncoderError::InvalidBlockKey(key))
        }
    }

    /// The raw key bytes.
    pub fn as_bytes(self) -> [u8; 2] {
        self.0
    }
}

/// What [`write_block`] produced, for callers that need to patch or audit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    /// The block key.
    pub key: BlockKey,
    /// Offset of the block's key within the output buffer.
    pub offset: usize,
    /// Encoded size field.
    pub size: EncodedSize,
    /// Payload bytes written, including any zero padding and the CRC.
    pub data_len: usize,
}

/// Appends one framed SV8 block: `key`, self-including size, optional CRC,
/// payload.
///
/// `with_crc` is `true` only for `SH`, matching the reference encoder.
pub fn write_block(out: &mut Vec<u8>, key: BlockKey, payload: &[u8], with_crc: bool) -> BlockInfo {
    write_block_with_min_size(out, key, payload, with_crc, 0)
}

/// [`write_block`] with the reference's `min_size` behaviour.
///
/// `min_data_len` is the minimum payload-plus-CRC length; the payload is
/// zero-padded to reach it. The reference uses this when rewriting `SH` in
/// place so the block-size field keeps its original width.
pub fn write_block_with_min_size(
    out: &mut Vec<u8>,
    key: BlockKey,
    payload: &[u8],
    with_crc: bool,
    min_data_len: usize,
) -> BlockInfo {
    let crc_len = if with_crc { 4 } else { 0 };
    let mut payload_len = payload.len();
    let pad = if payload_len + crc_len < min_data_len {
        min_data_len - payload_len - crc_len
    } else {
        0
    };
    payload_len += pad;
    let data_len = payload_len + crc_len;
    let size = encode_size_self_including(data_len as u64 + 2);

    let offset = out.len();
    out.extend_from_slice(&key.as_bytes());
    out.extend_from_slice(size.as_bytes());
    if with_crc {
        let mut crc = Crc32::new();
        crc.update(payload);
        crc.update_zeros(pad);
        out.extend_from_slice(&crc.finish().to_be_bytes());
    }
    out.extend_from_slice(payload);
    out.resize(out.len() + pad, 0);

    BlockInfo {
        key,
        offset,
        size,
        data_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a base-128 size field, returning `(value, byte_count)`.
    fn decode_size(bytes: &[u8]) -> (u64, usize) {
        let mut value = 0u64;
        for (index, &byte) in bytes.iter().enumerate() {
            value = value * 128 + u64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return (value, index + 1);
            }
        }
        panic!("unterminated size field")
    }

    #[test]
    fn plain_size_small_values_are_single_bytes() {
        assert_eq!(encode_size(0).as_bytes(), [0x00]);
        assert_eq!(encode_size(1).as_bytes(), [0x01]);
        assert_eq!(encode_size(127).as_bytes(), [0x7f]);
    }

    #[test]
    fn plain_size_crosses_to_two_bytes_at_128() {
        assert_eq!(encode_size(128).as_bytes(), [0x81, 0x00]);
        assert_eq!(encode_size(16383).as_bytes(), [0xff, 0x7f]);
        assert_eq!(encode_size(16384).as_bytes(), [0x81, 0x80, 0x00]);
    }

    #[test]
    fn plain_size_round_trips_at_every_width_boundary() {
        for width in 1..=MAX_SIZE_BYTES {
            let max = (1u128 << (7 * width)) - 1;
            for value in [max as u64, (max as u64).saturating_sub(1)] {
                let encoded = encode_size(value);
                assert_eq!(encoded.len(), width, "value {value}");
                assert_eq!(decode_size(encoded.as_bytes()), (value, width));
            }
        }
    }

    #[test]
    fn plain_size_maximum_is_ten_bytes_and_round_trips() {
        let encoded = encode_size(u64::MAX);
        assert_eq!(encoded.len(), MAX_SIZE_BYTES);
        assert_eq!(decode_size(encoded.as_bytes()), (u64::MAX, 10));
        // 2^64-1 is one 1-bit group followed by nine 127-bit groups.
        assert_eq!(encoded.as_bytes()[0], 0x81);
        assert_eq!(&encoded.as_bytes()[1..9], &[0xff; 8]);
        assert_eq!(encoded.as_bytes()[9], 0x7f);
    }

    #[test]
    fn self_including_size_short_payloads_are_single_bytes() {
        assert_eq!(encode_size_self_including(0).as_bytes(), [0x01]);
        assert_eq!(encode_size_self_including(2).as_bytes(), [0x03]);
        assert_eq!(encode_size_self_including(126).as_bytes(), [0x7f]);
    }

    #[test]
    fn self_including_size_grows_at_127() {
        // value + 1 byte code length: 127 + 1 = 128, needs two groups.
        assert_eq!(encode_size_self_including(127).as_bytes(), [0x81, 0x01]);
    }

    #[test]
    fn self_including_size_round_trips_through_the_reference_relation() {
        for value in [0u64, 1, 2, 125, 126, 127, 16381, 16382, 1 << 20, 1 << 40] {
            let encoded = encode_size_self_including(value);
            let (decoded, width) = decode_size(encoded.as_bytes());
            assert_eq!(width, encoded.len());
            assert_eq!(
                decoded,
                value + width as u64,
                "self-including value {value} must decode to value + width"
            );
        }
    }

    #[test]
    fn crc32_matches_the_standard_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(&[0u8; 32]), 0x190a_55ad);
    }

    #[test]
    fn crc32_incremental_matches_oneshot() {
        let mut crc = Crc32::new();
        crc.update(b"12345");
        crc.update(b"6789");
        assert_eq!(crc.finish(), crc32(b"123456789"));
    }

    #[test]
    fn golomb_zero_with_twelve_bit_remainder() {
        let mut w = BitWriter::new();
        encode_golomb(&mut w, 0, 12);
        // unary(0) = one bit '1', then twelve zero remainder bits.
        assert_eq!(w.finish(), [0b1000_0000, 0b0000_0000]);
    }

    #[test]
    fn golomb_small_value_remainder_is_low_bits() {
        let mut w = BitWriter::new();
        encode_golomb(&mut w, 1, 12);
        // '1' + 0000_0000_0001: 13 bits.
        assert_eq!(w.finish(), [0b1000_0000, 0b0000_1000]);
    }

    #[test]
    fn golomb_quotient_larger_than_thirty_one_is_chunked() {
        let mut w = BitWriter::new();
        // value with quotient 32: unary length 33 -> 31 zero bits then value 1
        // in 2 bits.
        encode_golomb(&mut w, 32, 0);
        let bytes = w.finish();
        // 33 bits: 31 zeros, then value 1 in 2 bits ('0','1'), then 7 pad bits.
        // The single '1' is the first bit of the fifth byte.
        assert_eq!(bytes[0..4], [0x00, 0x00, 0x00, 0x00]);
        assert_eq!(bytes[4], 0b1000_0000);
    }

    #[test]
    fn seek_delta_is_interleaved_magnitude_and_sign() {
        assert_eq!(seek_delta(0), 0);
        assert_eq!(seek_delta(1), 2);
        assert_eq!(seek_delta(-1), 3);
        assert_eq!(seek_delta(7), 14);
        assert_eq!(seek_delta(-7), 15);
        assert_eq!(seek_delta(i64::MIN), 1);
    }

    #[test]
    fn block_key_rejects_non_uppercase() {
        assert!(BlockKey::new(*b"SH").is_ok());
        assert_eq!(
            BlockKey::new(*b"sh"),
            Err(EncoderError::InvalidBlockKey(*b"sh"))
        );
        assert!(BlockKey::new(*b"A ").is_err());
    }

    #[test]
    fn end_of_stream_block_is_three_bytes() {
        let mut out = Vec::new();
        write_block(&mut out, BlockKey::SE, &[], false);
        assert_eq!(out, b"SE\x03");
    }

    #[test]
    fn crc_block_layout_is_key_size_crc_payload() {
        let payload = [0xAAu8, 0xBB];
        let mut out = Vec::new();
        let info = write_block(&mut out, BlockKey::SH, &payload, true);
        assert_eq!(info.data_len, 2 + 4);
        assert_eq!(&out[0..2], b"SH");
        // self-including size of data_len(6) + 2 = 8 -> 8 + 1 code byte = 9.
        assert_eq!(info.size.as_bytes(), [0x09]);
        assert_eq!(out[2], 0x09);
        assert_eq!(&out[3..7], &crc32(&payload).to_be_bytes());
        assert_eq!(&out[7..], &payload);
    }

    #[test]
    fn min_size_pads_the_payload_and_crc_covers_the_padding() {
        let mut out = Vec::new();
        let info = write_block_with_min_size(&mut out, BlockKey::SE, &[], false, 3);
        assert_eq!(info.data_len, 3);
        // self-including size for data_len 3 + 2 = 5 -> [0x06].
        assert_eq!(out, [b'S', b'E', 0x06, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn size_field_width_boundary_for_block_framing() {
        // data_len + 2 <= 126 keeps a one-byte size field; 127 needs two.
        let one_byte = write_block_to_vec(124, false);
        assert_eq!(one_byte[2..3], [0x7f]);
        let two_byte = write_block_to_vec(125, false);
        assert_eq!(two_byte[2..4], [0x81, 0x01]);
    }

    fn write_block_to_vec(payload_len: usize, with_crc: bool) -> Vec<u8> {
        let payload = vec![0u8; payload_len];
        let mut out = Vec::new();
        write_block(&mut out, BlockKey::AP, &payload, with_crc);
        out
    }
}
