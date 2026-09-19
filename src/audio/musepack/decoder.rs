//! Musepack SV8 audio decoding: bitstream, requantisation, synthesis.
//!
//! Port of the project's vendored `libmpcdec` (`mpc_decoder.c` frame decode
//! and `read_bitstream_sv8`, `requant.c`, `synth_filter.c`,
//! `mpc_bits_reader.c` helpers). Arithmetic follows the reference exactly in
//! its floating-point configuration: the reference is built without
//! `MPC_FIXED_POINT`, so [`MPC_SHL`](self) and every `MPC_SCALE_CONST*` are
//! plain `f32` multiply/adds and no fixed-point shifts apply.
//!
//! All state is owned by the decoder; [`MpcDecoder::read_f32`] may be called
//! with arbitrary frame counts and resumes exactly where it stopped.

// The synthesis and scalefactor constants are the reference's decimal literals,
// kept verbatim so the compiler rounds them to the same `f32`/`f64` values.
#![allow(clippy::excessive_precision)]

use std::io::Read;

use super::sv8::{MusepackError, MusepackInfo};
use super::tables::{
    self, CAN_BANDS, CAN_DSCF, CAN_Q1, CAN_Q9UP, CAN_RES, CAN_SCFI, CanRef, DC, DI_OPT_INT, LOG2,
    LOG2_LOST,
};

/// Samples per Musepack frame (`36 * 32`).
pub const FRAME_LENGTH: usize = 36 * 32;
/// Reference synthesizer delay.
pub const SYNTH_DELAY: u64 = 481;
const V_MEM: usize = 2304;
const V_SIZE: usize = V_MEM + 960;
const MAX_BANDS: usize = 32;
/// Maximum accepted size of one compressed SV8 block payload, in bytes.
///
/// The reference demux rejects a block larger than `DEMUX_BUFFER_SIZE - 11`
/// (`(65536 - 4352) - 11`). This bounds the streaming input buffer independent
/// of the member size: at most one block is buffered at a time.
pub const MAX_BLOCK_BYTES: usize = (65536 - 4352) - 11;
/// Synthesis window offsets (`synth_filter.c`).
const SYNTH_OFFSETS: [usize; 16] = [
    0, 96, 128, 224, 256, 352, 384, 480, 512, 608, 640, 736, 768, 864, 896, 992,
];

/// Bounded, incremental SV8 block reader over a caller-owned `Read`.
///
/// Reads the `MPCK` magic and header blocks at construction, then exposes one
/// audio (`AP`) block payload at a time in `block_buf`. No whole-stream
/// buffering: only the current block is held (≤ [`MAX_BLOCK_BYTES`]).
struct BlockReader {
    source: Box<dyn Read>,
    block_buf: Vec<u8>,
    pending_ap: Option<usize>,
    /// Compressed bytes taken from `source` so far (diagnostics/tests).
    consumed: usize,
}

impl BlockReader {
    fn new(source: Box<dyn Read>) -> Self {
        Self {
            source,
            block_buf: Vec::new(),
            pending_ap: None,
            consumed: 0,
        }
    }

    /// Fills `buf` completely. `Ok(false)` when EOF occurs before any byte;
    /// a partial fill is a truncation error.
    fn fill(source: &mut dyn Read, buf: &mut [u8]) -> Result<bool, MusepackError> {
        let mut filled = 0usize;
        while filled < buf.len() {
            match source.read(&mut buf[filled..]) {
                Ok(0) => {
                    if filled == 0 {
                        return Ok(false);
                    }
                    return Err(MusepackError("truncated SV8 stream".into()));
                }
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(MusepackError(format!("SV8 source error: {e}"))),
            }
        }
        Ok(true)
    }

    fn read_exact_or_eof(&mut self, buf: &mut [u8]) -> Result<bool, MusepackError> {
        let filled = Self::fill(&mut *self.source, buf)?;
        if filled {
            self.consumed += buf.len();
        }
        Ok(filled)
    }

    fn read_byte(&mut self) -> Result<Option<u8>, MusepackError> {
        let mut byte = [0u8; 1];
        if self.read_exact_or_eof(&mut byte)? {
            Ok(Some(byte[0]))
        } else {
            Ok(None)
        }
    }

    fn read_key(&mut self) -> Result<Option<[u8; 2]>, MusepackError> {
        let Some(a) = self.read_byte()? else {
            return Ok(None);
        };
        let b = self
            .read_byte()?
            .ok_or_else(|| MusepackError("truncated SV8 block key".into()))?;
        if !a.is_ascii_uppercase() || !b.is_ascii_uppercase() {
            return Err(MusepackError("invalid SV8 block key".into()));
        }
        Ok(Some([a, b]))
    }

    /// Returns `(declared_size, header_len)` where `header_len` counts the key.
    fn read_block_size(&mut self) -> Result<(u64, usize), MusepackError> {
        let mut size = 0u64;
        let mut count = 0usize;
        loop {
            let byte = self
                .read_byte()?
                .ok_or_else(|| MusepackError("truncated SV8 size field".into()))?;
            count += 1;
            size = size
                .checked_mul(128)
                .and_then(|s| s.checked_add((byte & 0x7f) as u64))
                .ok_or_else(|| MusepackError("SV8 size overflow".into()))?;
            if size > (1 << 48) {
                return Err(MusepackError("SV8 size exceeds the accepted bound".into()));
            }
            if byte & 0x80 == 0 {
                return Ok((size, 2 + count));
            }
            if count > 10 {
                return Err(MusepackError("SV8 size field too long".into()));
            }
        }
    }

    fn read_payload(&mut self, len: usize) -> Result<(), MusepackError> {
        if len > MAX_BLOCK_BYTES {
            return Err(MusepackError("SV8 block exceeds the accepted bound".into()));
        }
        self.block_buf.resize(len, 0);
        if !Self::fill(&mut *self.source, &mut self.block_buf)? {
            return Err(MusepackError("truncated SV8 block payload".into()));
        }
        self.consumed += len;
        Ok(())
    }

