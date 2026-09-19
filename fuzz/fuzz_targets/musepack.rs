//! Fuzz target: the Musepack SV8 parser AND streaming decoder.
//!
//! The corpus seeds are the committed fixtures; mutations exercise the real
//! bitstream/requantisation/synthesis path through a deliberately awkward
//! short-read source (bounded incremental input). Properties: no panic, no
//! hang, bounded resource use, clean error/EOF.
#![no_main]

use std::io::{self, Read};

use libfuzzer_sys::fuzz_target;
use musicpack_core::audio;
use musicpack_core::audio::musepack::sv8;

/// Returns at most `max` bytes per call so whole-stream buffering cannot hide.
struct ShortRead {
    data: Vec<u8>,
    pos: usize,
    max: usize,
}

impl Read for ShortRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        let n = self.max.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fuzz_target!(|data: &[u8]| {
    // Container/metadata parsing on any input.
    let _ = sv8::parse(data);
    let _ = sv8::audio_blocks(data);

    let max = 1 + (data.get(4).copied().unwrap_or(0) as usize % 64);
    let read = Box::new(ShortRead {
        data: data.to_vec(),
        pos: 0,
        max,
    });

    if data.starts_with(b"MPCK") {
        if let Ok(mut decoder) = audio::open(read) {
            let mut buffer = vec![0.0f32; 2048];
            let mut total = 0usize;
            for _ in 0..64 {
                match decoder.read_f32(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(frames) => {
                        total += frames;
                        if total > 4_000_000 {
                            break;
                        }
                    }
                }
            }
            let mut i32buf = vec![0i32; 32];
            let _ = decoder.read_s32(&mut i32buf);
        }
    } else {
        let _ = audio::open(read);
    }
});
