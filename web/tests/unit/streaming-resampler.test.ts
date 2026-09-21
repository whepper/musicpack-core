// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it } from 'vitest';
import { RingBuffer } from '../../app/src/lib/playback/ring-buffer';
import { StreamingResampler } from '../../app/src/lib/playback/streaming-resampler';

function ramp(frames: number, channels = 1, offset = 0): Float32Array {
  return Float32Array.from({ length: frames * channels }, (_, sample) => {
    const frame = Math.floor(sample / channels);
    const channel = sample % channels;
    return offset + frame + channel * 1000;
  });
}

function convert(
  sourceRate: number,
  outputRate: number,
  chunks: Float32Array[],
  sourceChannels = 1,
  outputChannels = 1,
): Float32Array {
  const ring = new RingBuffer(10000, outputChannels);
  const resampler = new StreamingResampler(
    sourceRate,
    sourceChannels,
    outputRate,
    outputChannels,
  );
  for (const chunk of chunks) {
    expect(resampler.process(chunk, 0, ring)).toBe(chunk.length / sourceChannels);
  }
  expect(resampler.finish(ring)).toBe(true);
  const output = new Float32Array(ring.availableFrames * outputChannels);
  ring.readInterleaved(output, ring.availableFrames);
  return output;
}

describe('StreamingResampler', () => {
  it('converts 44.1 kHz to 48 kHz with the correct duration and source positions', () => {
    const output = convert(44100, 48000, [ramp(441)]);
    expect(output).toHaveLength(480);
    for (const index of [0, 1, 160, 320, 478]) {
      expect(output[index]).toBeCloseTo(Math.min((index * 44100) / 48000, 440), 4);
    }
    expect(output[479]).toBe(440);
  });

  it('converts 48 kHz to 44.1 kHz with the correct duration and source positions', () => {
    const output = convert(48000, 44100, [ramp(480)]);
    expect(output).toHaveLength(441);
    for (const index of [0, 1, 147, 294, 440]) {
      expect(output[index]).toBeCloseTo((index * 48000) / 44100, 5);
    }
  });

  it('is identical across arbitrary source chunk boundaries', () => {
    const source = ramp(441);
    const whole = convert(44100, 48000, [source]);
    const split = convert(44100, 48000, [
      source.slice(0, 17),
      source.slice(17, 211),
      source.slice(211, 212),
      source.slice(212),
    ]);
    expect(split).toEqual(whole);
  });

  it('resets state between consecutive tracks while preserving their exact adjacency', () => {
    const first = convert(10, 20, [ramp(3, 1, 1)]);
    const second = convert(20, 20, [ramp(2, 2, 10)], 2, 2);
    expect(Array.from(first)).toEqual([1, 1.5, 2, 2.5, 3, 3]);
    expect(Array.from(second)).toEqual([10, 1010, 11, 1011]);
  });

  it('never writes beyond the fixed ring capacity', () => {
    const ring = new RingBuffer(4, 1);
    const resampler = new StreamingResampler(10, 1, 20, 1);
    const source = ramp(10);
    expect(resampler.process(source, 0, ring)).toBeLessThan(source.length);
    expect(ring.availableFrames).toBe(4);
    expect(ring.freeFrames).toBe(0);
  });

  it('resumes an upsample after repeated ring saturation without loss or duplication', () => {
    const source = ramp(17);
    const expected = convert(10, 20, [source]);
    const ring = new RingBuffer(5, 1);
    const resampler = new StreamingResampler(10, 1, 20, 1);
    const actual: number[] = [];
    let offset = 0;
    let finished = false;

    for (let iteration = 0; iteration < 20 && !finished; iteration++) {
      offset = resampler.process(source, offset, ring, 3, 4);
      if (offset === source.length) finished = resampler.finish(ring, 4);
      const drained = new Float32Array(ring.availableFrames);
      ring.readInterleaved(drained, drained.length);
      actual.push(...drained);
    }

    expect(finished).toBe(true);
    expect(offset).toBe(source.length);
    expect(Float32Array.from(actual)).toEqual(expected);
  });
});
