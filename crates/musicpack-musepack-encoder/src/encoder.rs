//! Full PCM → Musepack SV8 encoder (Phase 15G).
//!
//! This assembles the independently-verified components into the reference
//! frame loop (`codec/mpcenc/mpcenc.c::mainloop`). The order below is the
//! actual reference-compatible execution order, not a conceptual ideal: in
//! particular `SCF_Extraktion` precedes transient/NS analysis, and
//! `writeBitstream_SV8` runs even for digital-silence frames.
//!
//! The observable order is:
//!
//! ```text
//! Analyse_Filter
//!   → Psychoakustisches_Modell
//!   → RaiseSMR (if minSMR > 0)
//!   → MS_LR_Entscheidung (if MS_Channelmode > 0)
//!   → SCF_Extraktion
//!   → TransientenCalc
//!   → NS_Analyse (if NS_Order > 0)
//!   → Allocate (L, then R)
//!   → Quantisierung
//!   → writeBitstream_SV8
//! ```
//!
//! Note the exact ordering: `SCF_Extraktion` runs **before** `TransientenCalc`
//! and `NS_Analyse`, and `TransientenCalc` before `NS_Analyse`; `MS_LR`
//! rewrites the subband samples before `SCF_Extraktion`; `writeBitstream_SV8`
//! runs even when the frame was skipped as digital silence.
//!
//! All state is per-instance. See `GERMAN_TERMINOLOGY.md` and
//! `PSYCHOACOUSTIC_CONTRACT.md` for the source mapping.

use crate::blocks::{EncoderInfo, GainInfo, StreamInfo, Sv8StreamWriter};
use crate::coding::ans::{Anspec, Smr as CodingSmr};
use crate::coding::frame::FrameEncoder;
use crate::coding::quant::Quantizer;
use crate::coding::{ScfState, allocate, ns_analyse, scf_extraktion};
use crate::error::EncoderError;
use crate::filterbank::{AnalysisFilterbank, Subband};
use crate::psy::{self, PcmFrame, PsyParams, PsychoacousticModel};

/// Samples per frame (`MPC_FRAME_LENGTH`).
pub const FRAME_LENGTH: usize = 1152;
/// Analysis buffer length (`ANABUFFER`).
pub const ANABUFFER: usize = 1600;
/// PCM centring offset (`CENTER`).
pub const CENTER: usize = 448;
/// Decoder delay flushed at the end (`DECODER_DELAY = 512 - 32 + 1`).
pub const DECODER_DELAY: u64 = 481;
/// Reference `MPPENC_DENORMAL_FIX_BASE`, computed in `f64` as the reference
/// macro is.
const DENORMAL_FIX_BASE: f64 = 32.0 * 1024.0 / (1u32 << 24) as f64;
const DENORMAL_FIX_LEFT: f64 = DENORMAL_FIX_BASE;
const DENORMAL_FIX_RIGHT: f64 = DENORMAL_FIX_BASE * 0.5;

/// Subbands.
const SUBBANDS: usize = 32;
/// Samples per subband.
const SAMPLES: usize = 36;

/// Full-scale divisor for wide PCM: `2^16`.
///
/// A left-aligned `i32` sample divided by this lands on the encoder's
/// ±32768 sample scale (the scale 16-bit input already has natively), so
/// 16-, 24- and 32-bit sources share one conversion. The division is
/// evaluated in `f64` — where the exact quotient of a 32-bit integer and
/// `65536.0` is representable — and then rounded to `f32` exactly once,
/// which is the reference conversion's rounding point (`f24`/`f32` return
/// `float` from an exact `double` expression).
const FULL_SCALE_DIVISOR: f64 = 65536.0;

/// The interleaved PCM slice of one `encode*` call, in either supported
/// input representation.
///
/// Both variants convert per sample to the same reference `f32` value on
/// the ±32768 scale; the codec pipeline downstream is representation-
/// agnostic.
#[derive(Clone, Copy)]
enum PcmSource<'a> {
    /// Native interleaved 16-bit samples ([`MusepackEncoder::encode`]).
    Bits16(&'a [i16]),
    /// Full-scale left-aligned 32-bit samples
    /// ([`MusepackEncoder::encode_s32`]).
    FullScale32(&'a [i32]),
}

impl PcmSource<'_> {
    /// Total number of interleaved samples (frames × channels).
    fn len(&self) -> usize {
        match self {
            Self::Bits16(pcm) => pcm.len(),
            Self::FullScale32(pcm) => pcm.len(),
        }
    }

    /// One interleaved sample as `(value, nonzero)`.
    ///
    /// `value` is the sample on the encoder's ±32768 scale **before** the
    /// denormal-fix constant is added; `nonzero` is the raw-value test the
    /// reference's `DigitalSilence` performs on the sample bytes (a sample
    /// value is zero if and only if its little-endian bytes are zero for
    /// every supported integer width).
    ///
    /// * `Bits16`: the 16-bit value is exact in `f32`, so no rounding can
    ///   occur here — identical to the reference `b[0] * scalel` with the
    ///   default `scalel = 1.0f`.
    /// * `FullScale32`: `v / 65536.0` in `f64` (exact — see
    ///   [`FULL_SCALE_DIVISOR`]) rounded once to `f32`. For a 16-bit source
    ///   left-aligned as `v = s16 << 16` this equals `s16` exactly; for
    ///   24-bit (`v = s24 << 8`) it equals the reference `f24`
    ///   conversion; for 32-bit it equals the reference `f32` conversion.
    fn sample(&self, index: usize) -> (f32, bool) {
        match self {
            Self::Bits16(pcm) => {
                let s = pcm[index];
                (f32::from(s), s != 0)
            }
            Self::FullScale32(pcm) => {
                let v = pcm[index];
                (((v as f64) / FULL_SCALE_DIVISOR) as f32, v != 0)
            }
        }
    }
}