    /// Loads the next audio (`AP`) block payload into `block_buf`; returns
    /// `false` at a clean stream end (`SE` or EOF at a block boundary).
    fn next_ap_block(&mut self) -> Result<bool, MusepackError> {
        if let Some(len) = self.pending_ap.take() {
            self.read_payload(len)?;
            return Ok(true);
        }
        loop {
            let Some(key) = self.read_key()? else {
                return Ok(false);
            };
            let (declared, header_len) = self.read_block_size()?;
            if declared < header_len as u64 {
                return Err(MusepackError("SV8 block smaller than its header".into()));
            }
            let len = usize::try_from(declared - header_len as u64)
                .map_err(|_| MusepackError("SV8 block length overflows usize".into()))?;
            if &key == b"AP" {
                self.read_payload(len)?;
                return Ok(true);
            }
            if &key == b"SE" {
                return Ok(false);
            }
            self.read_payload(len)?; // skip a non-audio block
        }
    }
}

/// Streaming SV8 decoder producing interleaved `f32` PCM.
pub struct MpcDecoder {
    // Container: the compressed input is consumed incrementally.
    reader: BlockReader,
    info: MusepackInfo,
    block_frames_left: u32,
    bit_pos: u64,

    // Stream facts.
    channels: usize,
    max_band: i32,
    ms: bool,
    block_pwr: u32,
    samples: u64,
    decoded_samples: u64,
    samples_to_skip: u64,

    // Per-frame bitstream state.
    last_max_band: i32,
    res_l: [i32; MAX_BANDS],
    res_r: [i32; MAX_BANDS],
    scf_index_l: [[i32; 3]; MAX_BANDS],
    scf_index_r: [[i32; 3]; MAX_BANDS],
    scfi_l: [i32; MAX_BANDS],
    scfi_r: [i32; MAX_BANDS],
    dscf_flag_l: [bool; MAX_BANDS],
    dscf_flag_r: [bool; MAX_BANDS],
    ms_flag: [bool; MAX_BANDS],
    q_l: [[i16; 36]; MAX_BANDS],
    q_r: [[i16; 36]; MAX_BANDS],
    r1: u32,
    r2: u32,

    // Synthesis state.
    y_l: [[f32; 32]; 36],
    y_r: [[f32; 32]; 36],
    v_l: Vec<f32>,
    v_r: Vec<f32>,
    scf: [f32; 256],
    di: [[f32; 16]; 32],
    cnk: [[u32; 32]; 16],

    // Output.
    frame: Vec<f32>,
    frame_len: usize,
    frame_pos: usize,
    eof: bool,
}

impl MpcDecoder {
    /// Opens a streaming decoder over a caller-owned reader.
    ///
    /// Reads only the `MPCK` magic and the header blocks up to the first audio
    /// block; the compressed member is consumed incrementally thereafter. The
    /// reference's `check_streaminfo` validation is applied before playback.
    pub fn from_reader(source: Box<dyn Read>) -> Result<Self, MusepackError> {
        let mut reader = BlockReader::new(source);
        let mut magic = [0u8; 4];
        if !reader.read_exact_or_eof(&mut magic)? || &magic != super::sv8::MAGIC {
            return Err(MusepackError("not a Musepack SV8 stream (MPCK)".into()));
        }
        let mut info = MusepackInfo::default();
        let mut have_header = false;
        loop {
            let Some(key) = reader.read_key()? else {
                return Err(MusepackError(
                    "SV8 stream ended before the audio blocks".into(),
                ));
            };
            let (declared, header_len) = reader.read_block_size()?;
            if declared < header_len as u64 {
                return Err(MusepackError("SV8 block smaller than its header".into()));
            }
            let len = usize::try_from(declared - header_len as u64)
                .map_err(|_| MusepackError("SV8 block length overflows usize".into()))?;
            if &key == b"AP" {
                reader.pending_ap = Some(len);
                break;
            }
            if &key == b"SE" {
                return Err(MusepackError("SV8 stream has no audio blocks".into()));
            }
            reader.read_payload(len)?;
            super::sv8::handle_header_block(&mut info, &key, &reader.block_buf)?;
            if &key == b"SH" {
                have_header = true;
            }
        }
        if !have_header {
            return Err(MusepackError(
                "SV8 stream has no usable SH header block".into(),
            ));
        }
        super::sv8::validate(&info)?;
        let channels = info.channels as usize;
        let mut di = [[0.0f32; 16]; 32];
        for (k, row) in DI_OPT_INT.iter().enumerate() {
            for (j, &value) in row.iter().enumerate() {
                di[k][j] = (value as f64 / 65536.0) as f32;
            }
        }
        let scf = build_scf();
        let samples_to_skip = SYNTH_DELAY + info.beg_silence;
        Ok(Self {
            reader,
            info: info.clone(),
            block_frames_left: 0,
            bit_pos: 0,
            channels,
            max_band: info.max_band as i32,
            ms: info.ms,
            block_pwr: info.block_pwr as u32,
            samples: info.samples,
            decoded_samples: 0,
            samples_to_skip,
            last_max_band: 0,
            res_l: [0; MAX_BANDS],
            res_r: [0; MAX_BANDS],
            scf_index_l: [[0; 3]; MAX_BANDS],
            scf_index_r: [[0; 3]; MAX_BANDS],
            scfi_l: [0; MAX_BANDS],
            scfi_r: [0; MAX_BANDS],
            dscf_flag_l: [false; MAX_BANDS],
            dscf_flag_r: [false; MAX_BANDS],
            ms_flag: [false; MAX_BANDS],
            q_l: [[0; 36]; MAX_BANDS],
            q_r: [[0; 36]; MAX_BANDS],
            r1: 1,
            r2: 1,
            y_l: [[0.0; 32]; 36],
            y_r: [[0.0; 32]; 36],
            v_l: vec![0.0; V_SIZE],
            v_r: vec![0.0; V_SIZE],
            scf,
            di,
            cnk: tables::cnk(),
            frame: vec![0.0; FRAME_LENGTH * channels],
            frame_len: 0,
            frame_pos: 0,
            eof: false,
        })
    }

