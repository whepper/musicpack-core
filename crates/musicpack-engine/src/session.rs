//! One decoder + resampler + bounded ring.
//!
//! Port of the per-track decode path of the reference browser engine:
//! `decoder.worker.js` decodes fixed chunks (8 × 1152 source frames) which the
//! worklet resamples into a bounded ring under watermark backpressure. EOF,
//! short reads, and the resampler tail flush are preserved.

use musicpack_core::audio::AudioDecoder;
use musicpack_core::player::engine::EngineError;
use musicpack_core::player::types::StreamInfo;

use crate::resampler::StreamingResampler;
use crate::ring::RingBuffer;

/// Source frames decoded per pump chunk (`8 * 1152`, the reference worker's
/// `FRAMES_PER_CHUNK`).
pub const DECODE_CHUNK_FRAMES: usize = 8 * 1152;

/// Main output ring capacity in seconds (`worklet-protocol.ts` `RING_SECONDS`).
pub const RING_SECONDS: f64 = 8.0;
/// High-water fill fraction: at or above, decoding pauses.
pub const HIGH_WATER: f64 = 0.8;
/// Low-water fill fraction: below, decoding resumes.
pub const LOW_WATER: f64 = 0.2;
/// Fill fraction considered "primed".
pub const PRIME_FRACTION: f64 = 0.2;
/// Extra frames decoded beyond a fade window so the lane primes fully.
pub const CALLBACK_MARGIN_FRAMES: u64 = 2048;

/// A decode session for one track.
pub struct DecodeSession {
    decoder: Box<dyn AudioDecoder>,
    ring: RingBuffer,
    resampler: StreamingResampler,
    decode_buf: Vec<f32>,
    pending: Vec<f32>,
    pending_offset: usize,
    eos: bool,
    tail_done: bool,
    declared_source_frames: Option<u64>,
    source_rate: u32,
    source_channels: usize,
    output_rate: u32,
    output_channels: usize,
}

impl DecodeSession {
    /// Opens a session over an already-constructed decoder.
    ///
    /// The decoder's format is normalized to `output_rate`/`output_channels`.
    /// Channel counts outside 1–2 are rejected (the engine supports mono and
    /// stereo only, like the reference pipeline).
    pub fn new(
        decoder: Box<dyn AudioDecoder>,
        output_rate: u32,
        output_channels: usize,
        ring_capacity_frames: usize,
    ) -> Result<Self, EngineError> {
        let info = decoder.info();
        if !(1..=2).contains(&info.channels) {
            return Err(EngineError(format!(
                "unsupported channel count: {}",
                info.channels
            )));
        }
        let source_rate = info.sample_rate;
        let source_channels = info.channels as usize;
        let declared_source_frames = info.total_frames;
        let resampler =
            StreamingResampler::new(source_rate, source_channels, output_rate, output_channels)
                .ok_or_else(|| EngineError("invalid resampler parameters".into()))?;
        let ring = RingBuffer::new(ring_capacity_frames, output_channels)
            .ok_or_else(|| EngineError("invalid ring capacity".into()))?;
        Ok(Self {
            decoder,
            ring,
            resampler,
            decode_buf: vec![0.0; DECODE_CHUNK_FRAMES * source_channels],
            pending: Vec::new(),
            pending_offset: 0,
            eos: false,
            tail_done: false,
            declared_source_frames,
            source_rate,
            source_channels,
            output_rate,
            output_channels,
        })
    }

    /// Output-pipeline stream facts (normalized to the output rate).
    ///
    /// `length_samples` is `ceil(source_frames * output_rate / source_rate)`,
    /// or 0 when the source length is unknown.
    pub fn info(&self) -> StreamInfo {
        let length_samples = match self.declared_source_frames {
            Some(src) if self.source_rate > 0 => {
                let out =
                    (src as u128 * self.output_rate as u128).div_ceil(self.source_rate as u128);
                out.min(u64::MAX as u128) as u64
            }
            _ => 0,
        };
        StreamInfo {
            rate: self.output_rate,
            channels: self.output_channels as u32,
            version: 0,
            length_samples,
        }
    }

    /// Source sample rate.
    pub fn source_rate(&self) -> u32 {
        self.source_rate
    }

    /// Source channel count.
    pub fn source_channels(&self) -> usize {
        self.source_channels
    }

    /// Output sample rate.
    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Output channel count.
    pub fn output_channels(&self) -> usize {
        self.output_channels
    }

    /// The bounded output ring.
    pub fn ring(&self) -> &RingBuffer {
        &self.ring
    }

    /// Mutable access to the output ring (draining, playhead rebase).
    pub fn ring_mut(&mut self) -> &mut RingBuffer {
        &mut self.ring
    }

    /// True once the decoder reported clean EOF.
    pub fn eos(&self) -> bool {
        self.eos
    }

    /// True while a decoded source chunk is still being resampled.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Output frames consumed since the last ring reset (the rendered
    /// playhead).
    pub fn rendered_frames(&self) -> u64 {
        self.ring.rendered_frames()
    }

