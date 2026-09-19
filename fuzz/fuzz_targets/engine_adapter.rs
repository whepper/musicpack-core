//! Fuzz target: the Phase 11 engine adapter.
//!
//! Properties under test:
//!
//! - no panic on arbitrary bytes fed to the ring/resampler/mixer/session;
//! - termination (pump/consume always make progress or report EOF);
//! - bounded memory (fixed-capacity rings, bounded chunks);
//! - frame arithmetic never overflows into invalid indexing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use musicpack_engine::{Mixer, RingBuffer, StreamingResampler, mix_interleaved};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let channels = (data[0] % 8) as usize + 1;
    let source_rate = 8000u32 + (data.get(1).copied().unwrap_or(0) as u32) * 997;
    let output_rate = 8000u32 + (data.get(2).copied().unwrap_or(0) as u32) * 1013;
    let capacity = (data.get(3).copied().unwrap_or(1) as usize % 512) + 1;

    // Ring write/read under arbitrary chunk sizes.
    let Some(mut ring) = RingBuffer::new(capacity, channels) else {
        return;
    };
    let mut offset = 4usize;
    let mut guard = 0usize;
    while offset < data.len() && guard < 4096 {
        let frames = (data[offset] as usize % 37) + 1;
        offset += 1;
        let samples: Vec<f32> = (0..frames * channels)
            .map(|i| data.get((offset + i) % data.len()).copied().unwrap_or(0) as f32 / 128.0 - 1.0)
            .collect();
        ring.write_interleaved(&samples);
        let mut out = vec![0.0f32; frames * channels];
        ring.read_interleaved(&mut out, frames);
        ring.continue_playhead_from((data[offset % data.len()] as u64) % 7);
        guard += 1;
    }

    // Resampler across arbitrary chunk boundaries.
    if let Some(mut resampler) =
        StreamingResampler::new(source_rate, channels, output_rate, channels)
    {
        let mut ring = RingBuffer::new(capacity.max(1) * 4, channels).unwrap();
        let source: Vec<f32> = data.iter().map(|b| *b as f32 / 128.0 - 1.0).collect();
        let total_frames = source.len() / channels;
        let mut frame_offset = 0usize;
        for _ in 0..256 {
            let before = frame_offset;
            frame_offset = resampler.process(&source, frame_offset, &mut ring, 64, 64);
            let _ = resampler.finish(&mut ring, 64);
            let mut out = vec![0.0f32; 64 * channels];
            ring.read_interleaved(&mut out, 64);
            if frame_offset == before {
                // Either EOF or the ring was full before draining; both are
                // terminal for this bounded loop.
                break;
            }
            if frame_offset >= total_frames {
                let _ = resampler.finish(&mut ring, usize::MAX);
                break;
            }
        }
    }

    // Mixer over arbitrary fade windows.
    let fade = (data[0] as u64) * 3 + 1;
    let mut mixer = Mixer::new(fade);
    mixer.start(0);
    let mut mixed = vec![0.0f32; channels];
    let cur: Vec<f32> = data.iter().take(channels).map(|b| *b as f32).collect();
    let nxt: Vec<f32> = data.iter().rev().take(channels).map(|b| *b as f32).collect();
    for _ in 0..fade.min(10_000) {
        let Some((og, ig)) = mixer.next_gains() else {
            break;
        };
        mix_interleaved(
            &mut mixed,
            if cur.is_empty() { None } else { Some(&cur) },
            if nxt.is_empty() { None } else { Some(&nxt) },
            og,
            ig,
        );
    }
});
