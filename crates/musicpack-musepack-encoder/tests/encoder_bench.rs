//! Release-mode throughput and stage benchmarks (Phase 15H.0-15H.1).
//!
//! Measurement only: no codec behaviour is changed here. Uses only public
//! APIs, never diagnostic tracing for the production baseline, and sinks all
//! output so the compiler cannot eliminate work.
//!
//! Run with:
//! ```sh
//! cargo test --release -p musicpack-musepack-encoder --test encoder_bench \
//!   -- --ignored --nocapture
//! ```
//!
//! The stage benchmark replicates the integrated frame loop stage by stage
//! and asserts byte-identity with `MusepackEncoder::encode`, so the reported
//! per-stage timings genuinely describe the integrated encoder.

use std::time::{Duration, Instant};

use musicpack_musepack_encoder::blocks::{EncoderInfo, GainInfo, StreamInfo, Sv8StreamWriter};
use musicpack_musepack_encoder::coding::ans::{Anspec, Smr as CodingSmr};
use musicpack_musepack_encoder::coding::frame::FrameEncoder;
use musicpack_musepack_encoder::coding::quant::Quantizer;
use musicpack_musepack_encoder::coding::{ScfState, allocate, ns_analyse, scf_extraktion};
use musicpack_musepack_encoder::encoder::{
    ANABUFFER, CENTER, DECODER_DELAY, EncoderConfig, FRAME_LENGTH, MusepackEncoder,
};
use musicpack_musepack_encoder::filterbank::{AnalysisFilterbank, Subband};
use musicpack_musepack_encoder::psy::{self, PcmFrame, PsychoacousticModel};

const SUBBANDS: usize = 32;
const SAMPLES: usize = 36;
/// Exact `MPPENC_DENORMAL_FIX` values (32·1024/2²⁴ and half that).
const DENORMAL_FIX_LEFT: f64 = 0.001953125;
const DENORMAL_FIX_RIGHT: f64 = 0.0009765625;

// ---------------------------------------------------------------------------
// Deterministic PCM generators (stereo i16, interleaved).
// ---------------------------------------------------------------------------

fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

fn signed16(state: u32) -> i16 {
    (((state >> 16) & 0xFFFF) as i32 - 32768) as i16
}

fn gen_silence(frames: usize) -> Vec<i16> {
    vec![0i16; frames * 2]
}

fn gen_noise(frames: usize) -> Vec<i16> {
    let mut sl = 0x1234_5678u32;
    let mut sr = 0x9ABC_DEF0u32;
    let mut out = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        out.push(signed16(lcg(&mut sl)));
        out.push(signed16(lcg(&mut sr)));
    }
    out
}

fn gen_sine(frames: usize, rate: u32, freq_l: f64, freq_r: f64, amp: f64) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        let l = (amp * (2.0 * std::f64::consts::PI * freq_l * t).sin()) as i16;
        let r = (amp * (2.0 * std::f64::consts::PI * freq_r * t).sin()) as i16;
        out.push(l);
        out.push(r);
    }
    out
}

fn gen_multitone(frames: usize, rate: u32) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        let v = 6000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
            + 4000.0 * (2.0 * std::f64::consts::PI * 3000.0 * t).sin()
            + 2000.0 * (2.0 * std::f64::consts::PI * 7000.0 * t).sin();
        let s = v.clamp(-32768.0, 32767.0) as i16;
        out.push(s);
        out.push(s);
    }
    out
}

/// Low noise floor with periodic stereo-differing impulses.
fn gen_transient(frames: usize) -> Vec<i16> {
    let mut sl = 0x1357_9BDFu32;
    let mut sr = 0x2468_ACE0u32;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let mut l = signed16(lcg(&mut sl)) / 32;
        let mut r = signed16(lcg(&mut sr)) / 32;
        if i % 4096 == 0 {
            l = 30000;
        }
        if i % 4096 == 2048 {
            r = -30000;
        }
        out.push(l);
        out.push(r);
    }
    out
}

// ---------------------------------------------------------------------------
// Stats + harness.
// ---------------------------------------------------------------------------

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort_unstable();
    v[v.len() / 2]
}