/// One frame of the diagnostic trace (see [`MusepackEncoder::encode_traced`]).
#[derive(Clone, Debug)]
pub struct FrameTrace {
    /// Frame index.
    pub index: u64,
    /// Frames read for this iteration.
    pub current_read: usize,
    /// Digital-silence flag for this iteration.
    pub silence: bool,
    /// Whether the frame was processed (not skipped as digital silence).
    pub processed: bool,
    /// `Res_L`.
    pub res_l: [i32; SUBBANDS],
    /// `Res_R`.
    pub res_r: [i32; SUBBANDS],
    /// `SCF_Index_L` after allocation.
    pub scf_l: [[i32; 3]; SUBBANDS],
    /// `SCF_Index_R` after allocation.
    pub scf_r: [[i32; 3]; SUBBANDS],
    /// `MS_Flag`.
    pub ms_flag: [u8; SUBBANDS],
    /// Post-allocation subband samples (L).
    pub x_l: [[f32; SAMPLES]; SUBBANDS],
    /// Post-allocation subband samples (R).
    pub x_r: [[f32; SAMPLES]; SUBBANDS],
    /// `SMR.L`.
    pub smr_l: [f32; SUBBANDS],
    /// `SMR.R`.
    pub smr_r: [f32; SUBBANDS],
    /// The analysis window (L) used for this frame.
    pub main_l: Vec<f32>,
    /// The analysis window (R) used for this frame.
    pub main_r: Vec<f32>,
    /// Raw filterbank output (L), before M/S rewriting.
    pub x_raw_l: [[f32; SAMPLES]; SUBBANDS],
    /// Raw filterbank output (R), before M/S rewriting.
    pub x_raw_r: [[f32; SAMPLES]; SUBBANDS],
    /// Post-M/S subband samples (L), before SCF.
    pub x_ms_l: [[f32; SAMPLES]; SUBBANDS],
    /// Post-M/S subband samples (R), before SCF.
    pub x_ms_r: [[f32; SAMPLES]; SUBBANDS],
    /// `ANSspec.L` after the model call.
    pub ans_l: [f32; 512],
    /// Tracked loudness (`state.loud`).
    pub loud: f32,
    /// First eight `Xsave_L` values.
    pub xsave_l: [f32; 8],
    /// First eight `PreThr_L` values.
    pub pre_thr_l: [f32; 8],
    /// First eight `tmp_Mask_L` values.
    pub tmp_mask_l: [f32; 8],
    /// First four short-time integrators `a`.
    pub int_a: [f32; 4],
    /// First four long-time integrators `b`.
    pub int_b: [f32; 4],
    /// Window-1 FFT power spectrum (L).
    pub erg_l: [f32; 512],
    /// Window-1 partition energy (L).
    pub ls_l: [f32; 57],
    /// Window-1 FFT phase (L).
    pub phs_l: [f32; 512],
    /// First sixteen CVD voice-line values.
    pub vocal_l: [i32; 16],
    /// Window-2 FFT power spectrum (L).
    pub erg2_l: [f32; 512],
    /// Window-1 unpredictability (L).
    pub cw_l: [f32; 512],
    /// Short-partition transient flags (L).
    pub transient_l: [i32; 19],
    /// Window-1 weighted partition energy (L).
    pub cls_l: [f32; 57],
}

/// Outcome of one processed frame: `Res_L`, `Res_R`, and the frame's
/// diagnostic trace when collection was requested.
type FrameOutcome = ([i32; SUBBANDS], [i32; SUBBANDS], Option<FrameTrace>);

/// Encoder configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncoderConfig {
    /// Quality (`0..=10`).
    pub quality: f32,
    /// Sample rate (44100, 48000, 37800 or 32000).
    pub sample_rate: u32,
    /// Channels (1 or 2).
    pub channels: u32,
    /// `frames_per_block_pwr` (default 6).
    pub frames_per_block_pwr: u32,
    /// `seek_pwr` (`SeekDistance`, default 1).
    pub seek_pwr: u32,
}

impl EncoderConfig {
    /// A configuration with the reference defaults (`frames_per_block_pwr = 6`,
    /// `seek_pwr = 1`).
    #[must_use]
    pub fn new(quality: f32, sample_rate: u32, channels: u32) -> Self {
        Self {
            quality,
            sample_rate,
            channels,
            frames_per_block_pwr: 6,
            seek_pwr: 1,
        }
    }
}

/// The full encoder for one stream.
///
/// All mutable state is owned per instance; there is no mutable global state
/// anywhere in the crate (all tables are `const`). State lifetimes mirror the
/// reference exactly:
///
/// * **stream lifetime:** filterbank history, psychoacoustic state (FFT/CVD
///   history, temporal/masking state), SCF cross-frame state (`SCF_Index`),
///   quantiser state (`Q`);
/// * **frame lifetime:** `Res`, allocation outputs, `MS_Flag`, the subband
///   matrix, frame-local coding state and frame-local diagnostic state;
/// * **AP-block lifetime** (owned by [`FrameEncoder`](crate::coding::frame)):
///   the AP bit buffer, frame count, `SCF_Last`, `DSCF_Flag`.
///
/// Silence/position accounting (`silence`, `old_silence`, `current_read`,
/// `all_samples_read`, `samples_in_wave`) and container/seek accounting
/// (`block_cnt`, `seek_pos`, `seek_entries`, `Sv8StreamWriter`) are likewise
/// per-instance stream state.
pub struct MusepackEncoder {
    config: EncoderConfig,
    params: PsyParams,
    max_band: usize,
    filterbank: AnalysisFilterbank,
    psy: PsychoacousticModel,
    scf: ScfState,
    quant: Quantizer,
    frame: FrameEncoder,
    stream: Sv8StreamWriter,
    main_l: Vec<f32>,
    main_r: Vec<f32>,
    main_m: Vec<f32>,
    main_s: Vec<f32>,
    samples_in_wave: u64,
    all_samples_read: u64,
    current_read: usize,
    silence: bool,
    old_silence: bool,
    block_cnt: u32,
    seek_pos: u32,
    seek_entries: Vec<u64>,
    x: [Subband; SUBBANDS],
    ms_flag: [u8; SUBBANDS],
}

impl MusepackEncoder {
    /// Creates an encoder for the given configuration.
    ///
    /// Validation mirrors the reference where the reference validates, and
    /// fails closed where an accepted value would produce an inconsistent
    /// stream:
    ///
    /// * `frames_per_block_pwr` must be even and `<= 14`. `SH` stores
    ///   `frames_per_block_pwr >> 1` (three bits, log4), so only even powers
    ///   `0..=14` are representable; the reference CLI only ever produces
    ///   even values (`--num_frames x` sets `2x`). Odd values are rejected
    ///   (`EncoderError::InvalidBlockPower`) instead of silently emitting a
    ///   header that disagrees with the actual `AP` block size.
    /// * `seek_pwr > 15` is reset to `1`, exactly like the reference
    ///   `mpc_encoder_init` (`if (SeekDistance > 15) SeekDistance = 1;`),
    ///   rather than being written into the four-bit `ST` field truncated or
    ///   panicking on the seek-entry shift.
    pub fn new(config: EncoderConfig) -> Result<Self, EncoderError> {
        if !config.quality.is_finite() {
            return Err(EncoderError::NonFiniteQuality(config.quality));
        }
        if config.frames_per_block_pwr > 14 || config.frames_per_block_pwr % 2 == 1 {
            return Err(EncoderError::InvalidBlockPower(config.frames_per_block_pwr));
        }
        let mut config = config;
        if config.seek_pwr > 15 {
            config.seek_pwr = 1;
        }
        let params = PsyParams::from_quality(config.quality);
        let psy = PsychoacousticModel::new(config.quality, config.sample_rate as f32)?;
        let max_band = psy.max_band() as usize;
        Ok(Self {
            config,
            params,
            max_band,
            filterbank: AnalysisFilterbank::new(),
            psy,
            scf: ScfState::default(),
            quant: Quantizer::new(),
            frame: FrameEncoder::new(config.frames_per_block_pwr),
            stream: Sv8StreamWriter::with_seek_ref(0),
            main_l: vec![0.0; ANABUFFER],
            main_r: vec![0.0; ANABUFFER],
            main_m: vec![0.0; ANABUFFER],
            main_s: vec![0.0; ANABUFFER],
            samples_in_wave: 0,
            all_samples_read: 0,
            current_read: 0,
            silence: false,
            old_silence: false,
            block_cnt: 0,
            seek_pos: 0,
            seek_entries: Vec::new(),
            x: [Subband::ZERO; SUBBANDS],
            ms_flag: [0; SUBBANDS],
        })
    }

