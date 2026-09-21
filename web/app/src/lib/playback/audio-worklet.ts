// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// MusicPack PCM AudioWorkletProcessor (Phase 6).
//
// Consumes already-decoded interleaved PCM delivered as transferred
// Float32Array messages, buffers it in a bounded ring, and renders it to the
// audio output. Decoding and network never happen here. The ring fill is
// reported through watermarks so the main thread applies backpressure to the
// decoder (pause/resume) and never overflows the buffer.
//
// Bundled as a standalone entry by Vite (imported via `?url`); the ring
// buffer module is inlined at build time.
import { RingBuffer } from './ring-buffer';
import { StreamingResampler } from './streaming-resampler';
import {
  HIGH_WATER,
  LOW_WATER,
  PRIME_FRACTION,
  RING_SECONDS,
  laneCapacityFrames,
  type WorkletReport,
} from './worklet-protocol';

const CALLBACK_REFILL_FRAMES = 512;

interface PendingPcm {
  data: Float32Array;
  frameOffset: number;
}

/** Per-lane state for the crossfade's INCOMING track. */
interface XfadeLane {
  ring: RingBuffer;
  resampler: StreamingResampler;
  pending: PendingPcm | null;
  ending: boolean;
  /** A partially flushed chunk still owes its producer credit; it is
   *  granted as soon as flushing completes (render callbacks drain the
   *  ring while mixing). */
  creditOwed: boolean;
  /** Echoed in go/cancel/ready messages so stale attempts are ignored. */
  token: number;
}

export class MusicPackPcmProcessor extends AudioWorkletProcessor {
  private ring: RingBuffer | null = null;
  private outputRate = 44100;
  private outputChannels = 2;
  private resampler: StreamingResampler | null = null;
  private lastRenderedReport = -1;
  private primed = false;
  private lastFull = false;
  private lastLow = true;
  private underrunning = false;
  private pending: PendingPcm | null = null;
  private ending = false;
  private generation = 0;
  // Crossfade lane (null when no fade is armed).
  private xlane: XfadeLane | null = null;
  private xtoken = 0;
  /** Token of the lane that was promoted at the last swap; late xsamples
   *  with this token belong to the now-current track. */
  private xswappedToken = -1;
  private xmixing = false;
  /** Output frames of overlap already mixed into the outgoing stream. */
  private xmixedFrames = 0;
  private xfadeFrames = 0;
  private xoutgoingAtSwap = 0;
  private xincomingAtSwap = 0;
  /** Outgoing ring's own renderedFrames at the instant mixing began (i.e.
   *  the album-clock position of the boundary itself, BEFORE any overlap).
   *  This is the swap's rebase target: the promoted ring should report as
   *  if it started exactly here, so the reported position advances by the
   *  same amount the caller compresses the outgoing track's declared
   *  length by (`overlapFrames` below) — position and declared duration
   *  can never disagree by construction, instead of both drifting by the
   *  nominal fade window on every single transition. */
  private xoutgoingAtMixStart = 0;
  /** True frames of the outgoing track's tail actually blended with the
   *  incoming lane (bounded by the fade window, and SHORTER than it
   *  whenever the outgoing ring ran dry before the window elapsed). */
  private xoverlapAtSwap = 0;
  /** Interleaved scratch for one outgoing + one incoming frame. */
  private mixScratch = new Float32Array(4);

