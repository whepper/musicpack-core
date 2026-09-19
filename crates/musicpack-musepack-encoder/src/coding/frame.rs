//! SV8 audio-frame coding — the reference `writeBitstream_SV8`
//! (`codec/libmpcenc/encode_sv7.c`).
//!
//! This turns one frame's coding state (`Res`, post-allocation `SCF_Index`,
//! `MS_Flag`, `Q`) into the entropy-coded bit stream, accumulates frames into
//! an `AP` block, and frames the block through the Phase 15C SV8 writer. It
//! does **not** reimplement container framing.
//!
//! # State
//!
//! The reference keeps frame-to-frame state on `mpc_encoder_t`: `framesInBlock`,
//! `MaxBand`, `DSCF_Flag_L/R` (reset to 1 on the first frame of a block) and
//! `SCF_Last_L/R`. [`FrameEncoder`] owns all of it, including the shared bit
//! buffer for the current block. `Q` itself is owned by the quantiser and is
//! passed in unchanged.
//!
//! # Bit ordering
//!
//! All fields go through the crate's MSB-first [`BitWriter`], matching the
//! reference `writeBits`. Frames are concatenated without inter-frame byte
//! alignment; the buffer is byte-aligned (zero padding) only when the `AP`
//! block is flushed, exactly as `writeBlock` does.

use super::huffman::{encode_enum, encode_log};
use super::huffman_tables as t;
use crate::bitwriter::BitWriter;
use crate::error::EncoderError;
use crate::sv8::{BlockKey, write_block};

const SUBBANDS: usize = 32;
const SAMPLES: usize = 36;
/// The reference `thres[9]` per-`Res` context selector.
const THRES: [i32; 9] = [0, 0, 3, 7, 9, 1, 3, 4, 8];

/// Codes SV8 audio frames into `AP` blocks.
#[derive(Debug)]
pub struct FrameEncoder {
    bits: BitWriter,
    frames_in_block: u32,
    frames_per_block_pwr: u32,
    max_band: i32,
    dscf_flag_l: [i32; SUBBANDS],
    dscf_flag_r: [i32; SUBBANDS],
    scf_last_l: [i32; SUBBANDS],
    scf_last_r: [i32; SUBBANDS],
    out: Vec<u8>,
}

impl FrameEncoder {
    /// Creates an encoder that flushes an `AP` block every
    /// `1 << frames_per_block_pwr` frames.
    pub fn new(frames_per_block_pwr: u32) -> Self {
        Self {
            bits: BitWriter::new(),
            frames_in_block: 0,
            frames_per_block_pwr,
            max_band: 0,
            dscf_flag_l: [0; SUBBANDS],
            dscf_flag_r: [0; SUBBANDS],
            scf_last_l: [0; SUBBANDS],
            scf_last_r: [0; SUBBANDS],
            out: Vec::new(),
        }
    }

    /// The `AP` block bytes flushed so far.
    pub fn blocks(&self) -> &[u8] {
        &self.out
    }

