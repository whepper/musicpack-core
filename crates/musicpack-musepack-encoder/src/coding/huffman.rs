//! Huffman/entropy primitives for the SV8 audio frame: the reference
//! `encodeLog` and `encodeEnum` from `codec/libmpcenc/bitstream.c`.
//!
//! The codebooks themselves live in [`super::huffman_tables`].

use super::huffman_tables as t;
use crate::bitwriter::BitWriter;

/// The reference `encodeLog`: a truncated binary code.
///
/// `max` is the number of representable values; the tables are indexed by
/// `max - 1`.
///
/// # Panics
///
/// Panics if `max == 0` (the reference never calls it that way; `max` is
/// `MaxBand + 1 >= 1`).
pub fn encode_log(writer: &mut BitWriter, value: u32, max: u32) {
    let index = (max - 1) as usize;
    let lost = u32::from(t::MPC_LOG2_LOST[index]);
    let bits = u32::from(t::MPC_LOG2[index]);
    if value < lost {
        writer.write_bits(value, bits - 1);
    } else {
        writer.write_bits(value + lost, bits);
    }
}

/// The reference `encodeEnum`: a combinatorial rank over `n` bits.
pub fn encode_enum(writer: &mut BitWriter, bits: u32, n: u32) {
    let mut code = 0u32;
    let mut k = 0usize;
    for i in 0..n {
        if (bits >> i) & 1 == 1 {
            code += t::CNK[k][i as usize];
            k += 1;
        }
    }
    if k == 0 {
        return;
    }
    let row = k - 1;
    let col = (n - 1) as usize;
    let lost = t::CNK_LOST[row][col];
    let len = u32::from(t::CNK_LEN[row][col]);
    if code < lost {
        writer.write_bits(code, len - 1);
    } else {
        writer.write_bits(code + lost, len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_log_matches_the_reference_rule() {
        // max = 1: MPC_LOG2[0] = 1, MPC_LOG2_LOST[0] = 0, so the single value
        // is written as one zero bit.
        let mut w = BitWriter::new();
        encode_log(&mut w, 0, 1);
        assert_eq!(w.finish(), [0x00]);

        // max = 3, MPC_LOG2[2] = 2, MPC_LOG2_LOST[2] = 0: value 0 -> two bits.
        let mut w = BitWriter::new();
        encode_log(&mut w, 0, 3);
        assert_eq!(w.finish(), [0b0000_0000]);
    }

    #[test]
    fn encode_enum_zero_is_empty() {
        let mut w = BitWriter::new();
        encode_enum(&mut w, 0, 18);
        assert!(w.finish().is_empty());
    }

    #[test]
    fn encode_enum_round_trips_a_single_bit() {
        // bits = 1 (only position 0 set), n = 4: k = 1, code = Cnk[0][0] = 0,
        // Cnk_lost[0][3] = 0, Cnk_len[0][3] = 2 (from the reference table), so
        // code < 0 is false -> write code + 0 in 2 bits -> '00'.
        let mut w = BitWriter::new();
        encode_enum(&mut w, 0b0001, 4);
        assert_eq!(w.finish(), [0b0000_0000]);
    }
}