fn minimum(v: &[Duration]) -> Duration {
    *v.iter().min().unwrap()
}

/// Runs `f` after warm-up; returns per-iteration times. Setup must happen
/// inside `f`'s caller (fresh encoder per iteration), only the measured
/// closure is timed.
fn measure(warmup: usize, iters: usize, mut f: impl FnMut()) -> Vec<Duration> {
    for _ in 0..warmup {
        f();
    }
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        f();
        times.push(t0.elapsed());
    }
    times
}

fn sink_bytes(bytes: &[u8]) -> u64 {
    // Cheap fold so the output is consumed; the length assert below also pins
    // the stream against accidental empty-output regressions.
    let mut acc: u64 = bytes.len() as u64;
    for (i, b) in bytes.iter().enumerate() {
        acc = acc.wrapping_add(
            (*b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15u64.wrapping_add(i as u64)),
        );
    }
    acc
}

struct ThroughputRow {
    case: &'static str,
    quality: f32,
    secs_audio: f64,
    bytes: usize,
    min: Duration,
    median: Duration,
}

fn bench_encode(case: &'static str, quality: f32, rate: u32, pcm: &[i16]) -> ThroughputRow {
    let secs_audio = pcm.len() as f64 / 2.0 / rate as f64;
    let mut bytes = 0usize;
    let mut acc = 0u64;
    let times = measure(2, 5, || {
        let mut enc = MusepackEncoder::new(EncoderConfig::new(quality, rate, 2)).unwrap();
        let out = enc.encode(pcm).unwrap();
        bytes = out.len();
        acc ^= sink_bytes(&out);
    });
    std::hint::black_box(acc);
    assert!(bytes > 0, "{case}: empty output");
    ThroughputRow {
        case,
        quality,
        secs_audio,
        bytes,
        min: minimum(&times),
        median: median(times),
    }
}

fn print_throughput(rows: &[ThroughputRow]) {
    println!(
        "{:<14} {:>7} {:>9} {:>10} {:>12} {:>12} {:>9}",
        "case", "qual", "audio", "bytes", "min", "median", "realtime"
    );
    for r in rows {
        println!(
            "{:<14} {:>7.1} {:>8.1}s {:>10} {:>11.1?} {:>11.1?} {:>8.1}x",
            r.case,
            r.quality,
            r.secs_audio,
            r.bytes,
            r.min,
            r.median,
            r.secs_audio / r.median.as_secs_f64(),
        );
    }
}

// ---------------------------------------------------------------------------
// Throughput matrix (§3–§4).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn bench_throughput_matrix() {
    let mut rows = Vec::new();
    // ~1 s cases across signal characteristics (q5/44.1k primary).
    let s1 = 44100usize;
    rows.push(bench_encode("silence-1s", 5.0, 44100, &gen_silence(s1)));
    rows.push(bench_encode("noise-1s", 5.0, 44100, &gen_noise(s1)));
    rows.push(bench_encode(
        "sine-1s",
        5.0,
        44100,
        &gen_sine(s1, 44100, 1000.0, 1000.0, 20000.0),
    ));
    rows.push(bench_encode(
        "multitone-1s",
        5.0,
        44100,
        &gen_multitone(s1, 44100),
    ));
    rows.push(bench_encode("transient-1s", 5.0, 44100, &gen_transient(s1)));
    rows.push(bench_encode(
        "stereo-1s",
        5.0,
        44100,
        &gen_sine(s1, 44100, 700.0, 1100.0, 20000.0),
    ));
    // Medium ~10 s noise.
    rows.push(bench_encode("noise-10s", 5.0, 44100, &gen_noise(441000)));
    // Qualities on ~10 s noise.
    rows.push(bench_encode("noise-10s-q4", 4.0, 44100, &gen_noise(441000)));
    rows.push(bench_encode("noise-10s-q6", 6.0, 44100, &gen_noise(441000)));
    rows.push(bench_encode("noise-10s-q7", 7.0, 44100, &gen_noise(441000)));
    // Secondary sample rate.
    rows.push(bench_encode(
        "noise-10s-48k",
        5.0,
        48000,
        &gen_noise(480000),
    ));
    print_throughput(&rows);
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn bench_long_60s() {
    let rows = [bench_encode(
        "noise-60s",
        5.0,
        44100,
        &gen_noise(44100 * 60),
    )];
    print_throughput(&rows);
}