    /// Convenience constructor over an in-memory buffer (tests, embedded use).
    ///
    /// This buffers the caller's bytes; the production decoder seam
    /// ([`super::MusepackDecoder`]) uses [`Self::from_reader`] and never
    /// materialises the whole member.
    pub fn new(bytes: Vec<u8>) -> Result<Self, MusepackError> {
        Self::from_reader(Box::new(std::io::Cursor::new(bytes)))
    }

    /// The parsed stream facts.
    pub fn info(&self) -> &MusepackInfo {
        &self.info
    }

    /// Compressed bytes consumed from the source so far.
    ///
    /// Used by the streaming tests to prove the member is not read whole at
    /// open. It is a diagnostic accessor only.
    pub fn bytes_consumed(&self) -> usize {
        self.reader.consumed
    }

    /// Current size of the single bounded compressed-block buffer (bytes).
    pub fn buffered_block_bytes(&self) -> usize {
        self.reader.block_buf.len()
    }

    /// Decodes up to `out.len() / channels` frames of interleaved `f32`.
    ///
    /// Returns the frames written; `Ok(0)` is end of stream.
    pub fn read_f32(&mut self, out: &mut [f32]) -> Result<usize, MusepackError> {
        let channels = self.channels;
        let wanted = out.len() / channels;
        let mut written = 0usize;
        while written < wanted {
            if self.frame_pos >= self.frame_len && !self.next_frame()? {
                break;
            }
            let available = self.frame_len - self.frame_pos;
            let take = available.min(wanted - written);
            let src = self.frame_pos * channels;
            let dst = written * channels;
            out[dst..dst + take * channels]
                .copy_from_slice(&self.frame[src..src + take * channels]);
            self.frame_pos += take;
            written += take;
        }
        Ok(written)
    }

    // ---- bit reader (absolute MSB-first position) -------------------------

    #[inline]
    fn bit(&self, pos: u64) -> u32 {
        let byte = self
            .reader
            .block_buf
            .get((pos >> 3) as usize)
            .copied()
            .unwrap_or(0);
        ((byte >> (7 - (pos & 7))) & 1) as u32
    }

