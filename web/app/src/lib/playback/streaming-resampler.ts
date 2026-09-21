// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { RingBuffer } from './ring-buffer';

/** Stateful linear sample-rate conversion into a fixed-format output ring. */
export class StreamingResampler {
  private readonly step: number;
  private readonly previous: Float32Array;
  private readonly outputFrame: Float32Array;
  private havePrevious = false;
  private sourceFramesSeen = 0;
  private outputFramesWritten = 0;
  private done = false;

  /** True once finish() has flushed the final frame; further process()
   *  calls are ignored until a fresh resampler replaces this instance. */
  get isFinished(): boolean {
    return this.done;
  }

  constructor(
    readonly sourceRate: number,
    readonly sourceChannels: number,
    readonly outputRate: number,
    readonly outputChannels: number,
  ) {
    if (sourceRate <= 0 || outputRate <= 0 || sourceChannels <= 0 || outputChannels <= 0) {
      throw new Error('resampler rates and channel counts must be positive');
    }
    this.step = sourceRate / outputRate;
    this.previous = new Float32Array(sourceChannels);
    this.outputFrame = new Float32Array(outputChannels);
  }

  /**
   * Converts complete source frames starting at `frameOffset`. The returned
   * offset is the first unconsumed source frame. Limits keep render-callback
   * refill work deterministic; callers outside the callback may omit them.
   */
  process(
    input: Float32Array,
    frameOffset: number,
    ring: RingBuffer,
    maxSourceFrames = Number.POSITIVE_INFINITY,
    maxOutputFrames = Number.POSITIVE_INFINITY,
  ): number {
    if (this.done) return frameOffset;
    const totalFrames = Math.floor(input.length / this.sourceChannels);
    let consumed = 0;
    let written = 0;

    while (frameOffset < totalFrames && consumed < maxSourceFrames) {
      const source = frameOffset * this.sourceChannels;
      if (!this.havePrevious) {
        if (ring.freeFrames === 0 || written >= maxOutputFrames) break;
        this.copySourceFrame(input, source);
        this.mapFrame(input, source, 0);
        if (ring.writeInterleaved(this.outputFrame) !== 1) break;
        this.havePrevious = true;
        this.sourceFramesSeen = 1;
        this.outputFramesWritten = 1;
        frameOffset++;
        consumed++;
        written++;
        continue;
      }

      const sourceIndex = this.sourceFramesSeen;
      while (this.outputPosition <= sourceIndex + 1e-12) {
        if (ring.freeFrames === 0 || written >= maxOutputFrames) return frameOffset;
        const fraction = this.outputPosition - (sourceIndex - 1);
        this.mapFrame(input, source, fraction);
        if (ring.writeInterleaved(this.outputFrame) !== 1) return frameOffset;
        this.outputFramesWritten++;
        written++;
      }

      this.copySourceFrame(input, source);
      this.sourceFramesSeen++;
      frameOffset++;
      consumed++;
    }
    return frameOffset;
  }

  /** Flushes the final source frame's duration. Returns true when complete. */
  finish(ring: RingBuffer, maxOutputFrames = Number.POSITIVE_INFINITY): boolean {
    if (this.done) return true;
    if (!this.havePrevious) {
      this.done = true;
      return true;
    }
    let written = 0;
    while (this.outputPosition < this.sourceFramesSeen - 1e-12) {
      if (ring.freeFrames === 0 || written >= maxOutputFrames) return false;
      this.mapPreviousFrame();
      if (ring.writeInterleaved(this.outputFrame) !== 1) return false;
      this.outputFramesWritten++;
      written++;
    }
    this.done = true;
    return true;
  }

  private copySourceFrame(input: Float32Array, source: number): void {
    for (let channel = 0; channel < this.sourceChannels; channel++) {
      this.previous[channel] = input[source + channel] ?? 0;
    }
  }

  private get outputPosition(): number {
    return this.outputFramesWritten * this.step;
  }

  private mapFrame(input: Float32Array, source: number, fraction: number): void {
    for (let channel = 0; channel < this.outputChannels; channel++) {
      const sourceChannel = Math.min(channel, this.sourceChannels - 1);
      const left = this.previous[sourceChannel] ?? 0;
      const right = input[source + sourceChannel] ?? 0;
      this.outputFrame[channel] = left + (right - left) * fraction;
    }
  }

  private mapPreviousFrame(): void {
    for (let channel = 0; channel < this.outputChannels; channel++) {
      this.outputFrame[channel] = this.previous[Math.min(channel, this.sourceChannels - 1)] ?? 0;
    }
  }
}
