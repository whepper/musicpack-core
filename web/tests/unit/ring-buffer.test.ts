import { describe, it, expect } from 'vitest';
import { RingBuffer } from '../../app/src/lib/playback/ring-buffer';

function interleaved(channels: number, frames: number, start = 0): Float32Array {
  const out = new Float32Array(frames * channels);
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) out[i * channels + c] = start + i + c / 10;
  }
  return out;
}

describe('RingBuffer', () => {
  it('writes and reads frames in order', () => {
    const ring = new RingBuffer(8, 2);
    const chunk = interleaved(2, 4, 1);
    expect(ring.writeInterleaved(chunk)).toBe(4);
    expect(ring.availableFrames).toBe(4);
    const out = new Float32Array(8);
    expect(ring.readInterleaved(out, 4)).toBe(4);
    expect(Array.from(out)).toEqual(Array.from(chunk));
    expect(ring.availableFrames).toBe(0);
    expect(ring.renderedFrames).toBe(4);
  });

  it('never overflows: excess writes are dropped (bounded)', () => {
    const ring = new RingBuffer(4, 1);
    ring.writeInterleaved(interleaved(1, 3, 0));
    expect(ring.writeInterleaved(interleaved(1, 4, 10))).toBe(1); // only 1 free
    expect(ring.availableFrames).toBe(4);
  });

  it('wraps around the ring correctly', () => {
    const ring = new RingBuffer(4, 1);
    ring.writeInterleaved(interleaved(1, 3, 0));
    const out = new Float32Array(3);
    ring.readInterleaved(out, 3);
    ring.writeInterleaved(interleaved(1, 3, 100)); // wraps the write head
    expect(ring.availableFrames).toBe(3);
    const out2 = new Float32Array(3);
    ring.readInterleaved(out2, 3);
    expect(Array.from(out2)).toEqual([100, 101, 102]);
  });

  it('readPlanar fills channel outputs (AudioWorklet layout)', () => {
    const ring = new RingBuffer(8, 2);
    ring.writeInterleaved(interleaved(2, 3, 1));
    const l = new Float32Array(3);
    const r = new Float32Array(3);
    expect(ring.readPlanar([l, r], 3)).toBe(3);
    expect(Array.from(l)).toEqual([1, 2, 3]);
    expect(Array.from(r).map((v) => Math.round(v * 10))).toEqual([1, 2, 3].map((v) => v * 10 + 1));
  });

  it('reset clears the buffer and playhead accounting', () => {
    const ring = new RingBuffer(4, 1);
    ring.writeInterleaved(interleaved(1, 2, 0));
    ring.readInterleaved(new Float32Array(2), 2);
    expect(ring.renderedFrames).toBe(2);
    ring.reset();
    expect(ring.availableFrames).toBe(0);
    expect(ring.renderedFrames).toBe(0);
  });

  it('continuePlayheadFrom rebases the reported playhead without touching data', () => {
    const ring = new RingBuffer(8, 1);
    ring.writeInterleaved(interleaved(1, 4));
    ring.readInterleaved(new Float32Array(3), 3);
    expect(ring.renderedFrames).toBe(3);
    expect(ring.availableFrames).toBe(1);
    ring.continuePlayheadFrom(100);
    // Reported playhead continues at the absolute position; reads and
    // availability are untouched, so the buffered frame is still buffered
    // and reads the same data.
    expect(ring.renderedFrames).toBe(103);
    expect(ring.availableFrames).toBe(1);
    const out = new Float32Array(1);
    expect(ring.readInterleaved(out, 1)).toBe(1);
    expect(out[0]).toBeCloseTo(3, 5);
    expect(ring.renderedFrames).toBe(104);
  });

  it('continuePlayheadFrom ignores non-positive deltas', () => {
    const ring = new RingBuffer(4, 1);
    ring.writeInterleaved(interleaved(1, 2));
    ring.continuePlayheadFrom(-5);
    ring.continuePlayheadFrom(0);
    expect(ring.renderedFrames).toBe(0);
    expect(ring.availableFrames).toBe(2);
  });

  it('requires positive capacity and channels', () => {
    expect(() => new RingBuffer(0, 2)).toThrow();
    expect(() => new RingBuffer(8, 0)).toThrow();
  });
});