  constructor() {
    super();
    this.port.onmessage = (ev: MessageEvent) => {
      const msg = ev.data;
      switch (msg.type) {
        case 'config': {
          if (msg.generation < this.generation) break;
          this.generation = msg.generation;
          this.cancelXfade();
          this.outputRate = msg.outputRate;
          this.outputChannels = msg.outputChannels;
          this.ring = new RingBuffer(
            Math.round(this.outputRate * RING_SECONDS),
            this.outputChannels,
          );
          this.resetState(msg.sourceRate, msg.sourceChannels);
          break;
        }
        case 'track': {
          if (!this.ring || msg.generation !== this.generation) break;
          if (this.pending || this.ending) {
            this.reportError('track format changed before the previous track was accepted');
            break;
          }
          this.resampler = new StreamingResampler(
            msg.sourceRate,
            msg.sourceChannels,
            this.outputRate,
            this.outputChannels,
          );
          break;
        }
        case 'reset': {
          if (msg.generation < this.generation) break;
          this.generation = msg.generation;
          this.cancelXfade();
          this.ring?.reset();
          if (this.resampler) {
            this.resetState(this.resampler.sourceRate, this.resampler.sourceChannels);
          } else {
            this.clearFlowState();
          }
          break;
        }
        case 'samples':
        case 'xsamples': {
          // While a crossfade lane exists, xsamples feeds it; a late
          // xsamples AFTER the swap is simply this track's next PCM chunk
          // (the swap absorbed the lane's resampler state) — but ONLY for
          // the token that actually swapped, never a cancelled attempt.
          if (!this.ring || !this.resampler || msg.generation !== this.generation) break;
          const lane =
            msg.type === 'xsamples'
              ? this.xlane ?? (msg.token === this.xswappedToken ? null : undefined)
              : null;
          if (msg.type === 'xsamples' && lane === undefined) break;
          if (lane) {
            if (lane.pending || lane.creditOwed) {
              this.reportError('received crossfade PCM without an available producer credit');
              break;
            }
            lane.pending = { data: new Float32Array(msg.buffer), frameOffset: 0 };
            this.flushXlane();
            this.reportLevel();
            // Grant the credit only when the chunk fully flushed; a partial
            // flush (lane ring nearly full) owes it until mixing drains.
            if (!lane.pending) this.reportAccepted(2);
            else lane.creditOwed = true;
            break;
          }
          if (this.pending) {
            this.reportError('received PCM without an available producer credit');
            break;
          }
          // The absorbed crossfade lane's resampler may already be finished
          // (its xend arrived pre-swap). The engine only keeps pumping this
          // track while the worker is alive, so a finished resampler here
          // means the track's decode continued past the old end: rebuild it
          // transparently instead of silently dropping the chunk.
          if (this.resampler.isFinished) {
            this.resampler = new StreamingResampler(
              this.resampler.sourceRate,
              this.resampler.sourceChannels,
              this.outputRate,
              this.outputChannels,
            );
          }
          const data = new Float32Array(msg.buffer);
          this.pending = { data, frameOffset: 0 };
          const accepted = this.flushPending();
          this.reportLevel();
          if (accepted) this.reportAccepted();
          break;
        }
        case 'end': {
          if (!this.ring || !this.resampler || msg.generation !== this.generation) break;
          if (this.pending) {
            this.reportError('received track end before the PCM credit was accepted');
            break;
          }
          this.ending = true;
          this.flushEnd();
          this.reportLevel();
          break;
        }
        case 'xfade': {
          if (!this.ring || msg.generation !== this.generation || this.xlane) break;
          this.xtoken = msg.token;
          this.xfadeFrames = Math.max(1, Math.floor(msg.fadeFrames));
          this.xlane = {
            // Sized to hold the whole fade window plus margin (see
            // laneCapacityFrames) so long fades prime fully and the mixing
            // loop never waits on real-time decode of the incoming lane.
            ring: new RingBuffer(
              laneCapacityFrames(this.outputRate, this.xfadeFrames),
              this.outputChannels,
            ),
            resampler: new StreamingResampler(
              msg.sourceRate,
              msg.sourceChannels,
              this.outputRate,
              this.outputChannels,
            ),
            pending: null,
            ending: false,
            creditOwed: false,
            token: msg.token,
          };
          break;
        }
        case 'xend': {
          // While a lane exists, xend marks the incoming track's decode
          // end; after a swap it is this track's natural end — but again
          // only for the token that actually swapped.
          const lane = this.xlane;
          if (!lane) {
            if (msg.token !== this.xswappedToken) break;
            if (!this.ring || !this.resampler || msg.generation !== this.generation) break;
            if (this.pending) {
              this.reportError('received track end before the PCM credit was accepted');
              break;
            }
            this.ending = true;
            this.flushEnd();
            this.reportLevel();
            break;
          }
          if (msg.generation !== this.generation) break;
          if (lane.pending) {
            this.reportError('received crossfade end before the PCM credit was accepted');
            break;
          }
          lane.ending = true;
          this.flushXlane();
          this.reportLevel();
          break;
        }
        case 'xfade-go': {
          const lane = this.xlane;
          if (
            !lane ||
            msg.generation !== this.generation ||
            msg.token !== lane.token
          ) {
            break;
          }
          this.xmixing = true;
          // Snapshot the outgoing ring's position right as mixing begins:
          // the swap rebase target, so the boundary lands exactly here
          // regardless of how much of the fade window the outgoing ring
          // can actually still supply.
          this.xoutgoingAtMixStart = this.ring ? this.ring.renderedFrames : 0;
          break;
        }
        case 'xfade-cancel': {
          if (msg.generation !== this.generation) break;
          this.cancelXfade();
          break;
        }
      }
    };
  }

