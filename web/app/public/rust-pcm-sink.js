// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
//
// Test-only Rust PCM sink AudioWorklet (Phase 14F).
//
// Deliberately dumb: it consumes interleaved PCM from the bounded SAB ring the
// Rust producer worker fills, writes it to the output channels, reports
// underruns, and mirrors consumed frames into a capture buffer for the test to
// checksum. No decoding, no DSP, no application state. Not part of production.

const CTRL = { WRITE: 0, READ: 1, UNDERRUNS: 2, EOS: 3, PAUSED: 4, IDLE: 5, CAP: 6, MIN_OCC: 7, MAX_OCC: 8 };

class RustPcmSink extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = options.processorOptions;
    this.state = new Int32Array(o.control);
    this.ring = new Float32Array(o.data);
    this.capture = new Float32Array(o.capture);
    this.ch = o.channels;
    this.cap = o.frames;
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const n = out[0].length;
    const occStart = Atomics.load(this.state, CTRL.WRITE) - Atomics.load(this.state, CTRL.READ);
    if (occStart > Atomics.load(this.state, CTRL.MAX_OCC)) {
      Atomics.store(this.state, CTRL.MAX_OCC, occStart);
    }
    let underran = false;
    for (let i = 0; i < n; i++) {
      const r = Atomics.load(this.state, CTRL.READ);
      const w = Atomics.load(this.state, CTRL.WRITE);
      if (r >= w) {
        for (let c = 0; c < this.ch; c++) out[c][i] = 0;
        underran = true;
      } else {
        const idx = (r % this.cap) * this.ch;
        for (let c = 0; c < this.ch; c++) out[c][i] = this.ring[idx + c];
        const ci = r * this.ch;
        if (ci + this.ch <= this.capture.length) {
          for (let c = 0; c < this.ch; c++) this.capture[ci + c] = this.ring[idx + c];
        }
        Atomics.store(this.state, CTRL.READ, r + 1);
      }
    }
    if (underran) {
      Atomics.store(this.state, CTRL.UNDERRUNS, Atomics.load(this.state, CTRL.UNDERRUNS) + 1);
    }
    const occEnd = Atomics.load(this.state, CTRL.WRITE) - Atomics.load(this.state, CTRL.READ);
    if (occEnd < Atomics.load(this.state, CTRL.MIN_OCC)) {
      Atomics.store(this.state, CTRL.MIN_OCC, occEnd);
    }
    Atomics.notify(this.state, CTRL.IDLE);
    return true;
  }
}

registerProcessor('rust-pcm-sink', RustPcmSink);
