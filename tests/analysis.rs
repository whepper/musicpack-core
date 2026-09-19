//! Phase 9 analysis: reference-golden differential for the waveform
//! accumulator and BS.1770 loudness meter, plus chunk invariance, fixture
//! parity and hostile-input coverage.
//!
//! The committed table `tests/data/analysis_c_reference.txt` is produced by
//! `tools/analysis_probe.c`, which compiles the reference's `waveform.c`,
//! `loudness.c` and vendored `ebur128` (see that file's header for the exact
//! regeneration command). Waveform records are compared **byte-for-byte**
//! (payload SHA-256 + first/last bytes + point count); loudness records are
//! compared within ±0.05 LU / ±0.05 dBTP.

use std::io::Cursor;
use std::path::PathBuf;

use musicpack_core::audio::{self, LoudnessMeter, WaveformAccumulator, gain_db};
use musicpack_core::format::checksum::sha256_hex;

const LOUDNESS_TOLERANCE: f64 = 0.05;

fn audio_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference/audio")
}

fn golden_table() -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/analysis_c_reference.txt"),
    )
    .expect("committed analysis golden table")
}

/// Reconstructs the exact interleaved f32 input for a golden spec.
///
/// `flac:<name>` identifies a committed fixture; `synth:<kind>:<rate>:<ch>:<frames>`
/// is the deterministic generator from `tools/analysis_probe.c`.
fn pcm_for(label: &str) -> (Vec<f32>, u32, u8) {
    if label.ends_with(".flac") {
        let bytes = std::fs::read(audio_dir().join(label)).expect("fixture");
        let mut decoder = audio::open(Box::new(Cursor::new(bytes))).expect("flac opens");
        let channels = decoder.info().channels as usize;
        let sample_rate = decoder.info().sample_rate;
        let mut out = Vec::new();
        let mut buffer = vec![0.0f32; 1152 * channels];
        loop {
            let n = decoder.read_f32(&mut buffer).expect("decode");
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n * channels]);
        }
        return (out, sample_rate, decoder.info().channels);
    }

    let spec: Vec<&str> = label.split(':').collect();
    assert_eq!(spec.len(), 5, "synth spec {label}");
    assert_eq!(spec[0], "synth");
    let kind = spec[1];
    let rate: u32 = spec[2].parse().unwrap();
    let channels: u8 = spec[3].parse().unwrap();
    let frames: usize = spec[4].parse().unwrap();
    let mut out = Vec::with_capacity(frames * channels as usize);
    let mut rng: u32 = 0x1234_5678;
    for frame in 0..frames {
        for _ in 0..channels {
            let v = match kind {
                "silence" => 0.0f32,
                "dc" => 0.5,
                "impulse" => {
                    if frame == 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                "prng" => {
                    rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((rng >> 8) as f32) / 8_388_608.0 - 1.0
                }
                other => panic!("unknown synth kind {other}"),
            };
            out.push(v);
        }
    }
    (out, rate, channels)
}

fn waveform_payload(pcm: &[f32], rate: u32, channels: u8) -> Vec<u8> {
    let mut acc = WaveformAccumulator::new(rate, channels).unwrap();
    acc.feed(pcm).unwrap();
    acc.finish()
}

fn loudness_of(pcm: &[f32], rate: u32, channels: u8) -> (f64, f64) {
    let mut meter = LoudnessMeter::new(channels, rate).unwrap();
    meter.process(pcm).unwrap();
    let r = meter.result();
    (r.lufs, r.true_peak_db_tp)
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    assert!(hex.len() % 2 == 0);
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

#[test]
fn waveform_matches_reference_bytes() {
    let mut waveform_cases = 0;
    for line in golden_table().lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"waveform") {
            continue;
        }
        let label = fields[1];
        let points: usize = fields[2].strip_prefix("points=").unwrap().parse().unwrap();
        let sha = fields[3].strip_prefix("sha256=").unwrap();
        let head = hex_bytes(fields[4].strip_prefix("head=").unwrap());
        let tail = hex_bytes(fields[5].strip_prefix("tail=").unwrap());

        let (pcm, rate, channels) = pcm_for(label);
        let payload = waveform_payload(&pcm, rate, channels);
        assert_eq!(payload.len() / 2, points, "{label} point count");
        assert_eq!(sha256_hex(&payload), sha, "{label} payload digest");
        assert_eq!(
            &payload[..head.len().min(payload.len())],
            &head[..head.len().min(payload.len())]
        );
        if payload.len() >= tail.len() {
            assert_eq!(
                &payload[payload.len() - tail.len()..],
                &tail[..],
                "{label} tail"
            );
        }
        waveform_cases += 1;
    }
    assert!(waveform_cases >= 14, "expected a broad golden corpus");
}