    fn read_bits(&mut self, n: u32) -> u32 {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | self.bit(self.bit_pos);
            self.bit_pos += 1;
        }
        value
    }

    fn peek16(&self) -> u32 {
        let mut value = 0u32;
        for i in 0..16 {
            value = (value << 1) | self.bit(self.bit_pos + i);
        }
        value
    }

    fn can_dec(&mut self, can: &CanRef) -> i32 {
        let code = self.peek16();
        let mut idx = 0usize;
        while idx + 1 < can.table.len() && code < can.table[idx].0 as u32 {
            idx += 1;
        }
        let (_, length, value) = can.table[idx];
        if length == 0 {
            return 0;
        }
        let delta = (code >> (16 - length)) as i32;
        let sym_index = ((value as i32 - delta) & 0xFF) as usize;
        self.bit_pos += length as u64;
        can.sym[sym_index] as i32
    }

    fn log_dec(&mut self, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        let index = (max - 1) as usize;
        if index >= LOG2.len() {
            return 0;
        }
        let mut value = 0u32;
        if LOG2[index] > 1 {
            value = self.read_bits((LOG2[index] - 1) as u32);
        }
        if value >= LOG2_LOST[index] as u32 {
            value = ((value << 1) | self.read_bits(1)) - LOG2_LOST[index] as u32;
        }
        value
    }

    fn enum_dec(&mut self, k: u32, n: u32) -> u32 {
        if k == 0 || n == 0 || k > 16 || n > 32 {
            return 0;
        }
        let mut bits = 0u32;
        let length = tables::CNK_LEN[(k - 1) as usize][(n - 1) as usize];
        let mut code = self.read_bits(length.saturating_sub(1) as u32);
        if code >= tables::CNK_LOST[(k - 1) as usize][(n - 1) as usize] {
            code = ((code << 1) | self.read_bits(1))
                - tables::CNK_LOST[(k - 1) as usize][(n - 1) as usize];
        }
        let mut kk = k as i32;
        let mut nn = n as i32;
        loop {
            if nn == 0 {
                break;
            }
            nn -= 1;
            let row = &self.cnk[(kk - 1) as usize];
            if code >= row[nn as usize] {
                bits |= 1u32 << nn;
                code -= row[nn as usize];
                kk -= 1;
            }
            if kk == 0 {
                break;
            }
        }
        bits
    }

    fn random_int(&mut self) -> u32 {
        let t3 = self.r1;
        let t4 = self.r2;
        let mut t1 = t3 & 0xF5;
        let mut t2 = t4 >> 25;
        t1 = t1.count_ones() & 1;
        t2 &= 0x63;
        t2 = t2.count_ones() & 1;
        t1 <<= 31;
        self.r1 = (t3 >> 1) | t1;
        self.r2 = t4.wrapping_mul(2) | t2;
        self.r1 ^ self.r2
    }

    // ---- frame loop -------------------------------------------------------

    /// Decodes the next raw frame into `self.frame`; returns `false` at EOF.
    fn next_frame(&mut self) -> Result<bool, MusepackError> {
        if self.frame_pos < self.frame_len {
            return Ok(true);
        }
        if self.eof {
            return Ok(false);
        }
        loop {
            let samples_left =
                self.samples as i64 - self.decoded_samples as i64 + SYNTH_DELAY as i64;
            if samples_left <= 0 && self.samples != 0 {
                self.eof = true;
                return Ok(false);
            }

            let is_key = if self.block_frames_left == 0 {
                if !self.reader.next_ap_block()? {
                    self.eof = true;
                    return Ok(false);
                }
                self.block_frames_left = 1u32 << self.block_pwr;
                self.bit_pos = 0;
                true
            } else {
                false
            };
            self.block_frames_left -= 1;
            self.read_bitstream_sv8(is_key)?;
            let block_bits = self.reader.block_buf.len() as u64 * 8;
            if self.bit_pos > block_bits {
                return Err(MusepackError("SV8 audio block overrun".into()));
            }

            self.decoded_samples += FRAME_LENGTH as u64;
            let frame_samples = if samples_left > FRAME_LENGTH as i64 {
                FRAME_LENGTH
            } else if samples_left < 0 {
                0
            } else {
                samples_left as usize
            };

            if self.samples_to_skip < FRAME_LENGTH as u64 + SYNTH_DELAY {
                self.requantise();
                self.synthesise();
            }

            self.frame_pos = 0;
            if self.samples_to_skip > 0 {
                if frame_samples as u64 <= self.samples_to_skip {
                    self.samples_to_skip -= frame_samples as u64;
                    self.frame_len = 0;
                } else {
                    let skip = self.samples_to_skip as usize;
                    let channels = self.channels;
                    self.frame
                        .copy_within(skip * channels..frame_samples * channels, 0);
                    self.frame_len = frame_samples - skip;
                    self.samples_to_skip = 0;
                }
            } else {
                self.frame_len = frame_samples;
            }

            if self.frame_len > 0 {
                return Ok(true);
            }
        }
    }

    /// `mpc_decoder_read_bitstream_sv8`.
    fn read_bitstream_sv8(&mut self, is_key: bool) -> Result<(), MusepackError> {
        let max_band = self.max_band;
        let max_used_band = if is_key {
            self.log_dec((max_band + 1) as u32) as i32
        } else {
            let mut m = self.last_max_band + self.can_dec(&CAN_BANDS);
            if m > 32 {
                m -= 33;
            }
            m
        };
        if !(0..=32).contains(&max_used_band) {
            return Err(MusepackError("invalid SV8 max-used-band".into()));
        }
        self.last_max_band = max_used_band;

        if max_used_band != 0 {
            let m = (max_used_band - 1) as usize;
            self.res_l[m] = self.can_dec(&CAN_RES[0]);
            self.res_r[m] = self.can_dec(&CAN_RES[0]);
            if self.res_l[m] > 15 {
                self.res_l[m] -= 17;
            }
            if self.res_r[m] > 15 {
                self.res_r[m] -= 17;
            }
            let mut n = max_used_band - 2;
            while n >= 0 {
                let next = (n + 1) as usize;
                let idx = (self.res_l[next] > 2) as usize;
                self.res_l[n as usize] = self.can_dec(&CAN_RES[idx]) + self.res_l[next];
                if self.res_l[n as usize] > 15 {
                    self.res_l[n as usize] -= 17;
                }
                let idx = (self.res_r[next] > 2) as usize;
                self.res_r[n as usize] = self.can_dec(&CAN_RES[idx]) + self.res_r[next];
                if self.res_r[n as usize] > 15 {
                    self.res_r[n as usize] -= 17;
                }
                n -= 1;
            }

            if self.ms {
                let mut tot = 0u32;
                for n in 0..max_used_band as usize {
                    if self.res_l[n] != 0 || self.res_r[n] != 0 {
                        tot += 1;
                    }
                }
                let cnt = self.log_dec(tot);
                let mut tmp = 0u32;
                if cnt != 0 && cnt != tot {
                    tmp = self.enum_dec(cnt.min(tot - cnt), tot);
                }
                if cnt * 2 > tot {
                    tmp = !tmp;
                }
                let mut n = max_used_band - 1;
                while n >= 0 {
                    if self.res_l[n as usize] != 0 || self.res_r[n as usize] != 0 {
                        self.ms_flag[n as usize] = (tmp & 1) != 0;
                        tmp >>= 1;
                    }
                    n -= 1;
                }
            }
        }

        for n in max_used_band..=max_band {
            if n >= 0 && (n as usize) < MAX_BANDS {
                self.res_l[n as usize] = 0;
                self.res_r[n as usize] = 0;
            }
        }

        // SCFI.
        if is_key {
            for n in 0..32 {
                self.dscf_flag_l[n] = true;
                self.dscf_flag_r[n] = true;
            }
        }
        for n in 0..max_used_band as usize {
            let mut cnt: i32 = -1;
            if self.res_l[n] != 0 {
                cnt += 1;
            }
            if self.res_r[n] != 0 {
                cnt += 1;
            }
            if cnt >= 0 {
                let tmp = self.can_dec(&CAN_SCFI[cnt as usize]);
                if self.res_l[n] != 0 {
                    self.scfi_l[n] = tmp >> (2 * cnt);
                }
                if self.res_r[n] != 0 {
                    self.scfi_r[n] = tmp & 3;
                }
            }
        }

        // SCF / DSCF.
        for n in 0..max_used_band as usize {
            for side in 0..2usize {
                let res = if side == 0 {
                    self.res_l[n]
                } else {
                    self.res_r[n]
                };
                if res == 0 {
                    continue;
                }
                let scfi = if side == 0 {
                    self.scfi_l[n]
                } else {
                    self.scfi_r[n]
                };
                let mut scf = if side == 0 {
                    self.scf_index_l[n]
                } else {
                    self.scf_index_r[n]
                };
                let mut dscf = if side == 0 {
                    self.dscf_flag_l[n]
                } else {
                    self.dscf_flag_r[n]
                };
                if dscf {
                    scf[0] = self.read_bits(7) as i32 - 6;
                    dscf = false;
                } else {
                    let mut tmp = self.can_dec(&CAN_DSCF[1]);
                    if tmp == 64 {
                        tmp += self.read_bits(6) as i32;
                    }
                    scf[0] = ((scf[2] - 25 + tmp) & 127) - 6;
                }
                for m in 0..2usize {
                    if (scfi << m) & 2 == 0 {
                        let mut tmp = self.can_dec(&CAN_DSCF[0]);
                        if tmp == 31 {
                            tmp = 64 + self.read_bits(6) as i32;
                        }
                        scf[m + 1] = ((scf[m] - 25 + tmp) & 127) - 6;
                    } else {
                        scf[m + 1] = scf[m];
                    }
                }
                if side == 0 {
                    self.scf_index_l[n] = scf;
                    self.dscf_flag_l[n] = dscf;
                } else {
                    self.scf_index_r[n] = scf;
                    self.dscf_flag_r[n] = dscf;
                }
            }
        }

        // Samples.
        for n in 0..max_used_band as usize {
            for side in 0..2usize {
                let res = if side == 0 {
                    self.res_l[n]
                } else {
                    self.res_r[n]
                };
                if res == 0 {
                    continue;
                }
                if side == 0 {
                    self.read_samples_band(res, n, true);
                } else {
                    self.read_samples_band(res, n, false);
                }
            }
        }
        Ok(())
    }

    fn read_samples_band(&mut self, res: i32, band: usize, left: bool) {
        // Local copy of the quantiser slots, then write back (mirrors the
        // reference's pointer switch over L/R).
        let mut q = [0i16; 36];
        match res {
            -1 => {
                for slot in q.iter_mut() {
                    let tmp = self.random_int();
                    let sum = ((tmp >> 24) & 0xFF) as i32
                        + ((tmp >> 16) & 0xFF) as i32
                        + ((tmp >> 8) & 0xFF) as i32
                        + (tmp & 0xFF) as i32;
                    *slot = (sum - 510) as i16;
                }
            }
            1 => {
                let mut k = 0usize;
                while k < 36 {
                    let kmax = k + 18;
                    let cnt = self.can_dec(&CAN_Q1) as u32;
                    let mut idx = 0u32;
                    if cnt > 0 && cnt < 18 {
                        idx = self.enum_dec(if cnt <= 9 { cnt } else { 18 - cnt }, 18);
                    }
                    if cnt > 9 {
                        idx = !idx;
                    }
                    while k < kmax {
                        q[k] = 0;
                        if (idx & (1 << 17)) != 0 {
                            q[k] = ((self.read_bits(1) << 1) as i16) - 1;
                        }
                        idx <<= 1;
                        k += 1;
                    }
                }
            }
            2 => {
                let thres = 3u32;
                let tables = [&CAN_Q_TABLE[0][0], &CAN_Q_TABLE[0][1]];
                let mut idx = 2 * thres;
                let mut k = 0usize;
                while k < 36 {
                    let tmp = self.can_dec(tables[(idx > thres) as usize]) as usize;
                    q[k] = IDX50[tmp] as i16;
                    q[k + 1] = IDX51[tmp] as i16;
                    q[k + 2] = IDX52[tmp] as i16;
                    idx = (idx >> 1) + HUFF_Q2_VAR[tmp] as u32;
                    k += 3;
                }
            }
            3 | 4 => {
                let table = &CAN_Q_TABLE[1][(res - 3) as usize];
                let mut k = 0usize;
                while k < 36 {
                    let sym = self.can_dec(table) as i16;
                    q[k] = sign4(sym & 0xF);
                    q[k + 1] = sign4((sym >> 4) & 0xF);
                    k += 2;
                }
            }
            5..=8 => {
                let thres = [0u32, 0, 3, 0, 0, 1, 3, 4, 8][res as usize];
                let tables = [
                    &CAN_Q_TABLE[(res - 3) as usize][0],
                    &CAN_Q_TABLE[(res - 3) as usize][1],
                ];
                let mut idx = 2 * thres;
                for slot in q.iter_mut() {
                    *slot = self.can_dec(tables[(idx > thres) as usize]) as i16;
                    idx = (idx >> 1) + abs_i16(*slot) as u32;
                }
            }
            9..=17 => {
                for slot in q.iter_mut() {
                    // Reference: `(unsigned char) mpc_bits_can_dec(...)`.
                    let mut value = (self.can_dec(&CAN_Q9UP) as u8) as i32;
                    if res != 9 {
                        value = (value << (res - 9)) | self.read_bits((res - 9) as u32) as i32;
                    }
                    value -= DC[res as usize];
                    *slot = value as i16;
                }
            }
            _ => {}
        }
        if left {
            self.q_l[band] = q;
        } else {
            self.q_r[band] = q;
        }
    }

    /// `mpc_decoder_requantisierung`.
    fn requantise(&mut self) {
        let last_band = self.max_band as usize;
        for band in 0..=last_band {
            if band >= MAX_BANDS {
                break;
            }
            let ms = self.ms_flag[band];
            let res_l = self.res_l[band];
            let res_r = self.res_r[band];
            let q_l = self.q_l[band];
            let q_r = self.q_r[band];
            let scf_l = self.scf_index_l[band];
            let scf_r = self.scf_index_r[band];
            for n in 0..36 {
                let group = n / 12;
                let (yl, yr);
                if ms {
                    if res_l != 0 {
                        let fac_l = cc(res_l) * self.scf[(scf_l[group] & 0xFF) as usize];
                        if res_r != 0 {
                            let fac_r = cc(res_r) * self.scf[(scf_r[group] & 0xFF) as usize];
                            let tl = fac_l * q_l[n] as f32;
                            let tr = fac_r * q_r[n] as f32;
                            yl = tl + tr;
                            yr = tl - tr;
                        } else {
                            yl = fac_l * q_l[n] as f32;
                            yr = yl;
                        }
                    } else if res_r != 0 {
                        let fac_r = cc(res_r) * self.scf[(scf_r[group] & 0xFF) as usize];
                        yl = fac_r * q_r[n] as f32;
                        yr = -yl;
                    } else {
                        yl = 0.0;
                        yr = 0.0;
                    }
                } else if res_l != 0 {
                    let fac_l = cc(res_l) * self.scf[(scf_l[group] & 0xFF) as usize];
                    if res_r != 0 {
                        let fac_r = cc(res_r) * self.scf[(scf_r[group] & 0xFF) as usize];
                        yl = fac_l * q_l[n] as f32;
                        yr = fac_r * q_r[n] as f32;
                    } else {
                        yl = fac_l * q_l[n] as f32;
                        yr = 0.0;
                    }
                } else if res_r != 0 {
                    let fac_r = cc(res_r) * self.scf[(scf_r[group] & 0xFF) as usize];
                    yl = 0.0;
                    yr = fac_r * q_r[n] as f32;
                } else {
                    yl = 0.0;
                    yr = 0.0;
                }
                self.y_l[n][band] = yl;
                self.y_r[n][band] = yr;
            }
        }
    }

    /// `mpc_synthese_filter_float_scalar`.
    fn synthesise(&mut self) {
        self.v_l.copy_within(0..960, V_MEM);
        synth_channel(
            &mut self.frame,
            &mut self.v_l,
            &self.y_l,
            self.channels,
            0,
            &self.di,
        );
        if self.channels > 1 {
            self.v_r.copy_within(0..960, V_MEM);
            synth_channel(
                &mut self.frame,
                &mut self.v_r,
                &self.y_r,
                self.channels,
                1,
                &self.di,
            );
        }
    }
}

