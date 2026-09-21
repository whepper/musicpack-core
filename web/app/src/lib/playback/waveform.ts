// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Waveform envelope parsing, downsampling, and height mapping (see
// specs/musicpack-waveform-v1.md).
//
// Binary layout: `peak_u8, rms_u8, peak_u8, rms_u8, ...` (2 bytes/bucket).
// No per-file header; manifest carries interpretation.

export interface WaveformBars {
  peak: Uint8Array;
  rms: Uint8Array;
  points: number;
}

/// Parse a `peak-rms-u8` binary payload into peak/rms typed arrays.
/// Throws when `buf` length is not a multiple of 2.
export function parseWaveform(buf: ArrayBuffer): WaveformBars {
  const view = new Uint8Array(buf);
  if ((view.byteLength & 1) !== 0) {
    throw new Error(`waveform payload not a multiple of 2 bytes: ${view.byteLength}`);
  }
  const points = view.byteLength >> 1;
  const peak = new Uint8Array(points);
  const rms = new Uint8Array(points);
  for (let i = 0; i < points; i++) {
    peak[i] = view[i * 2] ?? 0;
    rms[i] = view[i * 2 + 1] ?? 0;
  }
  return { peak, rms, points };
}

/// Downsample a WaveformBars to `width` output columns preserving visible
/// peaks. For each output pixel:
///
///   * `peakOut` is the maximum `peak` value across the covered input range.
///   * `rmsOut` is the root-mean-square of the input `rms` values across
///     the same range (so a quiet middle doesn't visually flatten the
///     pixel column).
///
/// When `width >= points` we duplicate each input bucket to fill the
/// extra columns (no downsampling artifacts; a 1:1 mapping is preserved
/// when possible, otherwise the first/last columns absorb the slack).
export function downsampleToWidth(bars: WaveformBars, width: number): WaveformBars {
  const { peak, rms, points } = bars;
  if (width <= 0) {
    return { peak: new Uint8Array(0), rms: new Uint8Array(0), points: 0 };
  }
  if (width >= points) {
    const out = new Uint8Array(width);
    const outRms = new Uint8Array(width);
    if (points > 0) {
      for (let i = 0; i < width; i++) {
        const src = Math.min(points - 1, Math.floor((i * points) / width));
        out[i] = peak[src] ?? 0;
        outRms[i] = rms[src] ?? 0;
      }
    }
    return { peak: out, rms: outRms, points: width };
  }
  // Strict peak-preserving min-max-style downsample. Each output column
  // covers a contiguous slice of input buckets; peak = max, rms = sqrt(mean(rms²)).
  const out = new Uint8Array(width);
  const outRms = new Uint8Array(width);
  for (let i = 0; i < width; i++) {
    const start = Math.floor((i * points) / width);
    const end = Math.max(start + 1, Math.floor(((i + 1) * points) / width));
    let p = 0;
    let sumSq = 0;
    let count = 0;
    for (let j = start; j < end && j < points; j++) {
      const v = peak[j] ?? 0;
      if (v > p) p = v;
      const r = rms[j] ?? 0;
      sumSq += r * r;
      count++;
    }
    out[i] = p;
    outRms[i] = count > 0 ? Math.round(Math.sqrt(sumSq / count)) : 0;
  }
  return { peak: out, rms: outRms, points: width };
}

/// Map a u8 amplitude (0..255) to a pixel height (top-down from center).
/// `peak` is drawn full-height; `rms` is drawn as a thinner inner bar.
export function peakToHeight(value: number, height: number): number {
  if (height <= 0) return 0;
  const v = Math.max(0, Math.min(255, value | 0));
  return Math.round((v / 255) * height);
}

/// Cheap LRU cache keyed by track id. Bounded to keep memory bounded.
class WaveformCache {
  private map = new Map<number, WaveformBars>();
  constructor(private cap: number) {}
  get(id: number): WaveformBars | undefined {
    const v = this.map.get(id);
    if (v !== undefined) {
      this.map.delete(id);
      this.map.set(id, v);
    }
    return v;
  }
  set(id: number, bars: WaveformBars): void {
    if (this.map.has(id)) this.map.delete(id);
    this.map.set(id, bars);
    while (this.map.size > this.cap) {
      const first = this.map.keys().next().value;
      if (first === undefined) break;
      this.map.delete(first);
    }
  }
  clear(): void { this.map.clear(); }
}

const CACHE_CAP = 64;
export const waveformCache = new WaveformCache(CACHE_CAP);

/// Fetch and parse a track's waveform from the server. Returns null when
/// the track has no waveform.
export async function fetchWaveform(
  fetchImpl: typeof fetch,
  base: string,
  token: () => string | undefined,
  trackId: number,
): Promise<WaveformBars | null> {
  const cached = waveformCache.get(trackId);
  if (cached !== undefined) return cached;
  const headers: Record<string, string> = {};
  const t = token();
  if (t) headers['Authorization'] = `Bearer ${t}`;
  const res = await fetchImpl(`${base}/api/v1/tracks/${trackId}/waveform`, {
    credentials: 'same-origin',
    headers,
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`waveform fetch failed: ${res.status}`);
  const buf = await res.arrayBuffer();
  const bars = parseWaveform(buf);
  waveformCache.set(trackId, bars);
  return bars;
}