    /// The resolved profile parameters.
    #[must_use]
    pub fn params(&self) -> &PsyParams {
        &self.params
    }

    /// The reference `Max_Band` for this configuration.
    #[must_use]
    pub fn max_band(&self) -> usize {
        self.max_band
    }

    /// Encodes interleaved 16-bit PCM into a complete SV8 stream.
    ///
    /// The PCM is interpreted exactly as `Read_WAV_Samples` does for 16-bit
    /// input: `L = sample * 1.0 + fix_left`, `R = sample * 1.0 + fix_right`,
    /// `M = (L+R)*0.5`, `S = (L-R)*0.5`.
    ///
    /// For 24- and 32-bit sources use [`encode_s32`](Self::encode_s32),
    /// which accepts the same content without truncating it to 16 bits.
    ///
    /// This path never constructs diagnostic trace state; see
    /// [`encode_traced`](Self::encode_traced) for the debugging variant.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, EncoderError> {
        self.encode_inner(PcmSource::Bits16(pcm), None)
    }

    /// Encodes and also returns the per-frame diagnostic trace.
    ///
    /// The byte output is identical to [`encode`](Self::encode); collecting
    /// the trace only adds observation copies and never alters numerical
    /// evaluation order, state lifetime, frame ordering, or bitstream output.
    pub fn encode_traced(
        &mut self,
        pcm: &[i16],
    ) -> Result<(Vec<u8>, Vec<FrameTrace>), EncoderError> {
        let mut trace = Vec::new();
        let bytes = self.encode_inner(PcmSource::Bits16(pcm), Some(&mut trace))?;
        Ok((bytes, trace))
    }

    /// Encodes interleaved **full-scale left-aligned 32-bit** PCM into a
    /// complete SV8 stream — the wide-input sibling of
    /// [`encode`](Self::encode).
    ///
    /// Samples are sign-extended source values left-aligned to full scale:
    /// `i32::MIN..=i32::MAX` spans the same ±full-scale range that
    /// `i16::MIN..=i16::MAX` spans for `encode`. This is the format
    /// `musicpack_core::audio`'s `read_s32` produces (16-bit content in
    /// bits 31..16, 24-bit in bits 31..8, 32-bit verbatim), so a decoded
    /// buffer feeds this method directly:
    ///
    /// * **16-bit source:** `s16 as i32 << 16` — conversion is exact and
    ///   produces byte-identical output to `encode`;
    /// * **24-bit source:** `s24 as i32 << 8` — all 24 bits survive: the
    ///   low 8 bits become fractional bits on the ±32768 scale and the
    ///   value is exactly representable in `f32`, so nothing below the
    ///   reference's own `float` precision is lost;
    /// * **32-bit source:** passed verbatim — rounded once to `f32`
    ///   exactly as the reference conversion rounds it.
    ///
    /// Conversion (`v / 65536.0` rounded once to `f32`, then the reference
    /// denormal-fix constant added in `f64` and rounded back to `f32`)
    /// happens here at input time; psychoacoustic and coding stages are
    /// untouched and receive the same `f32` analysis buffers as before.
    ///
    /// This path never constructs diagnostic trace state; see
    /// [`encode_s32_traced`](Self::encode_s32_traced) for the debugging
    /// variant.
    pub fn encode_s32(&mut self, pcm: &[i32]) -> Result<Vec<u8>, EncoderError> {
        self.encode_inner(PcmSource::FullScale32(pcm), None)
    }

    /// Wide-input variant of [`encode_traced`](Self::encode_traced):
    /// byte-identical to [`encode_s32`](Self::encode_s32), with the
    /// per-frame diagnostic trace collected.
    pub fn encode_s32_traced(
        &mut self,
        pcm: &[i32],
    ) -> Result<(Vec<u8>, Vec<FrameTrace>), EncoderError> {
        let mut trace = Vec::new();
        let bytes = self.encode_inner(PcmSource::FullScale32(pcm), Some(&mut trace))?;
        Ok((bytes, trace))
    }

    fn encode_inner(
        &mut self,
        pcm: PcmSource<'_>,
        mut trace: Option<&mut Vec<FrameTrace>>,
    ) -> Result<Vec<u8>, EncoderError> {
        let channels = self.config.channels as usize;
        if channels == 0 || channels > 2 {
            return Err(EncoderError::InvalidChannelCount(self.config.channels));
        }
        let frames = (pcm.len() / channels) as u64;
        self.samples_in_wave = frames;

        self.write_header(frames)?;

        // Pre-loop read and buffer priming (mpcenc lines 1693-1704).
        let (read, silence) = self.read_block(pcm, 0);
        self.current_read = read;
        self.silence = silence;
        self.all_samples_read = read as u64;
        if read > 0 {
            let first_l = self.main_l[CENTER];
            let first_r = self.main_r[CENTER];
            let first_m = self.main_m[CENTER];
            let first_s = self.main_s[CENTER];
            fill(&mut self.main_l, first_l, CENTER);
            fill(&mut self.main_r, first_r, CENTER);
            fill(&mut self.main_m, first_m, CENTER);
            fill(&mut self.main_s, first_s, CENTER);
        }
        self.filterbank.init(
            self.main_l[CENTER],
            self.main_r[CENTER],
            &mut self.x,
            self.max_band,
        )?;

        let mut n: u64 = 0;
        while (n * FRAME_LENGTH as u64) < (self.samples_in_wave + DECODER_DELAY) {
            // Pad a short read with the previous sample (mpcenc line 1715).
            if self.current_read < FRAME_LENGTH && n > 0 {
                let last = CENTER + self.current_read - 1;
                let l = self.main_l[last];
                let r = self.main_r[last];
                let m = self.main_m[last];
                let s = self.main_s[last];
                fill_from(
                    &mut self.main_l,
                    l,
                    CENTER + self.current_read,
                    FRAME_LENGTH - self.current_read,
                );
                fill_from(
                    &mut self.main_r,
                    r,
                    CENTER + self.current_read,
                    FRAME_LENGTH - self.current_read,
                );
                fill_from(
                    &mut self.main_m,
                    m,
                    CENTER + self.current_read,
                    FRAME_LENGTH - self.current_read,
                );
                fill_from(
                    &mut self.main_s,
                    s,
                    CENTER + self.current_read,
                    FRAME_LENGTH - self.current_read,
                );
            }

            let mut res_l = [0i32; SUBBANDS];
            let mut res_r = [0i32; SUBBANDS];
            let mut frame_trace: Option<FrameTrace> = None;

            if !self.silence || !self.old_silence {
                let (rl, rr, ft) = self.process_frame(n, trace.is_some())?;
                res_l = rl;
                res_r = rr;
                frame_trace = ft;
            }
            self.old_silence = self.silence;

            if let Some(t) = trace.as_deref_mut() {
                t.push(frame_trace.unwrap_or_else(|| FrameTrace {
                    index: n,
                    current_read: self.current_read,
                    silence: self.silence,
                    processed: false,
                    res_l,
                    res_r,
                    scf_l: self.scf.scf_l,
                    scf_r: self.scf.scf_r,
                    ms_flag: self.ms_flag,
                    x_l: core::array::from_fn(|b| self.x[b].left),
                    x_r: core::array::from_fn(|b| self.x[b].right),
                    smr_l: [0.0; SUBBANDS],
                    smr_r: [0.0; SUBBANDS],
                    main_l: self.main_l.clone(),
                    main_r: self.main_r.clone(),
                    x_raw_l: core::array::from_fn(|b| self.x[b].left),
                    x_raw_r: core::array::from_fn(|b| self.x[b].right),
                    x_ms_l: core::array::from_fn(|b| self.x[b].left),
                    x_ms_r: core::array::from_fn(|b| self.x[b].right),
                    ans_l: *self.psy.ans_spec_l(),
                    loud: self.psy.loud(),
                    xsave_l: self.psy.xsave_l8(),
                    pre_thr_l: self.psy.pre_thr_l8(),
                    tmp_mask_l: self.psy.tmp_mask_l8(),
                    int_a: self.psy.int_a4(),
                    int_b: self.psy.int_b4(),
                    erg_l: *self.psy.last_erg_l(),
                    ls_l: *self.psy.last_ls_l(),
                    phs_l: *self.psy.last_phs_l(),
                    vocal_l: self.psy.vocal_l16(),
                    erg2_l: *self.psy.last_erg2_l(),
                    cw_l: *self.psy.last_cw_l(),
                    transient_l: [0; 19],
                    cls_l: *self.psy.last_cls_l(),
                }));
            }

            let ms_i32: [i32; SUBBANDS] = core::array::from_fn(|i| self.ms_flag[i] as i32);
            self.frame.encode_frame(
                self.max_band as i32,
                u32::from(self.params.ms_channelmode),
                &res_l,
                &res_r,
                &self.scf.scf_l,
                &self.scf.scf_r,
                &ms_i32,
                self.quant.q_l(),
                self.quant.q_r(),
            )?;
            let blocks = self.frame.take_blocks();
            if !blocks.is_empty() {
                self.flush_ap(&blocks);
            }

            // Advance the analysis buffer (mpcenc lines 1767-1774).
            memmove_left(&mut self.main_l);
            memmove_left(&mut self.main_r);
            memmove_left(&mut self.main_m);
            memmove_left(&mut self.main_s);
            let (read, silence) = self.read_block(pcm, self.all_samples_read);
            self.current_read = read;
            self.silence = silence;
            self.all_samples_read += read as u64;

            n += 1;
        }

        // Final partial AP block (mpcenc lines 1785-1792).
        let tail = self.frame.flush_partial();
        if !tail.is_empty() {
            self.flush_ap(&tail);
        }

        self.stream
            .write_seek_table(self.seek_pos, self.config.seek_pwr, &self.seek_entries)?;
        self.stream.write_end()?;
        Ok(self.stream.as_bytes().to_vec())
    }

    fn write_header(&mut self, samples: u64) -> Result<(), EncoderError> {
        self.stream.write_magic();
        self.stream.write_stream_info(&StreamInfo {
            samples,
            beg_silence: 0,
            sample_rate: self.config.sample_rate,
            max_band: self.max_band as u32,
            channels: self.config.channels,
            ms: self.params.ms_channelmode > 0,
            frames_per_block_pwr: self.config.frames_per_block_pwr,
        })?;
        self.stream.write_gain_info(&GainInfo::default())?;
        self.stream.write_encoder_info(&EncoderInfo {
            profile: self.params.full_qual,
            pns: self.params.pns > 0.0,
            major: 1,
            minor: 32,
            build: 0,
        })?;
        self.stream.write_seek_offset()?;
        Ok(())
    }

    fn flush_ap(&mut self, bytes: &[u8]) {
        if self.block_cnt & ((1 << self.config.seek_pwr) - 1) == 0 {
            self.seek_entries.push(self.stream.position());
            self.seek_pos += 1;
        }
        self.block_cnt += 1;
        self.stream.append(bytes);
    }

    /// One `if (!Silence || !OldSilence)` body. Returns `Res_L`/`Res_R` and,
    /// when `collect_trace` is set, the frame's diagnostic trace.
    ///
    /// The codec pipeline runs identically whether or not the trace is
    /// collected; tracing only adds observation copies of stage inputs and
    /// outputs.
    fn process_frame(
        &mut self,
        index: u64,
        collect_trace: bool,
    ) -> Result<FrameOutcome, EncoderError> {
        // Observation-only copy of the raw filterbank output. The subband
        // matrix is rewritten in place by the stages below, so this snapshot
        // must be taken here; skipping it changes nothing else.
        let x_raw = collect_trace.then(|| {
            (
                core::array::from_fn(|b| self.x[b].left),
                core::array::from_fn(|b| self.x[b].right),
            )
        });
        self.filterbank
            .process(&self.main_l, &self.main_r, &mut self.x, self.max_band)?;

        let pcm = PcmFrame {
            l: &self.main_l,
            r: &self.main_r,
            m: &self.main_m,
            s: &self.main_s,
        };
        let out = self.psy.analyse_frame(&pcm);
        let mut smr = out.smr;
        if self.params.min_smr > 0.0 {
            psy::raise_smr(self.max_band as i32, self.params.min_smr, &mut smr);
        }

        if self.params.ms_channelmode > 0 {
            let mut xl = [[0.0f32; SAMPLES]; SUBBANDS];
            let mut xr = [[0.0f32; SAMPLES]; SUBBANDS];
            for b in 0..SUBBANDS {
                xl[b] = self.x[b].left;
                xr[b] = self.x[b].right;
            }
            psy::ms_lr_entscheidung(
                self.max_band as i32,
                &mut self.ms_flag,
                &mut smr,
                &mut xl,
                &mut xr,
            );
            for b in 0..SUBBANDS {
                self.x[b].left = xl[b];
                self.x[b].right = xr[b];
            }
        }
        let x_ms = collect_trace.then(|| {
            (
                core::array::from_fn(|b| self.x[b].left),
                core::array::from_fn(|b| self.x[b].right),
            )
        });
        let ans_l = collect_trace.then(|| *self.psy.ans_spec_l());

        let coding_smr = CodingSmr {
            l: smr.l,
            r: smr.r,
            m: smr.m,
            s: smr.s,
        };

        let scf_out = scf_extraktion(
            &mut self.scf,
            self.max_band,
            self.params.comb_penalities,
            &mut self.x,
        )?;
        let transient = psy::transienten_calc(&out.transient_l, &out.transient_r);
        let ms_i32: [i32; SUBBANDS] = core::array::from_fn(|i| self.ms_flag[i] as i32);

        let (comp_l, comp_r, order_l, order_r, fir_l, fir_r) = if self.params.ns_order > 0 {
            let anspec = Anspec {
                l: *self.psy.ans_spec_l(),
                r: *self.psy.ans_spec_r(),
                m: *self.psy.ans_spec_m(),
                s: *self.psy.ans_spec_s(),
            };
            let ns = ns_analyse(
                self.max_band,
                &ms_i32,
                &coding_smr,
                &anspec,
                &self.scf.scf_l,
                &self.scf.scf_r,
                &transient,
                &scf_out.snr_comp_l,
                &scf_out.snr_comp_r,
            )?;
            (
                ns.snr_comp_l,
                ns.snr_comp_r,
                ns.order_l,
                ns.order_r,
                ns.fir_l,
                ns.fir_r,
            )
        } else {
            (
                scf_out.snr_comp_l,
                scf_out.snr_comp_r,
                [0u32; SUBBANDS],
                [0u32; SUBBANDS],
                [[0.0f32; 6]; SUBBANDS],
                [[0.0f32; 6]; SUBBANDS],
            )
        };

        let alloc = allocate(
            self.max_band,
            self.params.pns,
            &transient,
            &coding_smr,
            &scf_out.power_l,
            &scf_out.power_r,
            &comp_l,
            &comp_r,
            &mut self.scf.scf_l,
            &mut self.scf.scf_r,
            &mut self.x,
        )?;

        self.quant.quantize(
            self.max_band,
            &alloc.res_l,
            &alloc.res_r,
            &order_l,
            &order_r,
            &fir_l,
            &fir_r,
            &self.x,
        )?;

        let frame_trace = collect_trace.then(move || {
            let ((x_raw_l, x_raw_r), (x_ms_l, x_ms_r), ans_l) =
                (x_raw.unwrap(), x_ms.unwrap(), ans_l.unwrap());
            FrameTrace {
                index,
                current_read: self.current_read,
                silence: self.silence,
                processed: true,
                res_l: alloc.res_l,
                res_r: alloc.res_r,
                scf_l: self.scf.scf_l,
                scf_r: self.scf.scf_r,
                ms_flag: self.ms_flag,
                x_l: core::array::from_fn(|b| self.x[b].left),
                x_r: core::array::from_fn(|b| self.x[b].right),
                smr_l: smr.l,
                smr_r: smr.r,
                main_l: self.main_l.clone(),
                main_r: self.main_r.clone(),
                x_raw_l,
                x_raw_r,
                x_ms_l,
                x_ms_r,
                ans_l,
                loud: self.psy.loud(),
                xsave_l: self.psy.xsave_l8(),
                pre_thr_l: self.psy.pre_thr_l8(),
                tmp_mask_l: self.psy.tmp_mask_l8(),
                int_a: self.psy.int_a4(),
                int_b: self.psy.int_b4(),
                erg_l: *self.psy.last_erg_l(),
                ls_l: *self.psy.last_ls_l(),
                phs_l: *self.psy.last_phs_l(),
                vocal_l: self.psy.vocal_l16(),
                erg2_l: *self.psy.last_erg2_l(),
                cw_l: *self.psy.last_cw_l(),
                transient_l: out.transient_l,
                cls_l: *self.psy.last_cls_l(),
            }
        });

        Ok((alloc.res_l, alloc.res_r, frame_trace))
    }

    /// Reads one block of interleaved PCM into the analysis buffers at
    /// `CENTER`, reproducing `Read_WAV_Samples` for every supported integer
    /// width. Returns the number of frames actually read and the
    /// digital-silence flag.
    ///
    /// Per sample: the pre-scale `f32` value comes from
    /// [`PcmSource::sample`] (the reference `f16`/`f24`/`f32` conversion
    /// point), the denormal-fix constant is added in `f64` and rounded
    /// back to `f32` (the reference stores `float l[i] = f(c) * scalel +
    /// MPPENC_DENORMAL_FIX_*`), and `M`/`S` are computed from the stored
    /// `f32` pair exactly as the reference does. The default input scale
    /// is `1.0`, so the reference's `* scalel` multiply is an identity and
    /// is not written out.
    fn read_block(&mut self, pcm: PcmSource<'_>, all_read: u64) -> (usize, bool) {
        let channels = self.config.channels as usize;
        let remaining = self.samples_in_wave.saturating_sub(all_read);
        let requested = remaining.min(FRAME_LENGTH as u64) as usize;

        let mut silence = true;
        for i in 0..requested {
            let base = (all_read as usize + i) * channels;
            let (l, r) = if channels == 2 {
                let (fl, nonzero_l) = pcm.sample(base);
                let (fr, nonzero_r) = pcm.sample(base + 1);
                if nonzero_l || nonzero_r {
                    silence = false;
                }
                let l = f64::from(fl) + DENORMAL_FIX_LEFT;
                let r = f64::from(fr) + DENORMAL_FIX_RIGHT;
                (l as f32, r as f32)
            } else {
                let (temp, nonzero) = pcm.sample(base);
                if nonzero {
                    silence = false;
                }
                (
                    (f64::from(temp) + DENORMAL_FIX_LEFT) as f32,
                    (f64::from(temp) + DENORMAL_FIX_RIGHT) as f32,
                )
            };
            self.main_l[CENTER + i] = l;
            self.main_r[CENTER + i] = r;
            self.main_m[CENTER + i] = (l + r) * 0.5;
            self.main_s[CENTER + i] = (l - r) * 0.5;
        }
        // The reference zeroes the read buffer beyond the read samples and then
        // converts `RequestedSamples`; in this API the full block is always
        // available, so no padding conversion is needed.
        (requested, silence)
    }
}