/// `Cc[Res]` with the reference's negative-index padding element: `Cc = __Cc
/// + 1`, so `Cc[-1]` is the leading `111.2859…` entry. Valid streams use
/// `Res` in `-1..=17`.
fn cc(res: i32) -> f32 {
    if res == -1 {
        111.285_962_475_327f32
    } else if (0..=17).contains(&res) {
        tables::CC[res as usize]
    } else {
        0.0
    }
}

fn abs_i16(value: i16) -> i16 {
    value.wrapping_abs()
}

/// Sign-extend the low 4 bits of `nibble`.
fn sign4(nibble: i16) -> i16 {
    let n = nibble & 0xF;
    if n & 0x8 != 0 { n - 16 } else { n }
}

/// `mpc_decoder_scale_output(d, 1.0)` — the 256-entry scalefactor table.
fn build_scf() -> [f32; 256] {
    let mut scf = [0.0f32; 256];
    let factor = 1.0f64 / (1u64 << (16 - 1)) as f64;
    let mut f1 = factor;
    let mut f2 = factor;
    scf[1] = factor as f32;
    let up = 0.8329806647658267f64;
    f1 *= up;
    f2 *= 1.0 / up;
    for n in 1..=128i32 {
        scf[(1 + n) as usize] = f1 as f32;
        scf[((1 - n) & 0xFF) as usize] = f2 as f32;
        f1 *= up;
        f2 *= 1.0 / up;
    }
    scf
}