// ---------------------------------------------------------------------------
// Stage-level timing (§5–§6): faithful manual loop + byte-identity check.
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct StageTotals {
    pcm_prep: Duration,
    filterbank: Duration,
    psy: Duration,
    raise_smr: Duration,
    ms_decision: Duration,
    scf: Duration,
    transient_calc: Duration,
    ns_analyse: Duration,
    allocate: Duration,
    quantize: Duration,
    huffman_ap: Duration,
    container: Duration,
}

impl StageTotals {
    fn add(&mut self, other: &StageTotals) {
        macro_rules! acc {
            ($($f:ident),*) => { $(self.$f += other.$f;)* };
        }
        acc!(
            pcm_prep,
            filterbank,
            psy,
            raise_smr,
            ms_decision,
            scf,
            transient_calc,
            ns_analyse,
            allocate,
            quantize,
            huffman_ap,
            container
        );
    }

    fn total(&self) -> Duration {
        self.pcm_prep
            + self.filterbank
            + self.psy
            + self.raise_smr
            + self.ms_decision
            + self.scf
            + self.transient_calc
            + self.ns_analyse
            + self.allocate
            + self.quantize
            + self.huffman_ap
            + self.container
    }
}

/// Manual integrated loop mirroring `MusepackEncoder::encode_inner`, timing
/// each stage. Returns the assembled stream and accumulated stage times.
fn encode_staged(quality: f32, rate: u32, pcm: &[i16], totals: &mut StageTotals) -> Vec<u8> {
    let config = EncoderConfig::new(quality, rate, 2);
    let probe = MusepackEncoder::new(config).unwrap();
    let params = *probe.params();
    let max_band = probe.max_band();
    drop(probe);

    let mut filterbank = AnalysisFilterbank::new();
    let mut psy = PsychoacousticModel::new(quality, rate as f32).unwrap();
    let mut scf = ScfState::default();
    let mut quant = Quantizer::new();
    let mut frame = FrameEncoder::new(config.frames_per_block_pwr);
    let mut stream = Sv8StreamWriter::with_seek_ref(0);

    let mut main_l = vec![0.0f32; ANABUFFER];
    let mut main_r = vec![0.0f32; ANABUFFER];
    let mut main_m = vec![0.0f32; ANABUFFER];
    let mut main_s = vec![0.0f32; ANABUFFER];
    let samples_in_wave = (pcm.len() / 2) as u64;
    let mut all_samples_read = 0u64;
    let mut current_read = 0usize;
    let mut silence = false;
    let mut old_silence = false;
    let mut block_cnt = 0u32;
    let mut seek_pos = 0u32;
    let mut seek_entries = Vec::new();
    let mut x = [Subband::ZERO; SUBBANDS];
    let mut ms_flag = [0u8; SUBBANDS];

    let mut stage = StageTotals::default();

    // Header (container).
    timed_result(&mut stage.container, || {
        stream.write_magic();
        stream
            .write_stream_info(&StreamInfo {
                samples: samples_in_wave,
                beg_silence: 0,
                sample_rate: rate,
                max_band: max_band as u32,
                channels: 2,
                ms: params.ms_channelmode > 0,
                frames_per_block_pwr: config.frames_per_block_pwr,
            })
            .unwrap();
        stream.write_gain_info(&GainInfo::default()).unwrap();
        stream
            .write_encoder_info(&EncoderInfo {
                profile: params.full_qual,
                pns: params.pns > 0.0,
                major: 1,
                minor: 32,
                build: 0,
            })
            .unwrap();
        stream.write_seek_offset().unwrap();
    });

    // Pre-loop read + priming (PCM preparation).
    timed_result(&mut stage.pcm_prep, || {
        let (read, sil) = read_block(
            pcm,
            0,
            samples_in_wave,
            &mut main_l,
            &mut main_r,
            &mut main_m,
            &mut main_s,
        );
        current_read = read;
        silence = sil;
        all_samples_read = read as u64;
        if read > 0 {
            let (fl, fr, fm, fs) = (
                main_l[CENTER],
                main_r[CENTER],
                main_m[CENTER],
                main_s[CENTER],
            );
            fill(&mut main_l, fl, CENTER);
            fill(&mut main_r, fr, CENTER);
            fill(&mut main_m, fm, CENTER);
            fill(&mut main_s, fs, CENTER);
        }
    });
    timed_result(&mut stage.filterbank, || {
        filterbank
            .init(main_l[CENTER], main_r[CENTER], &mut x, max_band)
            .unwrap();
    });

    let mut n: u64 = 0;
    while (n * FRAME_LENGTH as u64) < (samples_in_wave + DECODER_DELAY) {
        timed_result(&mut stage.pcm_prep, || {
            if current_read < FRAME_LENGTH && n > 0 {
                let last = CENTER + current_read - 1;
                let (l, r, m, s) = (main_l[last], main_r[last], main_m[last], main_s[last]);
                fill_from(
                    &mut main_l,
                    l,
                    CENTER + current_read,
                    FRAME_LENGTH - current_read,
                );
                fill_from(
                    &mut main_r,
                    r,
                    CENTER + current_read,
                    FRAME_LENGTH - current_read,
                );
                fill_from(
                    &mut main_m,
                    m,
                    CENTER + current_read,
                    FRAME_LENGTH - current_read,
                );
                fill_from(
                    &mut main_s,
                    s,
                    CENTER + current_read,
                    FRAME_LENGTH - current_read,
                );
            }
        });

        let mut res_l = [0i32; SUBBANDS];
        let mut res_r = [0i32; SUBBANDS];

        if !silence || !old_silence {
            timed_result(&mut stage.filterbank, || {
                filterbank
                    .process(&main_l, &main_r, &mut x, max_band)
                    .unwrap();
            });

            let pcm_frame = PcmFrame {
                l: &main_l,
                r: &main_r,
                m: &main_m,
                s: &main_s,
            };
            let out = timed_result(&mut stage.psy, || psy.analyse_frame(&pcm_frame));
            let mut smr = out.smr;
            timed_result(&mut stage.raise_smr, || {
                if params.min_smr > 0.0 {
                    psy::raise_smr(max_band as i32, params.min_smr, &mut smr);
                }
            });

            timed_result(&mut stage.ms_decision, || {
                if params.ms_channelmode > 0 {
                    let mut xl = [[0.0f32; SAMPLES]; SUBBANDS];
                    let mut xr = [[0.0f32; SAMPLES]; SUBBANDS];
                    for b in 0..SUBBANDS {
                        xl[b] = x[b].left;
                        xr[b] = x[b].right;
                    }
                    psy::ms_lr_entscheidung(
                        max_band as i32,
                        &mut ms_flag,
                        &mut smr,
                        &mut xl,
                        &mut xr,
                    );
                    for b in 0..SUBBANDS {
                        x[b].left = xl[b];
                        x[b].right = xr[b];
                    }
                }
            });

            let coding_smr = CodingSmr {
                l: smr.l,
                r: smr.r,
                m: smr.m,
                s: smr.s,
            };

            let scf_out = timed_result(&mut stage.scf, || {
                scf_extraktion(&mut scf, max_band, params.comb_penalities, &mut x).unwrap()
            });
            let transient = timed_result(&mut stage.transient_calc, || {
                psy::transienten_calc(&out.transient_l, &out.transient_r)
            });
            let ms_i32: [i32; SUBBANDS] = core::array::from_fn(|i| ms_flag[i] as i32);

            let (comp_l, comp_r, order_l, order_r, fir_l, fir_r) = if params.ns_order > 0 {
                let anspec = Anspec {
                    l: *psy.ans_spec_l(),
                    r: *psy.ans_spec_r(),
                    m: *psy.ans_spec_m(),
                    s: *psy.ans_spec_s(),
                };
                let ns = timed_result(&mut stage.ns_analyse, || {
                    ns_analyse(
                        max_band,
                        &ms_i32,
                        &coding_smr,
                        &anspec,
                        &scf.scf_l,
                        &scf.scf_r,
                        &transient,
                        &scf_out.snr_comp_l,
                        &scf_out.snr_comp_r,
                    )
                    .unwrap()
                });
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

            let alloc = timed_result(&mut stage.allocate, || {
                allocate(
                    max_band,
                    params.pns,
                    &transient,
                    &coding_smr,
                    &scf_out.power_l,
                    &scf_out.power_r,
                    &comp_l,
                    &comp_r,
                    &mut scf.scf_l,
                    &mut scf.scf_r,
                    &mut x,
                )
                .unwrap()
            });
            res_l = alloc.res_l;
            res_r = alloc.res_r;

            timed_result(&mut stage.quantize, || {
                quant
                    .quantize(
                        max_band,
                        &alloc.res_l,
                        &alloc.res_r,
                        &order_l,
                        &order_r,
                        &fir_l,
                        &fir_r,
                        &x,
                    )
                    .unwrap();
            });
        }
        old_silence = silence;

        let ms_i32: [i32; SUBBANDS] = core::array::from_fn(|i| ms_flag[i] as i32);
        timed_result(&mut stage.huffman_ap, || {
            frame
                .encode_frame(
                    max_band as i32,
                    u32::from(params.ms_channelmode),
                    &res_l,
                    &res_r,
                    &scf.scf_l,
                    &scf.scf_r,
                    &ms_i32,
                    quant.q_l(),
                    quant.q_r(),
                )
                .unwrap();
            let blocks = frame.take_blocks();
            if !blocks.is_empty() {
                if block_cnt & ((1 << config.seek_pwr) - 1) == 0 {
                    seek_entries.push(stream.position());
                    seek_pos += 1;
                }
                block_cnt += 1;
                stream.append(&blocks);
            }
        });

        timed_result(&mut stage.pcm_prep, || {
            memmove_left(&mut main_l);
            memmove_left(&mut main_r);
            memmove_left(&mut main_m);
            memmove_left(&mut main_s);
            let (read, sil) = read_block(
                pcm,
                all_samples_read,
                samples_in_wave,
                &mut main_l,
                &mut main_r,
                &mut main_m,
                &mut main_s,
            );
            current_read = read;
            silence = sil;
            all_samples_read += read as u64;
        });

        n += 1;
    }

    timed_result(&mut stage.huffman_ap, || {
        let tail = frame.flush_partial();
        if !tail.is_empty() {
            if block_cnt & ((1 << config.seek_pwr) - 1) == 0 {
                seek_entries.push(stream.position());
                seek_pos += 1;
            }
            block_cnt += 1;
            stream.append(&tail);
        }
    });
    timed_result(&mut stage.container, || {
        stream
            .write_seek_table(seek_pos, config.seek_pwr, &seek_entries)
            .unwrap();
        stream.write_end().unwrap();
    });

    totals.add(&stage);
    stream.as_bytes().to_vec()
}

fn timed_result<T>(slot: &mut Duration, f: impl FnOnce() -> T) -> T {
    let t0 = Instant::now();
    let v = f();
    *slot += t0.elapsed();
    v
}

// -- Local copies of the encoder's private buffer helpers. Kept identical to
// -- `src/encoder.rs`; the byte-identity self-check below fails loudly on any
// -- drift.
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

fn read_block(
    pcm: &[i16],
    all_read: u64,
    samples_in_wave: u64,
    main_l: &mut [f32],
    main_r: &mut [f32],
    main_m: &mut [f32],
    main_s: &mut [f32],
) -> (usize, bool) {
    let remaining = samples_in_wave.saturating_sub(all_read);
    let requested = remaining.min(FRAME_LENGTH as u64) as usize;
    let mut silence = true;
    for i in 0..requested {
        let base = (all_read as usize + i) * 2;
        let sl = pcm[base];
        let sr = pcm[base + 1];
        if sl != 0 || sr != 0 {
            silence = false;
        }
        let l = (f32::from(sl) * 1.0) as f64 + DENORMAL_FIX_LEFT;
        let r = (f32::from(sr) * 1.0) as f64 + DENORMAL_FIX_RIGHT;
        main_l[CENTER + i] = l as f32;
        main_r[CENTER + i] = r as f32;
        main_m[CENTER + i] = ((l + r) * 0.5) as f32;
        main_s[CENTER + i] = ((l - r) * 0.5) as f32;
    }
    (requested, silence)
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn bench_stages_q5() {
    // Representative workload: 10 s deterministic noise, q5/44.1 kHz.
    let pcm = gen_noise(441000);
    // Warm-up (also primes allocator/code pages).
    {
        let mut totals = StageTotals::default();
        let bytes = encode_staged(5.0, 44100, &pcm, &mut totals);
        std::hint::black_box(bytes);
    }
    // Self-check: the manual loop must reproduce `encode()` byte-for-byte.
    let mut reference = MusepackEncoder::new(EncoderConfig::new(5.0, 44100, 2)).unwrap();
    let expected = reference.encode(&pcm).unwrap();
    let mut totals = StageTotals::default();
    let actual = encode_staged(5.0, 44100, &pcm, &mut totals);
    assert_eq!(actual, expected, "staged loop diverged from encode()");
    std::hint::black_box(&actual);

    let total = totals.total();
    let pct = |d: Duration| 100.0 * d.as_secs_f64() / total.as_secs_f64();
    println!("stage breakdown for 10 s noise, q5/44.1k (single warm-up run):");
    for (name, d) in [
        ("PCM/frame preparation", totals.pcm_prep),
        ("AnalysisFilterbank", totals.filterbank),
        ("PsychoacousticModel", totals.psy),
        ("RaiseSMR", totals.raise_smr),
        ("MS_LR_Entscheidung", totals.ms_decision),
        ("SCF extraction", totals.scf),
        ("Transient analysis", totals.transient_calc),
        ("NS analysis", totals.ns_analyse),
        ("Allocation/PNS", totals.allocate),
        ("Quantisation", totals.quantize),
        ("Huffman/AP coding", totals.huffman_ap),
        ("SV8/container", totals.container),
    ] {
        println!("  {:<22} {:>10.3?} {:>5.1}%", name, d, pct(d));
    }
    println!("  {:<22} {:>10.3?}", "TOTAL (stages)", total);
}

/// Resolution distribution: which `encode_samples` paths dominate? Uses the
/// traced encoder; no codec changes.
#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn bench_res_distribution() {
    use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};
    use std::collections::BTreeMap;

    for (label, quality, pcm) in [
        ("noise-q5", 5.0f32, gen_noise(44100)),
        (
            "sine-q5",
            5.0,
            gen_sine(44100, 44100, 1000.0, 1000.0, 20000.0),
        ),
        ("transient-q5", 5.0, gen_transient(44100)),
        ("noise-q7", 7.0, gen_noise(44100)),
    ] {
        let mut enc = MusepackEncoder::new(EncoderConfig::new(quality, 44100, 2)).unwrap();
        let (_, trace) = enc.encode_traced(&pcm).unwrap();
        let mut hist: BTreeMap<i32, usize> = BTreeMap::new();
        let mut bands = 0usize;
        for t in &trace {
            if !t.processed {
                continue;
            }
            for b in 0..32 {
                *hist.entry(t.res_l[b]).or_default() += 1;
                *hist.entry(t.res_r[b]).or_default() += 1;
                bands += 2;
            }
        }
        let ge9: usize = hist.iter().filter(|(r, _)| **r >= 9).map(|(_, c)| c).sum();
        println!("{label}: {bands} band-channels over {} frames", trace.len());
        println!("  Res histogram: {hist:?}");
        println!(
            "  Res>=9: {ge9} ({:.2}%), Res<=0: {} ({:.2}%)",
            100.0 * ge9 as f64 / bands as f64,
            hist.iter()
                .filter(|(r, _)| **r <= 0)
                .map(|(_, c)| c)
                .sum::<usize>(),
            100.0
                * hist
                    .iter()
                    .filter(|(r, _)| **r <= 0)
                    .map(|(_, c)| c)
                    .sum::<usize>() as f64
                / bands as f64,
        );
    }
}