fn fill(buf: &mut [f32], value: f32, count: usize) {
    for slot in buf.iter_mut().take(count) {
        *slot = value;
    }
}

fn fill_from(buf: &mut [f32], value: f32, start: usize, count: usize) {
    for slot in buf.iter_mut().skip(start).take(count) {
        *slot = value;
    }
}

fn memmove_left(buf: &mut [f32]) {
    buf.copy_within(FRAME_LENGTH..FRAME_LENGTH + CENTER, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_encodes_a_valid_stream() {
        let config = EncoderConfig::new(5.0, 44100, 2);
        let mut enc = MusepackEncoder::new(config).unwrap();
        let pcm = vec![0i16; 44100 * 2];
        let out = enc.encode(&pcm).unwrap();
        assert_eq!(&out[0..4], b"MPCK");
        assert_eq!(&out[4..6], b"SH");
        assert!(out.windows(2).any(|w| w == b"AP"));
        assert!(out.windows(2).any(|w| w == b"SE"));
    }

    /// The complete integer matrix (qualities `0..=10` × the four SV8 rates)
    /// must construct an encoder; this replaces the pre-J.1 test that pinned
    /// `q3 @ 44100` as unsupported.
    #[test]
    fn accepts_the_full_integer_quality_matrix() {
        for quality in 0..=10 {
            for &rate in &[44100u32, 48000, 37800, 32000] {
                MusepackEncoder::new(EncoderConfig::new(quality as f32, rate, 2))
                    .unwrap_or_else(|e| panic!("q{quality} @{rate} Hz must be accepted: {e}"));
            }
        }
    }

    /// Odd `frames_per_block_pwr` values are not representable in `SH`
    /// (`>> 1`), so they must fail closed instead of emitting a header that
    /// disagrees with the actual `AP` block size (audit §9).
    #[test]
    fn odd_frame_block_powers_are_rejected() {
        for pwr in [1u32, 5, 13] {
            let config = EncoderConfig {
                frames_per_block_pwr: pwr,
                ..EncoderConfig::new(5.0, 44100, 2)
            };
            assert!(
                matches!(
                    MusepackEncoder::new(config),
                    Err(EncoderError::InvalidBlockPower(p)) if p == pwr
                ),
                "odd frames_per_block_pwr {pwr} must be rejected"
            );
        }
        // Even powers stay accepted, including the extremes.
        for pwr in [0u32, 2, 14] {
            let config = EncoderConfig {
                frames_per_block_pwr: pwr,
                ..EncoderConfig::new(5.0, 44100, 2)
            };
            assert!(
                MusepackEncoder::new(config).is_ok(),
                "even frames_per_block_pwr {pwr} must be accepted"
            );
        }
    }

    /// `seek_pwr > 15` must clamp to `1` exactly like the reference
    /// `mpc_encoder_init`, instead of truncating the four-bit `ST` field or
    /// panicking on the seek-entry shift (audit §9).
    #[test]
    fn oversized_seek_pwr_clamps_to_the_reference_value() {
        let pcm = vec![1234i16; 9000 * 2];
        let encode = |seek_pwr: u32| -> Vec<u8> {
            let config = EncoderConfig {
                seek_pwr,
                ..EncoderConfig::new(5.0, 44100, 2)
            };
            MusepackEncoder::new(config)
                .expect("seek_pwr validation")
                .encode(&pcm)
                .expect("encode")
        };
        let reference = encode(1);
        assert_eq!(
            encode(16),
            reference,
            "seek_pwr 16 must behave like the clamped reference value 1"
        );
        assert_eq!(
            encode(63),
            reference,
            "a shift-overflowing seek_pwr must clamp instead of panicking"
        );
        assert!(!encode(0).is_empty(), "seek_pwr 0 stays valid");
    }

    /// Finite fractional quality and C-style clipping are now supported
    /// (J.2): `5.5` builds, and `11.0`/`-1.0` clip to q10/q0 through
    /// `PsyParams::from_quality`. What must keep failing closed: non-finite
    /// qualities (typed rejection; C's `NaN` path is undefined) and
    /// non-SV8 sample rates.
    #[test]
    fn rejects_unsupported_configuration() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(matches!(
                MusepackEncoder::new(EncoderConfig::new(bad, 44100, 2)),
                Err(EncoderError::NonFiniteQuality(q)) if q.is_nan() || q.is_infinite()
            ));
        }
        assert!(MusepackEncoder::new(EncoderConfig::new(5.0, 96000, 2)).is_err()); // non-SV8 rate
    }

    /// The C clip semantics: finite out-of-range qualities are accepted and
    /// resolve to the endpoint profile rows (byte identity against the q0/
    /// q10 streams is asserted in `tests/encoder_fractional.rs`).
    #[test]
    fn accepts_fractional_and_clipped_finite_qualities() {
        assert!(MusepackEncoder::new(EncoderConfig::new(5.5, 44100, 2)).is_ok());
        assert!(MusepackEncoder::new(EncoderConfig::new(4.25, 44100, 2)).is_ok());
        assert!(MusepackEncoder::new(EncoderConfig::new(11.0, 44100, 2)).is_ok());
        assert!(MusepackEncoder::new(EncoderConfig::new(-1.0, 44100, 2)).is_ok());
    }

    // -----------------------------------------------------------------
    // J.6 wide-PCM input conversion
    // -----------------------------------------------------------------

    /// Converts one left-aligned sample through the wide input path.
    fn wide(value: i32) -> (f32, bool) {
        let samples = [value];
        PcmSource::FullScale32(&samples).sample(0)
    }

    /// Converts one native 16-bit sample through the classic input path.
    fn narrow(value: i16) -> (f32, bool) {
        let samples = [value];
        PcmSource::Bits16(&samples).sample(0)
    }

    /// The wide-input conversion reproduces the reference `f24`/`f32`
    /// scaling at every boundary the J.6 slice must pin: minimum, maximum,
    /// zero, ±1, values around zero, representative full-scale values, and
    /// the low-order bits that conversion to `i16` would lose.
    ///
    /// All expected literals are exact binary fractions (or the documented
    /// single round-to-`f32` result), i.e. bit patterns, not approximations.
    #[allow(clippy::excessive_precision)] // exact f32 literals kept verbatim
    #[test]
    fn wide_conversion_matches_reference_boundary_values() {
        // --- 16-bit content left-aligned (`s16 << 16`): scale 1.0 ---
        assert_eq!(wide((-32768i16 as i32) << 16).0, -32768.0); // minimum
        assert_eq!(wide((32767i16 as i32) << 16).0, 32767.0); // maximum
        assert_eq!(wide(0).0, 0.0); // zero
        assert_eq!(wide((1i32) << 16).0, 1.0); // +1
        assert_eq!(wide((-1i32) << 16).0, -1.0); // -1
        // Around zero: the largest value whose `>> 16` truncation is 0.
        assert_eq!(wide(0x0000_FFFF).0, 0.9999847412109375);
        // The most negative value whose `>> 16` truncation is -1.
        assert_eq!(wide(-65535).0, -0.9999847412109375);

        // --- 24-bit content left-aligned (`s24 << 8`): scale 1/256 ---
        // The 24-bit minimum coincides with the full-scale minimum.
        assert_eq!(wide((-8_388_608i32) << 8).0, -32768.0);
        assert_eq!(wide((8_388_607i32) << 8).0, 32767.99609375); // maximum
        assert_eq!(wide(0).0, 0.0);
        assert_eq!(wide(1i32 << 8).0, 0.00390625); // +1 LSB of 24-bit
        assert_eq!(wide((-1i32) << 8).0, -0.00390625); // -1 LSB of 24-bit
        // Around zero: 0x00FF is exactly the amplitude class whose 16-bit
        // truncation (`s24 >> 8`) is zero — the precision J.6 must keep.
        assert_eq!(wide(0x0000_FF00).0, 0.99609375);
        assert_eq!(wide(-0x0000_FF00).0, -0.99609375);
        // Odd low-order content keeps every significant 24-bit bit: the
        // exact quotient `0x00FEFD / 256` is representable in `f32`.
        assert_eq!(wide(0x00FE_FD00).0, 0x00_FE_FDu32 as f32 / 256.0);
        assert_eq!(wide(0x00FE_FD00).0, 254.98828125);

        // --- 32-bit content (verbatim): scale 1/65536 ---
        // i32::MAX is a plain nearest-round up to full scale (the exact
        // quotient sits 2^-16 below 32768, far inside half an `f32` ULP).
        assert_eq!(wide(i32::MAX).0, 32768.0);
        assert_eq!(wide(i32::MIN).0, -32768.0); // minimum, exact
        assert_eq!(wide(0).0, 0.0);
        assert_eq!(wide(1).0, 0.0000152587890625); // +1 LSB of 32-bit
        assert_eq!(wide(-1).0, -0.0000152587890625); // -1 LSB of 32-bit
        assert_eq!(wide(65535).0, 0.9999847412109375); // around zero
        assert_eq!(wide(-65535).0, -0.9999847412109375);
        assert_eq!(wide(65536).0, 1.0); // +1 at the 16-bit scale
        assert_eq!(wide(-65536).0, -1.0);
        // Double-rounding probe: the exact value is 256 + 2^-16, exactly
        // halfway between two `f32` neighbours — round-to-nearest-even
        // keeps the even mantissa (256.0), matching the reference's single
        // round of the exact `f32()` double expression.
        assert_eq!(wide(0x0100_0001).0, 256.0);

        // --- the classic 16-bit path is unchanged at its boundaries ---
        assert_eq!(narrow(i16::MIN).0, -32768.0);
        assert_eq!(narrow(i16::MAX).0, 32767.0);
        assert_eq!(narrow(0).0, 0.0);
        assert_eq!(narrow(1).0, 1.0);
        assert_eq!(narrow(-1).0, -1.0);

        // The nonzero flag is the reference raw-byte silence test: a value
        // is silent iff it is exactly zero, at either representation.
        for v in [i32::MIN, i32::MAX, 0, 1, -1, 0x100, -0x100] {
            assert_eq!(wide(v).1, v != 0, "wide nonzero flag for {v}");
        }
        for v in [i16::MIN, i16::MAX, 0, 1, -1] {
            assert_eq!(narrow(v).1, v != 0, "narrow nonzero flag for {v}");
        }
    }

    /// `read_block` composes the analysis buffers exactly like the
    /// reference's stereo and mono branches: `value + fix` evaluated in
    /// `f64` and stored to `f32`, then `M = (L+R)*0.5` / `S = (L-R)*0.5`
    /// from the stored pair (so `L`/`R` here carry the denormal fix).
    #[allow(clippy::excessive_precision)] // exact f32 literals kept verbatim
    #[test]
    fn read_block_composes_l_r_m_s_like_the_reference() {
        // Stereo: one zero frame and one near-full-scale 24-bit frame.
        let mut enc = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2)).unwrap();
        enc.samples_in_wave = 2;
        let pcm = [0i32, 0, 0x00FF_FF00, -0x00FF_FF00];
        let (read, silence) = enc.read_block(PcmSource::FullScale32(&pcm), 0);
        assert_eq!(read, 2);
        assert!(!silence, "nonzero samples must clear the silence flag");

        // Frame 0: only the denormal-fix constants are added.
        let fix_l = 0.001953125f32; // 2^-9, exact
        let fix_r = 0.0009765625f32; // 2^-10, exact
        assert_eq!(enc.main_l[CENTER], fix_l);
        assert_eq!(enc.main_r[CENTER], fix_r);
        assert_eq!(enc.main_m[CENTER], (fix_l + fix_r) * 0.5);
        assert_eq!(enc.main_s[CENTER], (fix_l - fix_r) * 0.5);

        // Frame 1: L = +0x00FFFF00, R = -0x00FFFF00 (mirrored pair, s24 =
        // ±65535, value ±255.99609375), each channel with its own fix,
        // composed in `f64` and stored to `f32`.
        let v = 255.99609375f32; // 65535/256, exact in f32
        let l = (f64::from(v) + DENORMAL_FIX_LEFT) as f32;
        let r = (f64::from(-v) + DENORMAL_FIX_RIGHT) as f32;
        assert_eq!(l, 255.998046875); // exact sum, no rounding
        assert_eq!(r, -255.9951171875);
        assert_eq!(enc.main_l[CENTER + 1], l);
        assert_eq!(enc.main_r[CENTER + 1], r);
        // M reduces to (fixL + fixR)/2 for the mirrored pair; S carries
        // the signal.
        assert_eq!(enc.main_m[CENTER + 1], 0.00146484375); // literal
        assert_eq!(enc.main_s[CENTER + 1], 255.99658203125); // literal
        assert_eq!(enc.main_m[CENTER + 1], (l + r) * 0.5);
        assert_eq!(enc.main_s[CENTER + 1], (l - r) * 0.5);

        // Mono: one shared value feeds both channels through their own fix.
        let mut mono = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 1)).unwrap();
        mono.samples_in_wave = 2;
        let (read, silence) = mono.read_block(PcmSource::FullScale32(&[-0x0000_0100, 0]), 0);
        assert_eq!(read, 2);
        assert!(!silence);
        let v = -0.00390625f32; // -1 LSB of 24-bit, exact
        let l = (f64::from(v) + DENORMAL_FIX_LEFT) as f32;
        let r = (f64::from(v) + DENORMAL_FIX_RIGHT) as f32;
        assert_eq!(mono.main_l[CENTER], l);
        assert_eq!(mono.main_r[CENTER], r);
        assert_eq!(mono.main_m[CENTER], (l + r) * 0.5);
        assert_eq!(mono.main_s[CENTER], (l - r) * 0.5);

        // An all-zero block is digital silence; a block containing any
        // nonzero sample (however small) is not.
        let mut quiet = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 1)).unwrap();
        quiet.samples_in_wave = 2;
        let (_, silence) = quiet.read_block(PcmSource::FullScale32(&[0, 0]), 0);
        assert!(silence, "an all-zero block is digital silence");
        let mut loud = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 1)).unwrap();
        loud.samples_in_wave = 2;
        let (_, silence) = loud.read_block(PcmSource::FullScale32(&[0, 1]), 0);
        assert!(!silence, "a 1-LSB 32-bit sample is not silence");
    }

    /// LCG noise matching `tools/gen_encoder_fixtures.py`.
    fn lcg_pcm(kind: &str, frames: usize, channels: u32) -> Vec<i16> {
        fn lcg(state: &mut u32) -> u32 {
            *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *state
        }
        let mut sl = 0x1234_5678u32;
        let mut sr = 0x9ABC_DEF0u32;
        let mut out = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let (l, r): (i16, i16) = match kind {
                "noise" => (
                    ((lcg(&mut sl) >> 16) as i32 - 32768) as i16,
                    ((lcg(&mut sr) >> 16) as i32 - 32768) as i16,
                ),
                "silence" => (0, 0),
                "constant" => (12000, 12000),
                "ramp" => {
                    let v = (((i % 512) as i32 - 256) * 60) as i16;
                    (v, v)
                }
                other => panic!("unknown kind {other}"),
            };
            out.push(l);
            if channels == 2 {
                out.push(r);
            }
        }
        out
    }

    /// The wide input path is a pure representation change: 16-bit content
    /// left-aligned to `i32` must encode **byte-identically** to the
    /// classic `i16` path (this is the J.6 proof that existing 16-bit
    /// behaviour survives the refactor unchanged).
    #[test]
    fn left_aligned_s32_reproduces_the_i16_path_byte_for_byte() {
        for (kind, frames, channels) in [
            ("noise", 5000, 2),
            ("silence", 5000, 2),
            ("constant", 5000, 2),
            ("ramp", 5000, 2),
            ("noise", 5000, 1),
            ("noise", 1152, 2),
            ("noise", 93728, 2),
        ] {
            let pcm = lcg_pcm(kind, frames, channels);
            let aligned: Vec<i32> = pcm.iter().map(|&s| i32::from(s) << 16).collect();
            let config = EncoderConfig::new(5.0, 44100, channels);
            let narrow_bytes = MusepackEncoder::new(config).unwrap().encode(&pcm).unwrap();
            let wide_bytes = MusepackEncoder::new(config)
                .unwrap()
                .encode_s32(&aligned)
                .unwrap();
            assert_eq!(
                narrow_bytes, wide_bytes,
                "{kind}/{frames}/{channels}ch: wide path must reproduce the i16 stream"
            );
        }
    }

    /// `encode_s32` and `encode_s32_traced` must share one implementation,
    /// exactly like `encode`/`encode_traced`.
    #[test]
    fn encode_s32_matches_encode_s32_traced() {
        let mut state = 0x1234_5678u32;
        let pcm: Vec<i32> = (0..2000)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                state as i32
            })
            .collect();
        let config = EncoderConfig::new(5.0, 44100, 2);
        let plain = MusepackEncoder::new(config)
            .unwrap()
            .encode_s32(&pcm)
            .unwrap();
        let (traced_bytes, trace) = MusepackEncoder::new(config)
            .unwrap()
            .encode_s32_traced(&pcm)
            .unwrap();
        assert_eq!(plain, traced_bytes, "traced bytes must match plain encode");
        assert!(!trace.is_empty());
    }

    /// Phase 5 discrimination: where the source carries low-order
    /// information, the wide stream must **differ** from what 16-bit
    /// truncation would have encoded — proving wide input is not
    /// accidentally reduced to `i16` anywhere in the new path.
    #[test]
    fn wide_pcm_differs_from_i16_truncated_encoding() {
        // A 24-bit signal living entirely below the 16-bit truncation
        // threshold: every sample's `s24 >> 8` is zero, so the truncated
        // encoding is the digital-silence stream while the wide encoding
        // carries real (albeit quiet) content.
        let low_order: Vec<i32> = (0..4096u32)
            .map(|i| {
                let state = i.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state & 0xFF) << 8) as i32 // s24 in [0, 255], left-aligned
            })
            .collect();
        assert!(
            low_order.iter().all(|&v| (v >> 16) == 0),
            "the fixture must be invisible to 16-bit truncation"
        );
        assert!(low_order.iter().any(|&v| v != 0), "…but not to us");

        let wide_stream = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2))
            .unwrap()
            .encode_s32(&low_order)
            .unwrap();
        let truncated: Vec<i16> = low_order.iter().map(|&v| (v >> 16) as i16).collect();
        assert!(truncated.iter().all(|&s| s == 0));
        let zeros = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2))
            .unwrap()
            .encode(&vec![0i16; low_order.len()])
            .unwrap();
        assert_ne!(
            wide_stream, zeros,
            "24-bit low-order content must not encode as its i16 truncation"
        );
        let truncated_stream = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2))
            .unwrap()
            .encode(&truncated)
            .unwrap();
        assert_eq!(
            truncated_stream, zeros,
            "an all-zero truncation must be the digital-silence stream"
        );

        // Loud 32-bit content whose difference from its truncation lives
        // only in the low 16 bits: the streams must still diverge.
        let mut state = 0x1234_5678u32;
        let fullscale: Vec<i32> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                state as i32
            })
            .collect();
        let wide32 = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2))
            .unwrap()
            .encode_s32(&fullscale)
            .unwrap();
        let truncated32: Vec<i16> = fullscale.iter().map(|&v| (v >> 16) as i16).collect();
        let truncated32_stream = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2))
            .unwrap()
            .encode(&truncated32)
            .unwrap();
        assert_ne!(
            wide32, truncated32_stream,
            "32-bit low-order information must reach the stream"
        );
    }

    /// Replays the committed C-generated conversion oracle
    /// (`tests/data/encoder/pcm_conversion_oracle.txt`, produced by
    /// `tools/extract_pcm_oracle.c` from the reference
    /// `Read_WAV_Samples`) so a passing whole-stream differential cannot
    /// conceal a conversion coincidence: for the recorded left-aligned
    /// inputs the Rust input path must reproduce the reference **pre-fix
    /// value** and the stored `L`/`R`/`M`/`S` analysis buffers
    /// **bit-for-bit** — 24-bit stereo, 32-bit stereo and 24-bit mono.
    #[test]
    fn read_block_matches_the_c_pcm_conversion_oracle() {
        fn replay(channels: usize, pcm: &[i32], expected: &[[u32; 6]], section: &str) {
            let mut enc =
                MusepackEncoder::new(EncoderConfig::new(5.0, 44100, channels as u32)).unwrap();
            enc.samples_in_wave = expected.len() as u64;
            let src = PcmSource::FullScale32(pcm);
            let (read, silence) = enc.read_block(src, 0);
            assert_eq!(read, expected.len(), "{section}: read all oracle rows");
            assert!(!silence, "{section}: oracle input is not silence");
            for (i, want) in expected.iter().enumerate() {
                let (fl, fr) = if channels == 2 {
                    (src.sample(2 * i).0, src.sample(2 * i + 1).0)
                } else {
                    let v = src.sample(i).0;
                    (v, v)
                };
                assert_eq!(
                    fl.to_bits(),
                    want[0],
                    "{section} row {i}: pre-fix L value diverges from the C oracle"
                );
                assert_eq!(
                    fr.to_bits(),
                    want[1],
                    "{section} row {i}: pre-fix R value diverges from the C oracle"
                );
                for (slot, offset, label) in [
                    (&enc.main_l, 2usize, "L"),
                    (&enc.main_r, 3, "R"),
                    (&enc.main_m, 4, "M"),
                    (&enc.main_s, 5, "S"),
                ] {
                    assert_eq!(
                        slot[CENTER + i].to_bits(),
                        want[offset],
                        "{section} row {i}: analysis buffer {label} diverges from the C oracle"
                    );
                }
            }
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/encoder/pcm_conversion_oracle.txt");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

        let mut channels = 0usize;
        let mut section = String::new();
        let mut inputs: Vec<i32> = Vec::new();
        let mut expected: Vec<[u32; 6]> = Vec::new();
        let mut sections = 0usize;
        let mut rows = 0usize;

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("# file ") {
                if !expected.is_empty() {
                    replay(channels, &inputs, &expected, &section);
                    sections += 1;
                    rows += expected.len();
                }
                inputs.clear();
                expected.clear();
                let name = line
                    .split_whitespace()
                    .nth(2)
                    .expect("file name")
                    .to_string();
                channels = line
                    .split("channels=")
                    .nth(1)
                    .expect("channels=")
                    .split_whitespace()
                    .next()
                    .expect("channels value")
                    .parse()
                    .expect("channels integer");
                section = name;
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split_whitespace().collect();
            let hex: Vec<u32> = f[1..]
                .iter()
                .map(|v| u32::from_str_radix(v, 16).expect("hex32"))
                .collect();
            let want = if channels == 2 {
                assert_eq!(hex.len(), 8, "{section}: stereo row width");
                inputs.push(hex[0] as i32);
                inputs.push(hex[1] as i32);
                [hex[2], hex[3], hex[4], hex[5], hex[6], hex[7]]
            } else {
                assert_eq!(hex.len(), 6, "{section}: mono row width");
                inputs.push(hex[0] as i32);
                // Mono: one pre-fix value feeds both channels.
                [hex[1], hex[1], hex[2], hex[3], hex[4], hex[5]]
            };
            expected.push(want);
        }
        if !expected.is_empty() {
            replay(channels, &inputs, &expected, &section);
            sections += 1;
            rows += expected.len();
        }

        assert_eq!(sections, 3, "the oracle has three sections (24/32/mono)");
        assert_eq!(rows, 1800, "the oracle has 1800 recorded rows");
    }
}
