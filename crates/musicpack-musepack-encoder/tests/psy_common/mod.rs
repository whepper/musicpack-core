//! Shared helpers for the psychoacoustic oracle tests: deterministic PCM
//! generation and fixture parsing. Mirrors `tools/extract_psy_oracle.c`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Reference block size.
pub const BLOCK: usize = 1152;
/// Reference centering offset.
pub const CENTER: usize = 448;
/// Analysis buffer length (`ANABUFFER`).
pub const ANALYSE_BUFFER: usize = 1600;

/// A psychoacoustic oracle case.
#[derive(Clone, Debug)]
pub struct Case {
    /// `config_case` identifier.
    pub name: String,
    /// Frozen configuration name.
    pub config: String,
    /// Quality.
    pub qual: f32,
    /// Sample rate.
    pub rate: f32,
    /// Number of frames.
    pub frames: usize,
    /// Profile `Max_Band`.
    pub max_band: i32,
    /// `MS_Channelmode`.
    pub ms_mode: i32,
    /// `CVD_used`.
    pub cvd: i32,
    /// `NS_Order`.
    pub ns_order: i32,
    /// `minSMR`.
    pub min_smr: f32,
}

/// The test data directory.
#[must_use]
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/psy")
}

/// Parses `manifest.txt` into case metadata.
#[must_use]
pub fn read_manifest() -> Vec<Case> {
    let text = std::fs::read_to_string(data_dir().join("manifest.txt")).expect("psy manifest");
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        out.push(Case {
            name: f[0].to_string(),
            config: f[1].to_string(),
            qual: f[2].parse().unwrap(),
            rate: f[3].parse().unwrap(),
            frames: f[4].parse().unwrap(),
            max_band: f[5].parse().unwrap(),
            ms_mode: f[6].parse().unwrap(),
            cvd: f[7].parse().unwrap(),
            ns_order: f[8].parse().unwrap(),
            min_smr: f[9].parse().unwrap(),
        });
    }
    out
}

/// Reads a little-endian `f32` fixture.
#[must_use]
pub fn read_f32le(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("f32le fixture");
    assert_eq!(bytes.len() % 4, 0);
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Reads a sectioned `name count` / hex-line fixture.
#[must_use]
pub fn parse_hex_sections(text: &str) -> std::collections::HashMap<String, Vec<u32>> {
    let mut out = std::collections::HashMap::new();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut i = 0;
    while i < lines.len() {
        let parts: Vec<&str> = lines[i].split_whitespace().collect();
        assert_eq!(
            parts.len(),
            2,
            "expected a section header, got {:?}",
            lines[i]
        );
        let name = parts[0].to_string();
        let n: usize = parts[1].parse().unwrap();
        let vals: Vec<u32> = (0..n)
            .map(|j| u32::from_str_radix(lines[i + 1 + j].trim(), 16).unwrap())
            .collect();
        out.insert(name, vals);
        i += 1 + n;
    }
    out
}

/// The reference LCG.
#[must_use]
pub fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

/// The reference exact `f32` sample from an LCG draw.
#[must_use]
pub fn fval(state: &mut u32) -> f32 {
    (lcg(state) >> 9) as f32 * (1.0 / 4194304.0) - 1.0
}

fn sine(amp: f32, freq: f64, i: f64) -> f32 {
    amp * (std::f64::consts::TAU * freq * i / 44100.0).sin() as f32
}

/// PCM case kinds, matching the oracle enum order.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Silence,
    Impulse,
    Constant,
    Alternating,
    Sine,
    Multitone,
    Lowfreq,
    Highfreq,
    Noise,
    Transient,
    StereoSame,
    StereoDiff,
    LeftOnly,
    RightOnly,
    PhaseInvert,
    Ramp,
}

impl Kind {
    /// Parses the oracle case name.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "silence" => Kind::Silence,
            "impulse" => Kind::Impulse,
            "constant" => Kind::Constant,
            "alternating" => Kind::Alternating,
            "sine" => Kind::Sine,
            "multitone" => Kind::Multitone,
            "lowfreq" => Kind::Lowfreq,
            "highfreq" => Kind::Highfreq,
            "noise" => Kind::Noise,
            "transient" => Kind::Transient,
            "stereo_same" => Kind::StereoSame,
            "stereo_diff" => Kind::StereoDiff,
            "left_only" => Kind::LeftOnly,
            "right_only" => Kind::RightOnly,
            "phase_invert" => Kind::PhaseInvert,
            "ramp" => Kind::Ramp,
            other => panic!("unknown case kind {other}"),
        }
    }
}

/// Generates the global PCM arrays exactly as the oracle does.
#[must_use]
pub fn generate(kind: Kind, total: usize) -> (Vec<f32>, Vec<f32>) {
    let mut sl = 0x1234_5678u32;
    let mut sr = 0x9ABC_DEF0u32;
    let mut gl = Vec::with_capacity(total);
    let mut gr = Vec::with_capacity(total);
    for i in 0..total {
        let id = i as f64;
        gl.push(left(kind, id, &mut sl));
        gr.push(right(kind, id, &mut sr));
    }
    (gl, gr)
}

fn left(kind: Kind, i: f64, sl: &mut u32) -> f32 {
    match kind {
        Kind::Silence => 0.0,
        Kind::Impulse => {
            if (400.0..401.0).contains(&i) {
                1.0
            } else {
                0.0
            }
        }
        Kind::Constant => 0.5,
        Kind::Alternating => {
            if (i as i64) & 1 != 0 {
                -0.5
            } else {
                0.5
            }
        }
        Kind::Sine => sine(0.5, 1000.0, i),
        Kind::Multitone => sine(0.25, 440.0, i) + sine(0.25, 3000.0, i),
        Kind::Lowfreq => sine(0.6, 60.0, i),
        Kind::Highfreq => sine(0.6, 12000.0, i),
        Kind::Noise => fval(sl),
        Kind::Transient => {
            if (0.0..1.0).contains(&i) {
                1.0
            } else if (500.0..501.0).contains(&i) {
                -0.5
            } else if (1000.0..1001.0).contains(&i) {
                0.75
            } else {
                0.0
            }
        }
        Kind::StereoSame | Kind::StereoDiff => sine(0.4, 700.0, i),
        Kind::LeftOnly | Kind::PhaseInvert => sine(0.5, 900.0, i),
        Kind::RightOnly => 0.0,
        Kind::Ramp => ((i as i64 % 512) - 256) as f32 / 512.0,
    }
}

fn right(kind: Kind, i: f64, sr: &mut u32) -> f32 {
    match kind {
        Kind::Noise => fval(sr),
        Kind::StereoSame => sine(0.4, 700.0, i),
        Kind::StereoDiff => sine(0.4, 1100.0, i),
        Kind::LeftOnly => 0.0,
        Kind::RightOnly => sine(0.5, 900.0, i),
        Kind::PhaseInvert => -sine(0.5, 900.0, i),
        other => left(other, i, sr),
    }
}

/// The analysis window for frame `f`: `[g[base]] * CENTER ++ g[base..base+BLOCK]`.
#[must_use]
pub fn frame_window(g: &[f32], f: usize) -> Vec<f32> {
    let base = f * BLOCK;
    let mut w = vec![g[base]; CENTER];
    w.extend_from_slice(&g[base..base + BLOCK]);
    w
}
