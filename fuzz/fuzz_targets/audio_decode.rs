//! Fuzz target: the audio decode seam (WAV reader and FLAC adapter).
//!
//! Properties under test:
//!
//! - no panic on arbitrary bytes (including inside the third-party FLAC
//!   decoder, which the adapter catches);
//! - termination (every read either advances the source or reports EOF);
//! - bounded resource use: the only buffers sized from input are the
//!   caller's fixed decode buffers, never a declared audio length;
//! - checked chunk arithmetic in the WAV scanner (no overflow / wrap);
//! - a decoder that opens always reaches `Ok(0)` (or an error), never spins.
#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;

const FRAMES: usize = 1152;

fn drain_f32(data: &[u8]) {
    let Ok(mut decoder) = musicpack_core::audio::open(Box::new(Cursor::new(data.to_vec()))) else {
        return;
    };
    let channels = decoder.info().channels as usize;
    let mut buffer = vec![0.0f32; FRAMES * channels];
    loop {
        match decoder.read_f32(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fn drain_s32(data: &[u8]) {
    let Ok(mut decoder) = musicpack_core::audio::open(Box::new(Cursor::new(data.to_vec()))) else {
        return;
    };
    let channels = decoder.info().channels as usize;
    let mut buffer = vec![0i32; FRAMES * channels];
    loop {
        match decoder.read_s32(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fuzz_target!(|data: &[u8]| {
    drain_f32(data);
    drain_s32(data);
});