    /// Drains the `AP` blocks flushed since the previous call.
    ///
    /// One [`encode_frame`](Self::encode_frame) call flushes at most one block,
    /// so this returns that block (or an empty vector).
    pub fn take_blocks(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Flushes the current partial `AP` block, reproducing the reference's
    /// post-loop `writeBlock(&e, "AP", ...)`.
    pub fn flush_partial(&mut self) -> Vec<u8> {
        if self.frames_in_block == 0 {
            return Vec::new();
        }
        self.bits.align_to_byte();
        let payload = self.bits.as_bytes().to_vec();
        write_block(&mut self.out, BlockKey::AP, &payload, false);
        self.bits.clear();
        self.frames_in_block = 0;
        std::mem::take(&mut self.out)
    }

    /// Takes the `AP` block bytes flushed so far.
    pub fn into_blocks(self) -> Vec<u8> {
        self.out
    }

    /// Encodes one frame, flushing an `AP` block when the block is full.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame(
        &mut self,
        max_band_param: i32,
        ms_channelmode: u32,
        res_l: &[i32; SUBBANDS],
        res_r: &[i32; SUBBANDS],
        scf_l: &[[i32; 3]; SUBBANDS],
        scf_r: &[[i32; 3]; SUBBANDS],
        ms_flag: &[i32; SUBBANDS],
        q_l: &[[i16; SAMPLES]; SUBBANDS],
        q_r: &[[i16; SAMPLES]; SUBBANDS],
    ) -> Result<(), EncoderError> {
        if !(0..=31).contains(&max_band_param) {
            return Err(EncoderError::InvalidMaxBand(max_band_param.max(0) as usize));
        }

        // Highest band with any resolution.
        let mut n = max_band_param;
        while n >= 0 && res_l[n as usize] == 0 && res_r[n as usize] == 0 {
            n -= 1;
        }
        n += 1;

        let max_band: i32;
        if self.frames_in_block == 0 {
            encode_log(&mut self.bits, n as u32, (max_band_param + 1) as u32);
            max_band = n;
            self.max_band = n;
        } else {
            let mut delta = n - self.max_band;
            max_band = delta + self.max_band;
            self.max_band = max_band;
            if delta < 0 {
                delta += 33;
            }
            let h = t::HUFF_BANDS[delta as usize];
            self.bits.write_bits(u32::from(h.code), u32::from(h.len));
        }

        if max_band > 0 {
            self.resolution(max_band, res_l, res_r, ms_channelmode, ms_flag);
        }

        // SCF coding type (the first frame of a block forces key frames).
        if self.frames_in_block == 0 {
            self.dscf_flag_l = [1; SUBBANDS];
            self.dscf_flag_r = [1; SUBBANDS];
        }
        for nn in 0..max_band as usize {
            let mut tmp = 0i32;
            let mut cnt = -1i32;
            if res_l[nn] != 0 {
                tmp = i32::from(scf_l[nn][1] == scf_l[nn][0]) * 2
                    + i32::from(scf_l[nn][2] == scf_l[nn][1]);
                cnt += 1;
            }
            if res_r[nn] != 0 {
                tmp = (tmp << 2)
                    | (i32::from(scf_r[nn][1] == scf_r[nn][0]) * 2
                        + i32::from(scf_r[nn][2] == scf_r[nn][1]));
                cnt += 1;
            }
            if cnt >= 0 {
                let h = if cnt == 0 {
                    t::HUFF_SCFI_1[tmp as usize]
                } else {
                    t::HUFF_SCFI_2[tmp as usize]
                };
                self.bits.write_bits(u32::from(h.code), u32::from(h.len));
            }
        }

        // Scale factors.
        for nn in 0..max_band as usize {
            if res_l[nn] != 0 {
                self.write_scf(nn, true, scf_l[nn]);
            }
            if res_r[nn] != 0 {
                self.write_scf(nn, false, scf_r[nn]);
            }
        }

        // Quantised samples.
        for nn in 0..max_band as usize {
            self.encode_samples(res_l[nn], &q_l[nn]);
            self.encode_samples(res_r[nn], &q_r[nn]);
        }

        self.frames_in_block += 1;
        if self.frames_in_block == (1 << self.frames_per_block_pwr) {
            self.bits.align_to_byte();
            let payload = self.bits.as_bytes().to_vec();
            write_block(&mut self.out, BlockKey::AP, &payload, false);
            self.bits.clear();
            self.frames_in_block = 0;
        }
        Ok(())
    }