  private flushPending(): boolean {
    const ring = this.ring;
    const chunk = this.pending;
    const resampler = this.resampler;
    if (!ring || !chunk || !resampler) return false;
    chunk.frameOffset = resampler.process(chunk.data, chunk.frameOffset, ring);
    const complete =
      chunk.frameOffset >= Math.floor(chunk.data.length / resampler.sourceChannels);
    if (complete) this.pending = null;
    return complete;
  }

  private flushPendingBounded(): boolean {
    const ring = this.ring;
    const chunk = this.pending;
    const resampler = this.resampler;
    if (!ring || !chunk || !resampler) return false;
    chunk.frameOffset = resampler.process(
      chunk.data,
      chunk.frameOffset,
      ring,
      CALLBACK_REFILL_FRAMES,
      CALLBACK_REFILL_FRAMES,
    );
    const complete =
      chunk.frameOffset >= Math.floor(chunk.data.length / resampler.sourceChannels);
    if (complete) this.pending = null;
    return complete;
  }

  private flushEnd(maxOutputFrames = Number.POSITIVE_INFINITY): void {
    if (!this.ending || !this.ring || !this.resampler) return;
    if (!this.resampler.finish(this.ring, maxOutputFrames)) return;
    this.ending = false;
    if (this.ring.availableFrames > 0 && !this.primed) {
      this.primed = true;
      this.port.postMessage({
        type: 'primed',
        frames: this.ring.renderedFrames,
        generation: this.generation,
      });
    }
    this.port.postMessage({
      type: 'trackEnded',
      frames: this.ring.renderedFrames,
      generation: this.generation,
      available: this.ring.availableFrames,
    });
  }

  private resetState(sourceRate: number, sourceChannels: number): void {
    this.resampler = new StreamingResampler(
      sourceRate,
      sourceChannels,
      this.outputRate,
      this.outputChannels,
    );
    this.clearFlowState();
  }

  /** Resamples as much of the crossfade lane's pending PCM as fits. */
  private flushXlane(): void {
    const lane = this.xlane;
    if (!lane) return;
    if (lane.pending) {
      lane.pending.frameOffset = lane.resampler.process(
        lane.pending.data,
        lane.pending.frameOffset,
        lane.ring,
      );
      const total = Math.floor(lane.pending.data.length / lane.resampler.sourceChannels);
      if (lane.pending.frameOffset >= total) lane.pending = null;
    }
    if (lane.creditOwed && !lane.pending) {
      // The chunk fully drained (mixing freed ring space): grant the credit.
      lane.creditOwed = false;
      this.reportAccepted(2);
    }
    if (lane.ending && !lane.pending && !this.xmixing) {
      // The incoming track is fully queued: tell the engine the fade can go.
      if (!lane.resampler.finish(lane.ring)) return;
      lane.ending = false;
      this.port.postMessage({
        type: 'xfadeReady',
        frames: this.ring?.renderedFrames ?? 0,
        generation: this.generation,
        available: lane.ring.availableFrames,
        token: lane.token,
      });
    }
  }

  private cancelXfade(): void {
    this.xtoken++;
    this.xlane = null;
    this.xmixing = false;
    this.xmixedFrames = 0;
    // xswappedToken survives: late stragglers of a COMPLETED fade stay valid.
  }

  private reportError(message: string): void {
    this.port.postMessage({
      type: 'error',
      frames: this.ring?.renderedFrames ?? 0,
      generation: this.generation,
      message,
    });
  }

  private clearFlowState(): void {
    this.primed = false;
    this.lastFull = false;
    this.lastLow = true;
    this.underrunning = false;
    this.pending = null;
    this.ending = false;
    this.lastRenderedReport = -1;
  }

