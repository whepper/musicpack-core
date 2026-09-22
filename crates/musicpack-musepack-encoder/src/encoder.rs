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
    /// This path never constructs diagnostic trace state; see
    /// [`encode_traced`](Self::encode_traced) for the debugging variant.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, EncoderError> {
        self.encode_inner(pcm, None)
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
        let bytes = self.encode_inner(pcm, Some(&mut trace))?;
        Ok((bytes, trace))
    }

    fn encode_inner(
        &mut self,
        pcm: &[i16],
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
    /// `CENTER`, reproducing `Read_WAV_Samples` for 16-bit input. Returns the
    /// number of frames actually read and the digital-silence flag.
    fn read_block(&mut self, pcm: &[i16], all_read: u64) -> (usize, bool) {
        let channels = self.config.channels as usize;
        let remaining = self.samples_in_wave.saturating_sub(all_read);
        let requested = remaining.min(FRAME_LENGTH as u64) as usize;

        let mut silence = true;
        for i in 0..requested {
            let base = (all_read as usize + i) * channels;
            let (l, r) = if channels == 2 {
                let sl = pcm[base];
                let sr = pcm[base + 1];
                if sl != 0 || sr != 0 {
                    silence = false;
                }
                let l = (f32::from(sl) * 1.0) as f64 + DENORMAL_FIX_LEFT;
                let r = (f32::from(sr) * 1.0) as f64 + DENORMAL_FIX_RIGHT;
                (l as f32, r as f32)
            } else {
                let s = pcm[base];
                if s != 0 {
                    silence = false;
                }
                let temp = f32::from(s) * 1.0;
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

    /// Surfaces deliberately deferred to the fractional/clip parity slice
    /// (J.2) and non-SV8 rates must keep failing closed.
    #[test]
    fn rejects_unsupported_configuration() {
        assert!(MusepackEncoder::new(EncoderConfig::new(5.5, 44100, 2)).is_err()); // fractional (J.2)
        assert!(MusepackEncoder::new(EncoderConfig::new(11.0, 44100, 2)).is_err()); // clip parity (J.2)
        assert!(MusepackEncoder::new(EncoderConfig::new(-1.0, 44100, 2)).is_err()); // clip parity (J.2)
        assert!(MusepackEncoder::new(EncoderConfig::new(5.0, 96000, 2)).is_err()); // non-SV8 rate
    }
}