/// `mpc_compute_new_V` — the 32-point inverse transform's first stage.
fn compute_new_v(p_sample: &[f32; 32], t: &mut [f32; 64]) {
    let mut a = [0f32; 16];
    let mut b = [0f32; 16];

    for i in 0..16 {
        a[i] = p_sample[i] + p_sample[31 - i];
    }

    b[0] = a[0] + a[15];
    b[1] = a[1] + a[14];
    b[2] = a[2] + a[13];
    b[3] = a[3] + a[12];
    b[4] = a[4] + a[11];
    b[5] = a[5] + a[10];
    b[6] = a[6] + a[9];
    b[7] = a[7] + a[8];
    b[8] = (a[0] - a[15]) * 0.5024192929f32;
    b[9] = (a[1] - a[14]) * 0.5224986076f32;
    b[10] = (a[2] - a[13]) * 0.5669440627f32;
    b[11] = (a[3] - a[12]) * 0.6468217969f32;
    b[12] = (a[4] - a[11]) * 0.7881546021f32;
    b[13] = (a[5] - a[10]) * 1.0606776476f32;
    b[14] = (a[6] - a[9]) * 1.7224471569f32;
    b[15] = (a[7] - a[8]) * 5.1011486053f32;

    a[0] = b[0] + b[7];
    a[1] = b[1] + b[6];
    a[2] = b[2] + b[5];
    a[3] = b[3] + b[4];
    a[4] = (b[0] - b[7]) * 0.5097956061f32;
    a[5] = (b[1] - b[6]) * 0.6013448834f32;
    a[6] = (b[2] - b[5]) * 0.8999761939f32;
    a[7] = (b[3] - b[4]) * 2.5629155636f32;
    a[8] = b[8] + b[15];
    a[9] = b[9] + b[14];
    a[10] = b[10] + b[13];
    a[11] = b[11] + b[12];
    a[12] = (b[8] - b[15]) * 0.5097956061f32;
    a[13] = (b[9] - b[14]) * 0.6013448834f32;
    a[14] = (b[10] - b[13]) * 0.8999761939f32;
    a[15] = (b[11] - b[12]) * 2.5629155636f32;

    b[0] = a[0] + a[3];
    b[1] = a[1] + a[2];
    b[2] = (a[0] - a[3]) * 0.5411961079f32;
    b[3] = (a[1] - a[2]) * 1.3065630198f32;
    b[4] = a[4] + a[7];
    b[5] = a[5] + a[6];
    b[6] = (a[4] - a[7]) * 0.5411961079f32;
    b[7] = (a[5] - a[6]) * 1.3065630198f32;
    b[8] = a[8] + a[11];
    b[9] = a[9] + a[10];
    b[10] = (a[8] - a[11]) * 0.5411961079f32;
    b[11] = (a[9] - a[10]) * 1.3065630198f32;
    b[12] = a[12] + a[15];
    b[13] = a[13] + a[14];
    b[14] = (a[12] - a[15]) * 0.5411961079f32;
    b[15] = (a[13] - a[14]) * 1.3065630198f32;

    a[0] = b[0] + b[1];
    a[1] = (b[0] - b[1]) * 0.7071067691f32;
    a[2] = b[2] + b[3];
    a[3] = (b[2] - b[3]) * 0.7071067691f32;
    a[4] = b[4] + b[5];
    a[5] = (b[4] - b[5]) * 0.7071067691f32;
    a[6] = b[6] + b[7];
    a[7] = (b[6] - b[7]) * 0.7071067691f32;
    a[8] = b[8] + b[9];
    a[9] = (b[8] - b[9]) * 0.7071067691f32;
    a[10] = b[10] + b[11];
    a[11] = (b[10] - b[11]) * 0.7071067691f32;
    a[12] = b[12] + b[13];
    a[13] = (b[12] - b[13]) * 0.7071067691f32;
    a[14] = b[14] + b[15];
    a[15] = (b[14] - b[15]) * 0.7071067691f32;

    t[48] = -a[0];
    t[0] = a[1];
    t[8] = a[3];
    t[40] = -a[2] - t[8];
    t[12] = a[7];
    t[4] = a[5] + t[12];
    t[36] = -(t[4] + a[6]);
    t[44] = -a[4] - a[6] - a[7];
    t[14] = a[15];
    t[10] = a[11] + t[14];
    t[6] = t[10] + a[13];
    t[2] = a[9] + a[13] + a[15];
    t[34] = -t[2] - a[14];
    t[38] = t[34] + a[9] - a[10] - a[11];
    let tmp = -(a[12] + a[14] + a[15]);
    t[46] = tmp - a[8];
    t[42] = tmp - a[10] - a[11];

    a[0] = (p_sample[0] - p_sample[31]) * 0.5006030202f32;
    a[1] = (p_sample[1] - p_sample[30]) * 0.5054709315f32;
    a[2] = (p_sample[2] - p_sample[29]) * 0.5154473186f32;
    a[3] = (p_sample[3] - p_sample[28]) * 0.5310425758f32;
    a[4] = (p_sample[4] - p_sample[27]) * 0.5531039238f32;
    a[5] = (p_sample[5] - p_sample[26]) * 0.5829349756f32;
    a[6] = (p_sample[6] - p_sample[25]) * 0.6225041151f32;
    a[7] = (p_sample[7] - p_sample[24]) * 0.6748083234f32;
    a[8] = (p_sample[8] - p_sample[23]) * 0.7445362806f32;
    a[9] = (p_sample[9] - p_sample[22]) * 0.8393496275f32;
    a[10] = (p_sample[10] - p_sample[21]) * 0.9725682139f32;
    a[11] = (p_sample[11] - p_sample[20]) * 1.1694399118f32;
    a[12] = (p_sample[12] - p_sample[19]) * 1.4841645956f32;
    a[13] = (p_sample[13] - p_sample[18]) * 2.0577809811f32;
    a[14] = (p_sample[14] - p_sample[17]) * 3.4076085091f32;
    a[15] = (p_sample[15] - p_sample[16]) * 10.1900081635f32;

    b[0] = a[0] + a[15];
    b[1] = a[1] + a[14];
    b[2] = a[2] + a[13];
    b[3] = a[3] + a[12];
    b[4] = a[4] + a[11];
    b[5] = a[5] + a[10];
    b[6] = a[6] + a[9];
    b[7] = a[7] + a[8];
    b[8] = (a[0] - a[15]) * 0.5024192929f32;
    b[9] = (a[1] - a[14]) * 0.5224986076f32;
    b[10] = (a[2] - a[13]) * 0.5669440627f32;
    b[11] = (a[3] - a[12]) * 0.6468217969f32;
    b[12] = (a[4] - a[11]) * 0.7881546021f32;
    b[13] = (a[5] - a[10]) * 1.0606776476f32;
    b[14] = (a[6] - a[9]) * 1.7224471569f32;
    b[15] = (a[7] - a[8]) * 5.1011486053f32;

    a[0] = b[0] + b[7];
    a[1] = b[1] + b[6];
    a[2] = b[2] + b[5];
    a[3] = b[3] + b[4];
    a[4] = (b[0] - b[7]) * 0.5097956061f32;
    a[5] = (b[1] - b[6]) * 0.6013448834f32;
    a[6] = (b[2] - b[5]) * 0.8999761939f32;
    a[7] = (b[3] - b[4]) * 2.5629155636f32;
    a[8] = b[8] + b[15];
    a[9] = b[9] + b[14];
    a[10] = b[10] + b[13];
    a[11] = b[11] + b[12];
    a[12] = (b[8] - b[15]) * 0.5097956061f32;
    a[13] = (b[9] - b[14]) * 0.6013448834f32;
    a[14] = (b[10] - b[13]) * 0.8999761939f32;
    a[15] = (b[11] - b[12]) * 2.5629155636f32;

    b[0] = a[0] + a[3];
    b[1] = a[1] + a[2];
    b[2] = (a[0] - a[3]) * 0.5411961079f32;
    b[3] = (a[1] - a[2]) * 1.3065630198f32;
    b[4] = a[4] + a[7];
    b[5] = a[5] + a[6];
    b[6] = (a[4] - a[7]) * 0.5411961079f32;
    b[7] = (a[5] - a[6]) * 1.3065630198f32;
    b[8] = a[8] + a[11];
    b[9] = a[9] + a[10];
    b[10] = (a[8] - a[11]) * 0.5411961079f32;
    b[11] = (a[9] - a[10]) * 1.3065630198f32;
    b[12] = a[12] + a[15];
    b[13] = a[13] + a[14];
    b[14] = (a[12] - a[15]) * 0.5411961079f32;
    b[15] = (a[13] - a[14]) * 1.3065630198f32;

    a[0] = b[0] + b[1];
    a[1] = (b[0] - b[1]) * 0.7071067691f32;
    a[2] = b[2] + b[3];
    a[3] = (b[2] - b[3]) * 0.7071067691f32;
    a[4] = b[4] + b[5];
    a[5] = (b[4] - b[5]) * 0.7071067691f32;
    a[6] = b[6] + b[7];
    a[7] = (b[6] - b[7]) * 0.7071067691f32;
    a[8] = b[8] + b[9];
    a[9] = (b[8] - b[9]) * 0.7071067691f32;
    a[10] = b[10] + b[11];
    a[11] = (b[10] - b[11]) * 0.7071067691f32;
    a[12] = b[12] + b[13];
    a[13] = (b[12] - b[13]) * 0.7071067691f32;
    a[14] = b[14] + b[15];
    a[15] = (b[14] - b[15]) * 0.7071067691f32;

    t[15] = a[15];
    let inner = a[7] + t[15];
    t[13] = inner;
    t[11] = inner + a[11];
    t[5] = t[11] + a[5] + a[13];
    t[9] = a[3] + a[11] + a[15];
    t[7] = t[9] + a[13];
    t[1] = a[1] + a[9] + a[13] + a[15];
    t[33] = -t[1] - a[14];
    t[3] = a[5] + a[7] + a[9] + a[13] + a[15];
    t[35] = -t[3] - a[6] - a[14];
    let mut tmp2 = -(a[10] + a[11] + a[13] + a[14] + a[15]);
    t[37] = tmp2 - a[5] - a[6] - a[7];
    t[39] = tmp2 - a[2] - a[3];
    tmp2 += a[13] - a[12];
    t[41] = tmp2 - a[2] - a[3];
    t[43] = tmp2 - a[4] - a[6] - a[7];
    tmp2 = -(a[8] + a[12] + a[14] + a[15]);
    t[47] = tmp2 - a[0];
    t[45] = tmp2 - a[4] - a[6] - a[7];

    for i in 0..16 {
        t[32 - i] = -t[i];
    }
    for i in 0..15 {
        t[63 - i] = t[33 + i];
    }
}