  private reportAccepted(lane: 1 | 2 = 1): void {
    if (!this.ring) return;
    const source = lane === 2 ? this.xlane?.ring : this.ring;
    if (!source) return;
    this.port.postMessage({
      type: 'accepted',
      frames: this.ring.renderedFrames,
      generation: this.generation,
      available: source.availableFrames,
      lane,
    });
  }

  private reportLevel(): void {
    if (!this.ring) return;
    const fill = this.ring.availableFrames / this.ring.capacity;
    if (fill >= PRIME_FRACTION && !this.primed) {
      this.primed = true;
      this.port.postMessage({
        type: 'primed',
        frames: this.ring.renderedFrames,
        generation: this.generation,
      });
    }
    if (fill >= HIGH_WATER && !this.lastFull) {
      this.lastFull = true;
      this.port.postMessage({
        type: 'full',
        frames: this.ring.renderedFrames,
        generation: this.generation,
        available: this.ring.availableFrames,
      });
    } else if (fill < HIGH_WATER) {
      this.lastFull = false;
    }
    if (fill < LOW_WATER && !this.lastLow) {
      this.lastLow = true;
      this.port.postMessage({
        type: 'need',
        frames: this.ring.renderedFrames,
        generation: this.generation,
        available: this.ring.availableFrames,
      });
    } else if (fill >= LOW_WATER) {
      this.lastLow = false;
    }
  }

  private reportRendered(): void {
    if (!this.ring) return;
    const t = currentTime;
    if (this.lastRenderedReport < 0 || t - this.lastRenderedReport >= 0.2) {
      this.lastRenderedReport = t;
      this.port.postMessage({
        type: 'rendered',
        frames: this.ring.renderedFrames,
        generation: this.generation,
        available: this.ring.availableFrames,
      });
    }
  }

  override process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const output = outputs[0];
    if (!output || output.length === 0) return true;
    const frames = output[0]?.length ?? 0;
    const ring = this.ring;

    if (!ring) {
      for (const ch of output) ch.fill(0);
      return true;
    }

    if (this.xmixing && this.xlane) {
      this.processMixing(output, frames, ring);
      if (!this.xmixing) {
        // Fade completed inside this callback; report the exact accounting.
        const swapped = this.ring;
        if (swapped) {
          this.port.postMessage({
            type: 'xfaded',
            frames: swapped.renderedFrames,
            generation: this.generation,
            available: swapped.availableFrames,
            token: this.xtoken,
            outgoingFrames: this.xoutgoingAtSwap,
            incomingFrames: this.xincomingAtSwap,
            swapBaseFrames: this.xoutgoingAtMixStart,
            overlapFrames: this.xoverlapAtSwap,
          });
        }
        this.cancelXfade();
        this.reportLevel();
      }
      this.reportRendered();
      return true;
    }