#[test]
fn loudness_matches_reference_within_tolerance() {
    let mut loudness_cases = 0;
    for line in golden_table().lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"loudness") {
            continue;
        }
        let label = fields[1];
        let lufs: f64 = fields[2].strip_prefix("lufs=").unwrap().parse().unwrap();
        let tp: f64 = fields[3].strip_prefix("tp=").unwrap().parse().unwrap();

        let (pcm, rate, channels) = pcm_for(label);
        let (got_lufs, got_tp) = loudness_of(&pcm, rate, channels);
        assert!(
            (got_lufs - lufs).abs() <= LOUDNESS_TOLERANCE,
            "{label} lufs: got {got_lufs}, reference {lufs}"
        );
        assert!(
            (got_tp - tp).abs() <= LOUDNESS_TOLERANCE,
            "{label} tp: got {got_tp}, reference {tp}"
        );
        loudness_cases += 1;
    }
    assert!(loudness_cases >= 14, "expected a broad golden corpus");
}

#[test]
fn waveform_is_chunk_invariant_on_fixtures_and_synth() {
    let (fixture_pcm, rate, channels) = pcm_for("flac16-44k.flac");
    let synth = pcm_for("synth:prng:44100:2:88200").0;
    for (name, pcm) in [("fixture", fixture_pcm), ("synth", synth)] {
        let reference = waveform_payload(&pcm, rate, 2);
        let stride = channels as usize;
        for chunk in [4096usize, 1024, 137, 1] {
            let mut acc = WaveformAccumulator::new(rate, 2).unwrap();
            for part in pcm.chunks(chunk * stride) {
                acc.feed(part).unwrap();
            }
            assert_eq!(acc.finish(), reference, "{name} chunk {chunk}");
        }
    }
}

#[test]
fn loudness_is_chunk_invariant_bit_for_bit() {
    let (pcm, rate, channels) = pcm_for("synth:prng:44100:2:88200");
    let reference = loudness_of(&pcm, rate, channels);
    for chunk in [4096usize, 1024, 137, 1] {
        let mut meter = LoudnessMeter::new(channels, rate).unwrap();
        for part in pcm.chunks(chunk * channels as usize) {
            meter.process(part).unwrap();
        }
        let r = meter.result();
        assert_eq!(r.lufs.to_bits(), reference.0.to_bits(), "chunk {chunk}");
        assert_eq!(
            r.true_peak_db_tp.to_bits(),
            reference.1.to_bits(),
            "chunk {chunk}"
        );
    }
}

#[test]
fn waveform_fixture_parity_and_buckets() {
    // 2 s fixtures: 20 buckets (10/s), including the final partial bucket.
    for name in [
        "flac16-44k.flac",
        "flac24-48k.flac",
        "flac24-96k.flac",
        "flac-mono-44k.flac",
    ] {
        let (pcm, rate, channels) = pcm_for(name);
        let payload = waveform_payload(&pcm, rate, channels);
        assert_eq!(payload.len() / 2, 20, "{name}");
    }
}

#[test]
fn truncated_wav_feeds_cleanly() {
    let bytes = std::fs::read(audio_dir().join("wav-truncated.wav")).unwrap();
    let mut decoder = audio::open(Box::new(Cursor::new(bytes))).unwrap();
    let info = decoder.info().clone();
    let mut acc = WaveformAccumulator::new(info.sample_rate, info.channels).unwrap();
    let mut meter = LoudnessMeter::new(info.channels, info.sample_rate).unwrap();
    let mut buffer = vec![0.0f32; 1152 * info.channels as usize];
    let mut frames = 0u64;
    loop {
        let n = decoder.read_f32(&mut buffer).expect("decode");
        if n == 0 {
            break;
        }
        acc.feed(&buffer[..n * info.channels as usize]).unwrap();
        meter
            .process(&buffer[..n * info.channels as usize])
            .unwrap();
        frames += n as u64;
    }
    assert!(frames > 0 && frames < 88200);
    let payload = acc.finish();
    assert!(payload.len() / 2 > 0);
    let _ = meter.result();
}

#[test]
fn album_semantics_are_caller_policy() {
    let (a, rate, channels) = pcm_for("synth:prng:44100:2:88200");
    let (b, _, _) = pcm_for("synth:prng:44100:2:88200");

    let mut album = LoudnessMeter::new(channels, rate).unwrap();
    album.process(&a).unwrap();
    album.process(&b).unwrap();
    let sequential = album.result();

    let mut joined = a.clone();
    joined.extend_from_slice(&b);
    let mut concat = LoudnessMeter::new(channels, rate).unwrap();
    concat.process(&joined).unwrap();
    assert!((sequential.lufs - concat.result().lufs).abs() < 0.01);
}

#[test]
fn true_peak_degrades_to_sample_peak_at_high_rates() {
    // The reference disables interpolation at >=192 kHz; the reported true
    // peak is then the sample peak. Both remain finite and comparable.
    let (pcm, rate, channels) = pcm_for("synth:prng:192000:2:960000");
    let (_, tp) = loudness_of(&pcm, rate, channels);
    let sample_peak = pcm.iter().fold(0.0f64, |m, &s| m.max(s.abs() as f64));
    let sample_db = 20.0 * sample_peak.log10();
    assert!((tp - sample_db).abs() < 1e-9);
}

#[test]
fn gain_is_the_target_minus_measurement() {
    assert_eq!(gain_db(-12.0, -16.0), -4.0);
    assert!((gain_db(-7.19, -16.0) - -8.81).abs() < 1e-12);
}
