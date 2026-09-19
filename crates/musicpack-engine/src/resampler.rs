//! Stateful linear sample-rate conversion into a fixed-format output ring.
//!
//! Port of `web/app/src/lib/playback/streaming-resampler.ts`
//! (BSD-3-Clause). This is deliberately the same simple linear resampler the
//! reference browser engine uses: compatibility with its output (including
//! the `ceil(source * output / source_rate)` length normalization and its
//! channel truncation/duplication) is the requirement, not audio quality.

use super::ring::RingBuffer;

/// A streaming `f32` resampler.
pub struct StreamingResampler {
    source_rate: u32,
    source_channels: usize,
    output_rate: u32,
    output_channels: usize,
    step: f64,
    previous: Vec<f32>,
    output_frame: Vec<f32>,
    have_previous: bool,
    source_frames_seen: u64,
    output_frames_written: u64,
    done: bool,
}

impl StreamingResampler {
    /// Creates a resampler, or `None` when any rate/channel count is zero.
    pub fn new(
        source_rate: u32,
        source_channels: usize,
        output_rate: u32,
        output_channels: usize,
    ) -> Option<Self> {
        if source_rate == 0 || output_rate == 0 || source_channels == 0 || output_channels == 0 {
            return None;
        }
        Some(Self {
            source_rate,
            source_channels,
            output_rate,
            output_channels,
            step: source_rate as f64 / output_rate as f64,
            previous: vec![0.0; source_channels],
            output_frame: vec![0.0; output_channels],
            have_previous: false,
            source_frames_seen: 0,
            output_frames_written: 0,
            done: false,
        })
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

    /// True once [`Self::finish`] has flushed the final frame.
    pub fn is_finished(&self) -> bool {
        self.done
    }

    fn output_position(&self) -> f64 {
        self.output_frames_written as f64 * self.step
    }

    fn copy_source_frame(&mut self, input: &[f32], source: usize) {
        for channel in 0..self.source_channels {
            self.previous[channel] = input.get(source + channel).copied().unwrap_or(0.0);
        }
    }

    fn map_frame(&mut self, input: &[f32], source: usize, fraction: f64) {
        // Match the reference exactly: the arithmetic runs in f64 (JS
        // `Number`) and the result rounds once when stored into the f32
        // output frame.
        for channel in 0..self.output_channels {
            let source_channel = channel.min(self.source_channels - 1);
            let left = self.previous.get(source_channel).copied().unwrap_or(0.0) as f64;
            let right = input.get(source + source_channel).copied().unwrap_or(0.0) as f64;
            self.output_frame[channel] = (left + (right - left) * fraction) as f32;
        }
    }

    fn map_previous_frame(&mut self) {
        for channel in 0..self.output_channels {
            self.output_frame[channel] = self
                .previous
                .get(channel.min(self.source_channels - 1))
                .copied()
                .unwrap_or(0.0);
        }
    }

    /// Converts complete source frames starting at `frame_offset`. Returns the
    /// first unconsumed source frame.
    pub fn process(
        &mut self,
        input: &[f32],
        frame_offset: usize,
        ring: &mut RingBuffer,
        max_source_frames: usize,
        max_output_frames: usize,
    ) -> usize {
        if self.done {
            return frame_offset;
        }
        let total_frames = input.len() / self.source_channels;
        let mut consumed = 0usize;
        let mut written = 0usize;
        let mut frame_offset = frame_offset;

        while frame_offset < total_frames && consumed < max_source_frames {
            let source = frame_offset * self.source_channels;
            if !self.have_previous {
                if ring.free_frames() == 0 || written >= max_output_frames {
                    break;
                }
                self.copy_source_frame(input, source);
                self.map_frame(input, source, 0.0);
                let frame = self.output_frame.clone();
                if ring.write_interleaved(&frame) != 1 {
                    break;
                }
                self.have_previous = true;
                self.source_frames_seen = 1;
                self.output_frames_written = 1;
                frame_offset += 1;
                consumed += 1;
                written += 1;
                continue;
            }

            let source_index = self.source_frames_seen;
            while self.output_position() <= source_index as f64 + 1e-12 {
                if ring.free_frames() == 0 || written >= max_output_frames {
                    return frame_offset;
                }
                let fraction = self.output_position() - (source_index as f64 - 1.0);
                self.map_frame(input, source, fraction);
                let frame = self.output_frame.clone();
                if ring.write_interleaved(&frame) != 1 {
                    return frame_offset;
                }
                self.output_frames_written += 1;
                written += 1;
            }

            self.copy_source_frame(input, source);
            self.source_frames_seen += 1;
            frame_offset += 1;
            consumed += 1;
        }
        frame_offset
    }

    /// Flushes the final source frame's duration. Returns true when complete.
    pub fn finish(&mut self, ring: &mut RingBuffer, max_output_frames: usize) -> bool {
        if self.done {
            return true;
        }
        if !self.have_previous {
            self.done = true;
            return true;
        }
        let mut written = 0usize;
        let target = self.source_frames_seen as f64 - 1e-12;
        while self.output_position() < target {
            if ring.free_frames() == 0 || written >= max_output_frames {
                return false;
            }
            self.map_previous_frame();
            let frame = self.output_frame.clone();
            if ring.write_interleaved(&frame) != 1 {
                return false;
            }
            self.output_frames_written += 1;
            written += 1;
        }
        self.done = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize, channels: usize, offset: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for sample in 0..frames * channels {
            let frame = (sample / channels) as f32;
            let channel = (sample % channels) as f32;
            out.push(offset + frame + channel * 1000.0);
        }
        out
    }

    fn convert(
        source_rate: u32,
        output_rate: u32,
        chunks: &[Vec<f32>],
        source_channels: usize,
        output_channels: usize,
    ) -> Vec<f32> {
        let mut ring = RingBuffer::new(10_000, output_channels).unwrap();
        let mut resampler =
            StreamingResampler::new(source_rate, source_channels, output_rate, output_channels)
                .unwrap();
        for chunk in chunks {
            assert_eq!(
                resampler.process(chunk, 0, &mut ring, usize::MAX, usize::MAX),
                chunk.len() / source_channels
            );
        }
        assert!(resampler.finish(&mut ring, usize::MAX));
        let mut out = vec![0.0f32; ring.available_frames() as usize * output_channels];
        ring.read_interleaved(&mut out, ring.available_frames() as usize);
        out
    }

    #[test]
    fn upsamples_44100_to_48000_with_correct_duration() {
        let output = convert(44_100, 48_000, &[ramp(441, 1, 0.0)], 1, 1);
        assert_eq!(output.len(), 480);
        for index in [0usize, 1, 160, 320, 478] {
            let want = ((index as f64 * 44_100.0) / 48_000.0).min(440.0) as f32;
            assert!(
                (output[index] - want).abs() < 1e-3,
                "index {index}: {} vs {want}",
                output[index]
            );
        }
        assert_eq!(output[479], 440.0);
    }

    #[test]
    fn downsamples_48000_to_44100_with_correct_duration() {
        let output = convert(48_000, 44_100, &[ramp(480, 1, 0.0)], 1, 1);
        assert_eq!(output.len(), 441);
        for index in [0usize, 1, 147, 294, 440] {
            let want = (index as f64 * 48_000.0) / 44_100.0;
            assert!((output[index] as f64 - want).abs() < 1e-5);
        }
    }

    #[test]
    fn is_identical_across_arbitrary_chunk_boundaries() {
        let source = ramp(441, 1, 0.0);
        let whole = convert(44_100, 48_000, std::slice::from_ref(&source), 1, 1);
        let split = convert(
            44_100,
            48_000,
            &[
                source[0..17].to_vec(),
                source[17..211].to_vec(),
                source[211..212].to_vec(),
                source[212..].to_vec(),
            ],
            1,
            1,
        );
        assert_eq!(split, whole);
    }

    #[test]
    fn resets_state_between_tracks_with_exact_adjacency() {
        let first = convert(10, 20, &[ramp(3, 1, 1.0)], 1, 1);
        assert_eq!(first, [1.0, 1.5, 2.0, 2.5, 3.0, 3.0]);
        let second = convert(20, 20, &[ramp(2, 2, 10.0)], 2, 2);
        assert_eq!(second, [10.0, 1010.0, 11.0, 1011.0]);
    }

    #[test]
    fn never_writes_beyond_the_fixed_ring_capacity() {
        let mut ring = RingBuffer::new(4, 1).unwrap();
        let mut resampler = StreamingResampler::new(10, 1, 20, 1).unwrap();
        let source = ramp(10, 1, 0.0);
        assert!(resampler.process(&source, 0, &mut ring, usize::MAX, usize::MAX) < source.len());
        assert_eq!(ring.available_frames(), 4);
        assert_eq!(ring.free_frames(), 0);
    }

    #[test]
    fn resumes_upsample_after_repeated_saturation() {
        let source = ramp(17, 1, 0.0);
        let expected = convert(10, 20, std::slice::from_ref(&source), 1, 1);
        let mut ring = RingBuffer::new(5, 1).unwrap();
        let mut resampler = StreamingResampler::new(10, 1, 20, 1).unwrap();
        let mut actual: Vec<f32> = Vec::new();
        let mut offset = 0usize;
        let mut finished = false;
        for _ in 0..20 {
            offset = resampler.process(&source, offset, &mut ring, 3, 4);
            if offset == source.len() {
                finished = resampler.finish(&mut ring, 4);
            }
            let n = ring.available_frames() as usize;
            let mut drained = vec![0.0f32; n];
            ring.read_interleaved(&mut drained, n);
            actual.extend_from_slice(&drained);
            if finished {
                break;
            }
        }
        assert!(finished);
        assert_eq!(offset, source.len());
        assert_eq!(actual, expected);
    }
}
