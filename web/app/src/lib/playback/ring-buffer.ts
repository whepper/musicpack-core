// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Bounded interleaved PCM ring buffer shared by the AudioWorklet and the
// decode feed. Pure TS so it runs both in the bundled worklet and in Node
// unit tests. Tracks absolute read/write positions; the worklet consumes
// frames, the controller (via the worklet) fills it.

export class RingBuffer {
  private data: Float32Array;
  private readonly capacityFrames: number;
  private readAbs = 0;
  private writeAbs = 0;
  /** Added to the reported playhead only (never affects where reads and
   *  writes land). The crossfade ring swap uses this to continue the
   *  outgoing ring's absolute frame count without disturbing the promoted
   *  ring's physical layout. */
  private reportedBase = 0;
  readonly channels: number;

  constructor(capacityFrames: number, channels: number) {
    if (capacityFrames <= 0 || channels <= 0) {
      throw new Error('ring buffer needs positive capacity and channels');
    }
    this.capacityFrames = capacityFrames;
    this.channels = channels;
    this.data = new Float32Array(capacityFrames * channels);
  }

  get capacity(): number {
    return this.capacityFrames;
  }

  /** Frames available to read. */
  get availableFrames(): number {
    return this.writeAbs - this.readAbs;
  }

  /** Frames free to write. */
  get freeFrames(): number {
    return this.capacityFrames - this.availableFrames;
  }

  /** Total frames consumed since the last reset, plus the reported base
   *  (see `continuePlayheadFrom`). Monotonic playhead in the album-clock
   *  frame space. */
  get renderedFrames(): number {
    return this.readAbs + this.reportedBase;
  }

  /** Total frames written since the last reset. */
  get writtenFrames(): number {
    return this.writeAbs;
  }

  /** Copies interleaved samples into the ring; returns frames written. */
  writeInterleaved(chunk: Float32Array): number {
    const frames = Math.floor(chunk.length / this.channels);
    const n = Math.min(frames, this.freeFrames);
    if (n <= 0) return 0;
    const ch = this.channels;
    for (let i = 0; i < n; i++) {
      const w = (this.writeAbs + i) % this.capacityFrames;
      const src = i * ch;
      const dst = w * ch;
      for (let c = 0; c < ch; c++) this.data[dst + c] = chunk[src + c] ?? 0;
    }
    this.writeAbs += n;
    return n;
  }

  /** Reads up to maxFrames interleaved frames into out; returns frames read. */
  readInterleaved(out: Float32Array, maxFrames: number): number {
    const n = Math.min(maxFrames, this.availableFrames, Math.floor(out.length / this.channels));
    if (n <= 0) return 0;
    const ch = this.channels;
    for (let i = 0; i < n; i++) {
      const r = (this.readAbs + i) % this.capacityFrames;
      const src = r * ch;
      const dst = i * ch;
      for (let c = 0; c < ch; c++) out[dst + c] = this.data[src + c] ?? 0;
    }
    this.readAbs += n;
    return n;
  }

  /**
   * Reads `frames` frames directly into planar channel outputs (the
   * AudioWorklet layout). Requires `frames <= availableFrames`. When
   * `dstOffset` is set, writes start there instead of at 0.
   */
  readPlanar(outputs: Float32Array[], frames: number, dstOffset = 0): number {
    const n = Math.min(frames, this.availableFrames);
    if (n <= 0) return 0;
    const ch = this.channels;
    for (let i = 0; i < n; i++) {
      const r = (this.readAbs + i) % this.capacityFrames;
      const src = r * ch;
      for (let c = 0; c < ch; c++) {
        const o = outputs[c];
        if (o) o[dstOffset + i] = this.data[src + c] ?? 0;
      }
    }
    this.readAbs += n;
    return n;
  }

  /** Drops everything; playhead accounting restarts at zero. */
  reset(): void {
    this.readAbs = 0;
    this.writeAbs = 0;
    this.reportedBase = 0;
  }

  /** Continues an absolute playhead count: adds `delta` (no-op when <= 0)
   *  to the REPORTED playhead only. Reads, writes and availability are
   *  untouched, so the physical ring layout stays exactly as the decoder
   *  laid it out. Used by the crossfade ring swap: the promoted lane ring
   *  reports frames in the outgoing ring's frame space, keeping the album
   *  clock continuous across the transition. */
  continuePlayheadFrom(delta: number): void {
    if (delta <= 0) return;
    this.reportedBase += delta;
  }
}