    const available = ring.availableFrames;
    if (available < frames) {
      for (const ch of output) ch.fill(0);
      ring.readPlanar(output, frames);
      if (!this.underrunning) {
        this.underrunning = true;
        this.primed = false;
        this.port.postMessage({
          type: 'underrun',
          frames: ring.renderedFrames,
          generation: this.generation,
          available,
        });
      }
    } else {
      ring.readPlanar(output, frames);
      this.underrunning = false;
    }
    const accepted = this.flushPendingBounded();
    if (this.xlane) this.flushXlane();
    this.flushEnd(CALLBACK_REFILL_FRAMES);
    this.reportLevel();
    if (accepted) this.reportAccepted();
    this.reportRendered();
    return true;
  }

  /** Equal-power overlap-add while a fade is in progress. The outgoing ring
   *  drains under a cosine ramp while the incoming lane's head mixes over it
   *  with a sine ramp; when the window elapses the incoming ring becomes THE
   *  ring (counter reset), so downstream accounting simply continues on the
   *  new track. */
  private processMixing(
    output: Float32Array[],
    frames: number,
    outgoing: RingBuffer,
  ): void {
    const lane = this.xlane!;
    const fadeFrames = this.xfadeFrames;
    const ch = this.outputChannels;
    const scratch = this.mixScratch;
    let i = 0;
    while (i < frames) {
      if (this.xmixedFrames >= fadeFrames) {
        this.completeXfadeSwap();
        break; // plain rendering resumes below / next callback
      }
      const t = this.xmixedFrames / fadeFrames;
      const outGain = Math.cos((t * Math.PI) / 2);
      const inGain = Math.sin((t * Math.PI) / 2);
      let hasOut = false;
      let hasIn = false;
      if (outgoing.availableFrames > 0 && outgoing.readInterleaved(scratch, 1) === 1) {
        hasOut = true;
      }
      if (
        lane.ring.availableFrames > 0 &&
        lane.ring.readInterleaved(scratch.subarray(ch), 1) === 1
      ) {
        hasIn = true;
      }
      for (let c = 0; c < ch; c++) {
        const o = output[c];
        if (!o) continue;
        const outS = hasOut ? (scratch[c] ?? 0) : 0;
        const inS = hasIn ? (scratch[ch + c] ?? 0) : 0;
        o[i] = outS * outGain + inS * inGain;
      }
      this.xmixedFrames++;
      i++;
    }
    if (!this.xmixing) return;
    // Fill any rest of this callback straight from the (possibly swapped)
    // active ring; a short ring renders silence, which the normal underrun
    // path recovers from.
    const active = this.ring;
    if (i < frames) {
      const rest = frames - i;
      for (const cch of output) cch.fill(0, i);
      const got = active ? active.readPlanar(output, rest, i) : 0;
      if (got < rest && !this.underrunning) {
        this.underrunning = true;
        this.primed = false;
        this.port.postMessage({
          type: 'underrun',
          frames: active?.renderedFrames ?? 0,
          generation: this.generation,
          available: active?.availableFrames ?? 0,
        });
      } else if (got === rest) {
        this.underrunning = false;
      }
    }
  }

  /** Promotes the crossfade lane to the main decode path at the end of the
   *  fade window: its ring becomes THE ring (counter reset, so all
   *  accounting simply continues on the new track), its resampler and
   *  pending chunk are absorbed, and any straggler `xsamples`/`xend`
   *  messages now fall through the normal `samples`/`end` handlers. */
  private completeXfadeSwap(): void {
    const lane = this.xlane;
    const outgoing = this.ring;
    if (!lane || !outgoing) return;
    this.xoutgoingAtSwap = outgoing.renderedFrames;
    this.xincomingAtSwap = lane.ring.renderedFrames;
    // The true overlap is what the outgoing ring actually supplied during
    // the mix window — bounded by the fade window, and SHORTER whenever
    // the outgoing ring ran dry before the window elapsed (it simply stops
    // advancing once availableFrames hits 0, see processMixing above).
    this.xoverlapAtSwap = Math.max(0, this.xoutgoingAtSwap - this.xoutgoingAtMixStart);
    this.ring = lane.ring;
    // Continue the album clock from the BOUNDARY itself (xoutgoingAtMixStart),
    // not from the outgoing ring's raw swap-time count: the caller (Player)
    // compresses the outgoing track's declared length by the same overlap,
    // so rebasing to the boundary keeps the reported position and the
    // declared offsets model in agreement by construction. Rebasing to the
    // raw swap-time count instead (the previous behavior) credited the
    // outgoing track with its FULL uncompressed length every time, so the
    // reported position silently ran one fade-window ahead of the declared
    // offsets after every single crossfade — tick()'s cursor catch-up would
    // then "resolve" that manufactured gap by force-advancing the queue,
    // visible as skipping through several tracks right after a fade.
    const delta = this.xoutgoingAtMixStart - this.xincomingAtSwap;
    if (delta > 0) this.ring.continuePlayheadFrom(delta);
    this.resampler = lane.resampler;
    this.pending = lane.pending;
    this.ending = lane.ending;
    this.xswappedToken = lane.token;
    this.xlane = null;
    this.xmixing = false;
    this.primed = true;
    // Drain the absorbed pending chunk NOW (unbounded): chunks are small
    // (~1152 source frames) and the promoted ring has plenty of free space,
    // so leaving it half-done would falsely block the next producer credit.
    if (this.pending && this.resampler) {
      const p = this.pending;
      const total = Math.floor(p.data.length / this.resampler.sourceChannels);
      p.frameOffset = this.resampler.process(p.data, p.frameOffset, this.ring);
      if (p.frameOffset >= total) this.pending = null;
    }
  }
}

registerProcessor('musicpack-pcm', MusicPackPcmProcessor);