/// One channel of `mpc_synthese_filter_float_internal`.
fn synth_channel(
    out: &mut [f32],
    v: &mut [f32],
    y: &[[f32; 32]; 36],
    channels: usize,
    channel: usize,
    di: &[[f32; 16]; 32],
) {
    let mut pv = V_MEM;
    let mut t = [0f32; 64];
    for (n, samples) in y.iter().enumerate() {
        pv -= 64;
        compute_new_v(samples, &mut t);
        v[pv..pv + 64].copy_from_slice(&t);
        let mut out_index = n * 32 * channels + channel;
        for (k, row) in di.iter().enumerate() {
            let base = pv + k;
            let mut acc = 0f32;
            for j in 0..16 {
                acc += v[base + SYNTH_OFFSETS[j]] * row[j];
            }
            out[out_index] = acc;
            out_index += channels;
        }
    }
}

/// `HuffQ2_var` (quantiser-2 transition table).
static HUFF_Q2_VAR: [i8; 125] = [
    6, 5, 4, 5, 6, 5, 4, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3, 4, 5, 6, 5, 4, 5, 6, 5, 4, 3, 4, 5, 4, 3,
    2, 3, 4, 3, 2, 1, 2, 3, 4, 3, 2, 3, 4, 5, 4, 3, 4, 5, 4, 3, 2, 3, 4, 3, 2, 1, 2, 3, 2, 1, 0, 1,
    2, 3, 2, 1, 2, 3, 4, 3, 2, 3, 4, 5, 4, 3, 4, 5, 4, 3, 2, 3, 4, 3, 2, 1, 2, 3, 4, 3, 2, 3, 4, 5,
    4, 3, 4, 5, 6, 5, 4, 5, 6, 5, 4, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3, 4, 5, 6, 5, 4, 5, 6,
];