    fn resolution(
        &mut self,
        max_band: i32,
        res_l: &[i32; SUBBANDS],
        res_r: &[i32; SUBBANDS],
        ms_channelmode: u32,
        ms_flag: &[i32; SUBBANDS],
    ) {
        let mut tmp = res_l[(max_band - 1) as usize];
        if tmp < 0 {
            tmp += 17;
        }
        let h = t::HUFF_RES[tmp as usize];
        self.bits.write_bits(u32::from(h.code), u32::from(h.len));

        let mut tmp = res_r[(max_band - 1) as usize];
        if tmp < 0 {
            tmp += 17;
        }
        let h = t::HUFF_RES[tmp as usize];
        self.bits.write_bits(u32::from(h.code), u32::from(h.len));

        for nn in (0..=(max_band - 2)).rev() {
            let mut tmp = res_l[nn as usize] - res_l[(nn + 1) as usize];
            if tmp < 0 {
                tmp += 17;
            }
            let ctx = usize::from(res_l[(nn + 1) as usize] > 2);
            let h = t::HUFF_RES[ctx * 17 + tmp as usize];
            self.bits.write_bits(u32::from(h.code), u32::from(h.len));

            let mut tmp = res_r[nn as usize] - res_r[(nn + 1) as usize];
            if tmp < 0 {
                tmp += 17;
            }
            let ctx = usize::from(res_r[(nn + 1) as usize] > 2);
            let h = t::HUFF_RES[ctx * 17 + tmp as usize];
            self.bits.write_bits(u32::from(h.code), u32::from(h.len));
        }

        if ms_channelmode > 0 {
            let mut bits = 0u32;
            let mut count = 0u32;
            let mut total = 0u32;
            for nn in 0..max_band as usize {
                if res_l[nn] != 0 || res_r[nn] != 0 {
                    bits = (bits << 1) | (ms_flag[nn] as u32);
                    count += ms_flag[nn] as u32;
                    total += 1;
                }
            }
            encode_log(&mut self.bits, count, total);
            if count * 2 > total {
                bits = !bits;
            }
            encode_enum(&mut self.bits, bits, total);
        }
    }

    fn write_scf(&mut self, band: usize, left: bool, scf: [i32; 3]) {
        let (dscf_flag, scf_last) = if left {
            (&mut self.dscf_flag_l[band], &mut self.scf_last_l[band])
        } else {
            (&mut self.dscf_flag_r[band], &mut self.scf_last_r[band])
        };
        if *dscf_flag == 1 {
            self.bits.write_bits((scf[0] + 6) as u32, 7);
            *dscf_flag = 0;
        } else {
            let tmp = ((scf[0] - *scf_last + 31) & 127) as u32;
            if tmp < 64 {
                let h = t::HUFF_DSCF_2[tmp as usize];
                self.bits.write_bits(u32::from(h.code), u32::from(h.len));
            } else {
                let h = t::HUFF_DSCF_2[64];
                self.bits.write_bits(u32::from(h.code), u32::from(h.len));
                self.bits.write_bits(tmp - 64, 6);
            }
        }
        for m in 0..2 {
            if scf[m + 1] != scf[m] {
                let tmp = ((scf[m + 1] - scf[m] + 31) & 127) as u32;
                if tmp < 64 {
                    let h = t::HUFF_DSCF_1[tmp as usize];
                    self.bits.write_bits(u32::from(h.code), u32::from(h.len));
                } else {
                    let h = t::HUFF_DSCF_1[31];
                    self.bits.write_bits(u32::from(h.code), u32::from(h.len));
                    self.bits.write_bits(tmp - 64, 6);
                }
            }
        }
        *scf_last = scf[2];
    }

    fn write_huff(&mut self, h: t::Huff) {
        self.bits.write_bits(u32::from(h.code), u32::from(h.len));
    }

