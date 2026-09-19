//! Streaming demux proof: the Musepack decoder consumes a bounded incremental
//! `Read` and produces exactly the reference PCM (Phase 13B oracle hashes),
//! without materialising the whole compressed member.

use std::io::{self, Read};

use musicpack_core::audio::musepack::MpcDecoder;
use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::json::{self, Value};

const FIXTURES: [&str; 6] = [
    "sine32-q8.mpc",
    "sine37-q4.mpc",
    "sine44-q5.mpc",
    "sine44-q7.mpc",
    "sine44-q5-48s.mpc",
    "sine48-q6.mpc",
];

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/musepack/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn oracle() -> Vec<(String, String)> {
    let path = format!(
        "{}/tests/data/musepack_oracle.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(path).expect("oracle");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let v: Value = json::parse(line.as_bytes()).unwrap();
            let file = match v.get("file") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let sha = match v.get("pcmSha256") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            (file, sha)
        })
        .collect()
}

/// A deliberately awkward reader: at most `max` bytes per call, counting.
struct ChunkedReader {
    data: Vec<u8>,
    pos: usize,
    max: usize,
    fail_at: Option<usize>,
}

impl ChunkedReader {
    fn new(data: Vec<u8>, max: usize) -> Self {
        Self {
            data,
            pos: 0,
            max,
            fail_at: None,
        }
    }
    fn failing_at(mut self, at: usize) -> Self {
        self.fail_at = Some(at);
        self
    }
}

impl Read for ChunkedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(at) = self.fail_at {
            if self.pos >= at {
                return Err(io::Error::other("injected source error"));
            }
        }
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        let n = self.max.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn decode_all(decoder: &mut MpcDecoder) -> Vec<f32> {
    let channels = decoder.info().channels as usize;
    let mut pcm = Vec::new();
    let mut buffer = vec![0.0f32; 1152 * channels];
    loop {
        let frames = decoder.read_f32(&mut buffer).expect("decode");
        if frames == 0 {
            break;
        }
        pcm.extend_from_slice(&buffer[..frames * channels]);
    }
    pcm
}

fn canonical(pcm: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(pcm.len() * 4);
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    sha256_hex(&bytes)
}

#[test]
fn chunked_streaming_matches_the_reference_oracle() {
    let oracle = oracle();
    let sizes = [1usize, 2, 3, 7, 17, 31, 64, 127, 257, 1024, usize::MAX];
    for name in FIXTURES {
        let bytes = fixture(name);
        let want = &oracle
            .iter()
            .find(|(f, _)| f == name)
            .unwrap_or_else(|| panic!("{name} missing from oracle"))
            .1;
        for &size in &sizes {
            let reader = ChunkedReader::new(bytes.clone(), size);
            let mut decoder = MpcDecoder::from_reader(Box::new(reader)).unwrap();
            let pcm = decode_all(&mut decoder);
            assert_eq!(
                canonical(&pcm),
                *want,
                "{name} at chunk size {size}: PCM differs from the reference"
            );
        }
    }
}

#[test]
fn open_consumes_far_less_than_the_whole_member() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let size = bytes.len();
        let reader = ChunkedReader::new(bytes, 4096);
        let mut decoder = MpcDecoder::from_reader(Box::new(reader)).unwrap();
        // Only header blocks (plus the first AP header) have been read.
        let at_open = decoder.bytes_consumed();
        assert!(
            at_open < size,
            "{name}: open consumed the whole member ({at_open} >= {size})"
        );
        // Progressive consumption: decoding more reads more.
        let mut buffer = vec![0.0f32; 1152 * decoder.info().channels as usize];
        decoder.read_f32(&mut buffer).unwrap();
        assert!(decoder.bytes_consumed() > at_open, "{name}: no progress");
    }
}

#[test]
fn large_fixture_streams_with_a_bounded_buffer() {
    let bytes = fixture("sine44-q5-48s.mpc");
    let size = bytes.len();
    let reader = ChunkedReader::new(bytes, 997);
    let mut decoder = MpcDecoder::from_reader(Box::new(reader)).unwrap();
    let at_open = decoder.bytes_consumed();
    assert!(at_open < 4096, "header phase read {at_open} bytes");
    let pcm = decode_all(&mut decoder);
    assert!(!pcm.is_empty());
    // The compressed member is never held whole; only one bounded block buffer
    // exists (asserted by the decoder's `MAX_BLOCK_BYTES` cap at parse time).
    assert!(decoder.bytes_consumed() <= size);
    assert!(
        decoder.buffered_block_bytes() <= musicpack_core::audio::musepack::decoder::MAX_BLOCK_BYTES
    );
}

#[test]
fn truncation_and_source_errors_never_panic() {
    let bytes = fixture("sine44-q5.mpc");
    // Every truncation either errors cleanly or decodes partially.
    for cut in (0..bytes.len()).step_by(29) {
        let reader = ChunkedReader::new(bytes[..cut].to_vec(), 17);
        if let Ok(mut decoder) = MpcDecoder::from_reader(Box::new(reader)) {
            let mut buffer = vec![0.0f32; 2048];
            let _ = decoder.read_f32(&mut buffer);
        }
    }
    // A source error mid-stream surfaces as an error, not a silent EOF.
    let reader = ChunkedReader::new(bytes.clone(), 31).failing_at(bytes.len() / 2);
    let mut decoder = MpcDecoder::from_reader(Box::new(reader)).expect("open");
    let mut buffer = vec![0.0f32; 2048];
    let mut saw_error = false;
    for _ in 0..10_000 {
        match decoder.read_f32(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                saw_error = true;
                break;
            }
        }
    }
    assert!(saw_error, "injected source error must surface");
}
