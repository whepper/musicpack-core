//! Bounded interleaved PCM ring buffer.
//!
//! Port of `web/app/src/lib/playback/ring-buffer.ts` (BSD-3-Clause). The
//! worklet consumes frames while the decode pump fills them; absolute
//! read/write playheads make the accounting exact and the buffer is fixed
//! capacity, so it can never grow without bound.
//!
//! `reported_base` adds to the *reported* playhead only — never to where reads
//! or writes land. The crossfade swap uses it to continue the outgoing ring's
//! absolute frame count without disturbing the promoted ring's physical
//! layout.

/// A bounded interleaved `f32` ring buffer.
pub struct RingBuffer {
    data: Vec<f32>,
    capacity_frames: usize,
    read_abs: u64,
    write_abs: u64,
    reported_base: u64,
    channels: usize,
}

impl RingBuffer {
    /// Creates a ring, or `None` when capacity/channels are not positive.
    pub fn new(capacity_frames: usize, channels: usize) -> Option<Self> {
        if capacity_frames == 0 || channels == 0 {
            return None;
        }
        Some(Self {
            data: vec![0.0; capacity_frames * channels],
            capacity_frames,
            read_abs: 0,
            write_abs: 0,
            reported_base: 0,
            channels,
        })
    }

    /// Capacity in frames.
    pub fn capacity(&self) -> usize {
        self.capacity_frames
    }

    /// Channel count.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames available to read.
    pub fn available_frames(&self) -> u64 {
        self.write_abs - self.read_abs
    }

    /// Frames free to write.
    pub fn free_frames(&self) -> u64 {
        self.capacity_frames as u64 - self.available_frames()
    }

    /// Frames consumed since the last reset, plus the reported base.
    pub fn rendered_frames(&self) -> u64 {
        self.read_abs + self.reported_base
    }

    /// Frames written since the last reset.
    pub fn written_frames(&self) -> u64 {
        self.write_abs
    }

    /// Copies interleaved samples into the ring; returns frames written.
    pub fn write_interleaved(&mut self, chunk: &[f32]) -> usize {
        let channels = self.channels;
        let frames = chunk.len() / channels;
        let n = (frames as u64).min(self.free_frames()) as usize;
        if n == 0 {
            return 0;
        }
        for i in 0..n {
            let w = (self.write_abs as usize + i) % self.capacity_frames;
            let src = i * channels;
            let dst = w * channels;
            self.data[dst..dst + channels].copy_from_slice(&chunk[src..src + channels]);
        }
        self.write_abs += n as u64;
        n
    }

    /// Reads up to `max_frames` interleaved frames into `out`.
    pub fn read_interleaved(&mut self, out: &mut [f32], max_frames: usize) -> usize {
        let channels = self.channels;
        let n = max_frames
            .min(self.available_frames() as usize)
            .min(out.len() / channels);
        if n == 0 {
            return 0;
        }
        for i in 0..n {
            let r = (self.read_abs as usize + i) % self.capacity_frames;
            let src = r * channels;
            let dst = i * channels;
            out[dst..dst + channels].copy_from_slice(&self.data[src..src + channels]);
        }
        self.read_abs += n as u64;
        n
    }

    /// Reads `frames` frames into planar channel slices (worklet layout),
    /// starting at `dst_offset` in each output. Returns frames read.
    pub fn read_planar(
        &mut self,
        outputs: &mut [&mut [f32]],
        frames: usize,
        dst_offset: usize,
    ) -> usize {
        let channels = self.channels;
        let n = frames.min(self.available_frames() as usize);
        if n == 0 {
            return 0;
        }
        for i in 0..n {
            let r = (self.read_abs as usize + i) % self.capacity_frames;
            let src = r * channels;
            for c in 0..channels {
                if let Some(o) = outputs.get_mut(c) {
                    if dst_offset + i < o.len() {
                        o[dst_offset + i] = self.data[src + c];
                    }
                }
            }
        }
        self.read_abs += n as u64;
        n
    }

    /// Drops everything; playhead accounting restarts at zero.
    pub fn reset(&mut self) {
        self.read_abs = 0;
        self.write_abs = 0;
        self.reported_base = 0;
    }