    #[allow(clippy::needless_range_loop)]
    fn encode_samples(&mut self, res: i32, q: &[i16; SAMPLES]) {
        let mut k = 0usize;
        match res {
            -1 | 0 => {}
            1 => {
                let table = &t::HUFF_Q1;
                let mut idx: u32 = 1;
                while k < SAMPLES {
                    let kmax = k + 18;
                    let mut cnt = 0u32;
                    let mut sng = 0u32;
                    while k < kmax {
                        idx <<= 1;
                        if q[k] != 1 {
                            cnt += 1;
                            idx |= 1;
                            sng = (sng << 1) | ((q[k] >> 1) as u32);
                        }
                        k += 1;
                    }
                    self.write_huff(table[cnt as usize]);
                    if cnt > 0 {
                        if cnt > 9 {
                            idx = !idx;
                        }
                        encode_enum(&mut self.bits, idx, 18);
                        self.bits.write_bits(sng, cnt);
                    }
                }
            }
            2 => {
                let mut idx = 2 * THRES[2];
                while k < SAMPLES {
                    let tmp = q[k] as i32 + 5 * q[k + 1] as i32 + 25 * q[k + 2] as i32;
                    let sel = usize::from(idx > THRES[2]);
                    self.write_huff(t::HUFF_Q2[sel * 125 + tmp as usize]);
                    idx = (idx >> 1) + i32::from(t::HUFF_Q2_VAR[tmp as usize]);
                    k += 3;
                }
            }
            3 | 4 => {
                let table: &[t::Huff] = if res == 3 { &t::HUFF_Q3 } else { &t::HUFF_Q4 };
                while k < SAMPLES {
                    let tmp = q[k] as i32 + THRES[res as usize] * q[k + 1] as i32;
                    self.write_huff(table[tmp as usize]);
                    k += 2;
                }
            }
            5..=8 => {
                let (table, stride): (&[t::Huff], usize) = match res {
                    5 => (&t::HUFF_Q5, 15),
                    6 => (&t::HUFF_Q6, 31),
                    7 => (&t::HUFF_Q7, 63),
                    _ => (&t::HUFF_Q8, 127),
                };
                let mut idx = 2 * THRES[res as usize];
                while k < SAMPLES {
                    let sel = usize::from(idx > THRES[res as usize]);
                    self.write_huff(table[sel * stride + q[k] as usize]);
                    let mut tmp = q[k] as i32 - (1 << (res - 2)) + 1;
                    if tmp < 0 {
                        tmp = -tmp;
                    }
                    idx = (idx >> 1) + tmp;
                    k += 1;
                }
            }
            _ => {
                while k < SAMPLES {
                    self.write_huff(t::HUFF_Q9UP[(q[k] >> (res - 9)) as usize]);
                    if res != 9 {
                        let low = (q[k] as u32) & ((1 << (res - 9)) - 1);
                        self.bits.write_bits(low, (res - 9) as u32);
                    }
                    k += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_a_minimal_zero_band_frame() {
        let mut enc = FrameEncoder::new(2);
        let zeros_i32 = [0i32; SUBBANDS];
        let zeros_scf = [[0i32; 3]; SUBBANDS];
        let zeros_q = [[0i16; SAMPLES]; SUBBANDS];
        enc.encode_frame(
            0, 11, &zeros_i32, &zeros_i32, &zeros_scf, &zeros_scf, &zeros_i32, &zeros_q, &zeros_q,
        )
        .unwrap();
        // Four zero-band frames form one AP block with a small payload.
        for _ in 0..3 {
            enc.encode_frame(
                0, 11, &zeros_i32, &zeros_i32, &zeros_scf, &zeros_scf, &zeros_i32, &zeros_q,
                &zeros_q,
            )
            .unwrap();
        }
        let blocks = enc.into_blocks();
        assert_eq!(&blocks[0..2], b"AP");
    }

    #[test]
    fn rejects_out_of_range_band_count() {
        let mut enc = FrameEncoder::new(2);
        let zeros_i32 = [0i32; SUBBANDS];
        let zeros_scf = [[0i32; 3]; SUBBANDS];
        let zeros_q = [[0i16; SAMPLES]; SUBBANDS];
        assert!(
            enc.encode_frame(
                32, 11, &zeros_i32, &zeros_i32, &zeros_scf, &zeros_scf, &zeros_i32, &zeros_q,
                &zeros_q,
            )
            .is_err()
        );
    }
}