static IDX50: [i8; 125] = [
    -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0,
    1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2,
    -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1,
    2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2, -2, -1,
    0, 1, 2, -2, -1, 0, 1, 2, -2, -1, 0, 1, 2,
];

static IDX51: [i8; 125] = [
    -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, -2, -2,
    -2, -2, -2, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, -2, -2, -2, -2,
    -2, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, -2, -2, -2, -2, -2, -1,
    -1, -1, -1, -1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, -2, -2, -2, -2, -2, -1, -1, -1,
    -1, -1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2,
];

static IDX52: [i8; 125] = [
    -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2,
    -2, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    -1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
];

/// `can_Q[6][2]` reachable without borrowing the whole table.
static CAN_Q_TABLE: [[CanRef; 2]; 6] = [
    [
        CanRef {
            table: tables::HUFF_Q2_1,
            sym: tables::SYM_Q2_1,
        },
        CanRef {
            table: tables::HUFF_Q2_2,
            sym: tables::SYM_Q2_2,
        },
    ],
    [
        CanRef {
            table: tables::HUFF_Q3,
            sym: tables::SYM_Q3,
        },
        CanRef {
            table: tables::HUFF_Q4,
            sym: tables::SYM_Q4,
        },
    ],
    [
        CanRef {
            table: tables::HUFF_Q5_1,
            sym: tables::SYM_Q5_1,
        },
        CanRef {
            table: tables::HUFF_Q5_2,
            sym: tables::SYM_Q5_2,
        },
    ],
    [
        CanRef {
            table: tables::HUFF_Q6_1,
            sym: tables::SYM_Q6_1,
        },
        CanRef {
            table: tables::HUFF_Q6_2,
            sym: tables::SYM_Q6_2,
        },
    ],
    [
        CanRef {
            table: tables::HUFF_Q7_1,
            sym: tables::SYM_Q7_1,
        },
        CanRef {
            table: tables::HUFF_Q7_2,
            sym: tables::SYM_Q7_2,
        },
    ],
    [
        CanRef {
            table: tables::HUFF_Q8_1,
            sym: tables::SYM_Q8_1,
        },
        CanRef {
            table: tables::HUFF_Q8_2,
            sym: tables::SYM_Q8_2,
        },
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/musepack/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn bit_reader_is_msb_first_across_byte_boundaries() {
        let mut d = MpcDecoder::new(fixture("sine32-q8.mpc")).unwrap();
        assert!(d.reader.next_ap_block().unwrap(), "first audio block loads");
        let buf = d.reader.block_buf.clone();
        assert_eq!(d.bit_pos, 0);
        assert_eq!(d.read_bits(8), buf[0] as u32);
        assert_eq!(d.read_bits(4), (buf[1] >> 4) as u32);
        assert_eq!(d.read_bits(4), (buf[1] & 0x0F) as u32);
        assert_eq!(d.read_bits(16), ((buf[2] as u32) << 8) | buf[3] as u32);
        // `peek16` does not advance the position.
        let before = d.bit_pos;
        let peeked = d.peek16();
        assert_eq!(d.bit_pos, before);
        assert_eq!(peeked, ((buf[4] as u32) << 8) | buf[5] as u32);
    }

    #[test]
    fn bit_reader_past_end_is_zero_and_terminates() {
        let mut d = MpcDecoder::new(fixture("sine32-q8.mpc")).unwrap();
        assert!(d.reader.next_ap_block().unwrap());
        d.bit_pos = (d.reader.block_buf.len() as u64) * 8 + 100;
        assert_eq!(d.read_bits(32), 0);
        assert_eq!(d.peek16(), 0);
    }
}