    /// Adds `delta` to the *reported* playhead only (no-op when zero).
    pub fn continue_playhead_from(&mut self, delta: u64) {
        if delta == 0 {
            return;
        }
        self.reported_base = self.reported_base.saturating_add(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interleaved(channels: usize, frames: usize, start: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            for c in 0..channels {
                out.push(start + i as f32 + c as f32 / 10.0);
            }
        }
        out
    }

    #[test]
    fn writes_and_reads_frames_in_order() {
        let mut ring = RingBuffer::new(8, 2).unwrap();
        let chunk = interleaved(2, 4, 1.0);
        assert_eq!(ring.write_interleaved(&chunk), 4);
        assert_eq!(ring.available_frames(), 4);
        let mut out = vec![0.0f32; 8];
        assert_eq!(ring.read_interleaved(&mut out, 4), 4);
        assert_eq!(out, chunk);
        assert_eq!(ring.available_frames(), 0);
        assert_eq!(ring.rendered_frames(), 4);
    }

    #[test]
    fn never_overflows_excess_writes_are_dropped() {
        let mut ring = RingBuffer::new(4, 1).unwrap();
        ring.write_interleaved(&interleaved(1, 3, 0.0));
        assert_eq!(ring.write_interleaved(&interleaved(1, 4, 10.0)), 1);
        assert_eq!(ring.available_frames(), 4);
    }

    #[test]
    fn wraps_around_the_ring() {
        let mut ring = RingBuffer::new(4, 1).unwrap();
        ring.write_interleaved(&interleaved(1, 3, 0.0));
        let mut out = vec![0.0f32; 3];
        ring.read_interleaved(&mut out, 3);
        ring.write_interleaved(&interleaved(1, 3, 100.0));
        assert_eq!(ring.available_frames(), 3);
        let mut out2 = vec![0.0f32; 3];
        ring.read_interleaved(&mut out2, 3);
        assert_eq!(out2, [100.0, 101.0, 102.0]);
    }

    #[test]
    fn read_planar_fills_channel_outputs() {
        let mut ring = RingBuffer::new(8, 2).unwrap();
        ring.write_interleaved(&interleaved(2, 3, 1.0));
        let mut l = vec![0.0f32; 3];
        let mut r = vec![0.0f32; 3];
        {
            let mut outs: [&mut [f32]; 2] = [&mut l, &mut r];
            assert_eq!(ring.read_planar(&mut outs, 3, 0), 3);
        }
        assert_eq!(l, [1.0, 2.0, 3.0]);
        for (v, want) in r.iter().zip([1.1f32, 2.1, 3.1]) {
            assert!((v - want).abs() < 1e-6);
        }
    }

    #[test]
    fn reset_clears_buffer_and_playhead() {
        let mut ring = RingBuffer::new(4, 1).unwrap();
        ring.write_interleaved(&interleaved(1, 2, 0.0));
        ring.read_interleaved(&mut [0.0f32; 2], 2);
        assert_eq!(ring.rendered_frames(), 2);
        ring.reset();
        assert_eq!(ring.available_frames(), 0);
        assert_eq!(ring.rendered_frames(), 0);
    }

    #[test]
    fn continue_playhead_from_rebases_reported_playhead() {
        let mut ring = RingBuffer::new(8, 1).unwrap();
        ring.write_interleaved(&interleaved(1, 4, 0.0));
        ring.read_interleaved(&mut [0.0f32; 3], 3);
        assert_eq!(ring.rendered_frames(), 3);
        assert_eq!(ring.available_frames(), 1);
        ring.continue_playhead_from(100);
        assert_eq!(ring.rendered_frames(), 103);
        assert_eq!(ring.available_frames(), 1);
        let mut out = vec![0.0f32; 1];
        assert_eq!(ring.read_interleaved(&mut out, 1), 1);
        assert!((out[0] - 3.0).abs() < 1e-6);
        assert_eq!(ring.rendered_frames(), 104);
    }

    #[test]
    fn continue_playhead_from_ignores_zero() {
        let mut ring = RingBuffer::new(4, 1).unwrap();
        ring.write_interleaved(&interleaved(1, 2, 0.0));
        ring.continue_playhead_from(0);
        assert_eq!(ring.rendered_frames(), 0);
        assert_eq!(ring.available_frames(), 2);
    }

    #[test]
    fn requires_positive_capacity_and_channels() {
        assert!(RingBuffer::new(0, 2).is_none());
        assert!(RingBuffer::new(8, 0).is_none());
    }
}