    /// True when the decoder is exhausted, the resampler tail flushed, and
    /// the ring drained — the track's audible end.
    pub fn exhausted(&self) -> bool {
        self.eos
            && self.resampler.is_finished()
            && self.pending.is_empty()
            && self.ring.available_frames() == 0
    }

    /// Decodes/resamples as much as `max_output_frames` allows, bounded by the
    /// ring's free space. Returns `Err` on a decoder failure.
    pub fn pump(&mut self, max_output_frames: usize) -> Result<(), EngineError> {
        if self.eos {
            if !self.tail_done && self.resampler.finish(&mut self.ring, max_output_frames) {
                self.tail_done = true;
            }
            return Ok(());
        }
        if self.pending.is_empty() {
            let frames = self
                .decoder
                .read_f32(&mut self.decode_buf)
                .map_err(|e| EngineError(e.to_string()))?;
            if frames == 0 {
                self.eos = true;
                return self.pump(max_output_frames);
            }
            let total = frames * self.source_channels;
            self.pending.clear();
            self.pending.extend_from_slice(&self.decode_buf[..total]);
            self.pending_offset = 0;
        }
        let total_frames = self.pending.len() / self.source_channels;
        self.pending_offset = self.resampler.process(
            &self.pending,
            self.pending_offset,
            &mut self.ring,
            usize::MAX,
            max_output_frames,
        );
        if self.pending_offset >= total_frames {
            self.pending.clear();
            self.pending_offset = 0;
        }
        Ok(())
    }

    /// Fill fraction of the output ring (`available / capacity`).
    pub fn fill_fraction(&self) -> f64 {
        self.ring.available_frames() as f64 / self.ring.capacity() as f64
    }

    /// Rebuilds the ring so its reported playhead is zero while preserving the
    /// frames currently buffered.
    ///
    /// Used after a reopen+skip seek: the skipped frames are gone from the
    /// decoder, and the ring's playhead must restart at the seek target (the
    /// player carries the absolute position via its reset offset). One copy of
    /// at most the ring capacity; documented as the cost of codec-neutral
    /// seeking.
    pub fn rebase_playhead(&mut self) {
        let channels = self.output_channels;
        let capacity = self.ring.capacity();
        let available = self.ring.available_frames() as usize;
        let mut buffered = vec![0.0f32; available * channels];
        self.ring.read_interleaved(&mut buffered, available);
        self.ring = RingBuffer::new(capacity, channels).expect("valid ring parameters");
        self.ring.write_interleaved(&buffered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicpack_core::audio::AudioInfo;
    use musicpack_core::audio::Codec;
    use musicpack_core::error::Result as CoreResult;

    /// A deterministic ramp decoder: `frames` source frames of `channels`.
    struct RampDecoder {
        info: AudioInfo,
        remaining: u64,
        pos: u64,
    }

    impl RampDecoder {
        fn new(rate: u32, channels: u8, frames: u64) -> Self {
            Self {
                info: AudioInfo {
                    sample_rate: rate,
                    channels,
                    bits_per_sample: 16,
                    total_frames: Some(frames),
                    codec: Codec::Wav,
                    is_float: false,
                },
                remaining: frames,
                pos: 0,
            }
        }
    }

    impl AudioDecoder for RampDecoder {
        fn info(&self) -> &AudioInfo {
            &self.info
        }
        fn read_f32(&mut self, out: &mut [f32]) -> CoreResult<usize> {
            let channels = self.info.channels as usize;
            let want = (out.len() / channels).min(self.remaining as usize);
            for frame in 0..want {
                for c in 0..channels {
                    out[frame * channels + c] = (self.pos + frame as u64) as f32 + c as f32 * 0.5;
                }
            }
            self.pos += want as u64;
            self.remaining -= want as u64;
            Ok(want)
        }
        fn read_s32(&mut self, _out: &mut [i32]) -> CoreResult<usize> {
            Err(musicpack_core::error::Error::Unsupported { what: "s32".into() })
        }
    }

    #[test]
    fn session_pumps_to_eof_and_normalizes_length() {
        let decoder = Box::new(RampDecoder::new(44_100, 1, 441));
        let mut session = DecodeSession::new(decoder, 48_000, 1, 48_000 * 8).unwrap();
        assert_eq!(session.info().length_samples, 480);
        let mut guard = 0;
        while !session.eos() && guard < 1000 {
            let free = session.ring().free_frames() as usize;
            session.pump(free).unwrap();
            guard += 1;
        }
        assert!(session.eos());
        assert_eq!(session.ring().available_frames(), 480);
        // Draining the buffered output completes the track.
        let mut drained = vec![0.0f32; 480];
        session.ring_mut().read_interleaved(&mut drained, 480);
        assert!(session.exhausted());
    }

    #[test]
    fn rejects_more_than_two_channels() {
        let decoder = Box::new(RampDecoder::new(44_100, 6, 10));
        assert!(DecodeSession::new(decoder, 44_100, 2, 1000).is_err());
    }

    #[test]
    fn unknown_length_is_zero() {
        let mut decoder = Box::new(RampDecoder::new(44_100, 2, 10));
        decoder.info.total_frames = None;
        let session = DecodeSession::new(decoder, 44_100, 2, 1000).unwrap();
        assert_eq!(session.info().length_samples, 0);
    }
}
