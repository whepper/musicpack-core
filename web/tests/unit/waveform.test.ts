// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Tests for `parseWaveform` + `downsampleToWidth` (lib/playback/waveform.ts).

import { describe, it, expect } from 'vitest';
import { parseWaveform, downsampleToWidth } from '../../app/src/lib/playback/waveform';

function buildPayload(peaks: number[], rmss: number[]): ArrayBuffer {
  if (peaks.length !== rmss.length) throw new Error('length mismatch');
  const buf = new Uint8Array(peaks.length * 2);
  for (let i = 0; i < peaks.length; i++) {
    buf[i * 2] = (peaks[i] ?? 0) & 0xff;
    buf[i * 2 + 1] = (rmss[i] ?? 0) & 0xff;
  }
  return buf.buffer;
}

describe('parseWaveform', () => {
  it('round-trips a peak/rms sequence', () => {
    const buf = buildPayload([0, 1, 200, 255], [0, 50, 100, 128]);
    const bars = parseWaveform(buf);
    expect(bars.points).toBe(4);
    expect(Array.from(bars.peak)).toEqual([0, 1, 200, 255]);
    expect(Array.from(bars.rms)).toEqual([0, 50, 100, 128]);
  });

  it('rejects odd-byte payloads', () => {
    const buf = new Uint8Array([0, 1, 2]).buffer;
    expect(() => parseWaveform(buf)).toThrow(/multiple of 2/);
  });

  it('handles empty payloads', () => {
    const bars = parseWaveform(new ArrayBuffer(0));
    expect(bars.points).toBe(0);
    expect(bars.peak.length).toBe(0);
    expect(bars.rms.length).toBe(0);
  });
});

describe('downsampleToWidth', () => {
  it('preserves a single peak in a long input', () => {
    // 100 buckets with a single peak-255 at index 50, downsampled to 20.
    const peaks = new Array(100).fill(0);
    const rmss = new Array(100).fill(0);
    peaks[50] = 255;
    rmss[50] = 128;
    const out = downsampleToWidth({ peak: new Uint8Array(peaks), rms: new Uint8Array(rmss), points: 100 }, 20);
    expect(out.points).toBe(20);
    // The output column covering bucket 50 must still carry the spike
    // (peak is preserved through max-pool; rms is sqrt-of-mean so the
    // bucket that contains the spike has at least one non-zero column).
    expect(out.peak.some((p) => p === 255)).toBe(true);
    expect(out.rms.some((r) => r > 0)).toBe(true);
  });

  it('preserves 1:1 when width == points', () => {
    const peaks = [0, 1, 2, 3, 4];
    const rmss = [10, 20, 30, 40, 50];
    const out = downsampleToWidth({
      peak: new Uint8Array(peaks),
      rms: new Uint8Array(rmss),
      points: 5,
    }, 5);
    expect(Array.from(out.peak)).toEqual(peaks);
    expect(Array.from(out.rms)).toEqual(rmss);
  });

  it('clamps to 1:1 when width > points (covers input via floor mapping)', () => {
    const peaks = [10, 20, 30];
    const rmss = [5, 6, 7];
    const out = downsampleToWidth({
      peak: new Uint8Array(peaks),
      rms: new Uint8Array(rmss),
      points: 3,
    }, 10);
    expect(out.points).toBe(10);
    // Each output column maps back to one of the 3 input buckets (no zeros
    // unless the source was zero).
    for (const v of out.peak) expect([10, 20, 30]).toContain(v);
  });

  it('handles empty input', () => {
    const out = downsampleToWidth({
      peak: new Uint8Array(0),
      rms: new Uint8Array(0),
      points: 0,
    }, 100);
    expect(out.points).toBe(100);
    expect(out.peak.every((p) => p === 0)).toBe(true);
  });
});
