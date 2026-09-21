// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
//
// Production Rust playback PCM sink AudioWorklet (Phase 14H-1).
//
// Deliberately dumb, exactly like the Phase 14F test sink: it consumes
// interleaved Float32 PCM from a bounded SharedArrayBuffer ring the
// `rust-playback.worker.js` producer fills, writes it to its output channels,
// counts underruns/occupancy, and notifies the producer when it frees space.
//
// It must never decode, fetch, seek, own player state, choose representations,
// implement crossfade policy, or become the authoritative playback clock.
// Rust's `rendered_samples()` is the authority; this processor only reports
// the minimal consumption accounting the host needs.
//
// Control words (Int32 over the whole SAB, data region follows):
//   0 WRITE    absolute frames produced (producer writes)
//   1 READ     absolute frames consumed (this processor writes)
//   2 UNDERRUNS quanta that starved
//   3 EOS      1 once output fully drained
//   4 PAUSED   reserved
//   5 IDLE     notify target for capacity waits
//   6 CAP      ring capacity in frames
//   7 MIN_OCC  minimum observed occupancy
//   8 MAX_OCC  maximum observed occupancy
//   9 GEN      generation counter (seek/replacement isolation)

const CTRL = {
  WRITE: 0,
  READ: 1,
  UNDERRUNS: 2,
  EOS: 3,
  PAUSED: 4,
  IDLE: 5,
  CAP: 6,
  MIN_OCC: 7,
  MAX_OCC: 8,
  GEN: 9,
};

class RustPlaybackSink extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = options.processorOptions;
    this.state = new Int32Array(o.control);
    this.ring = new Float32Array(o.data);
    this.ch = o.channels;
    this.cap = o.frames;
    this.gen = Atomics.load(this.state, CTRL.GEN);
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const n = out[0].length;
    // Generation isolation: a seek/replacement bumps GEN and flushes the ring.
    // Drop any stale read cursor so pre-seek PCM can never sound afterwards.
    const gen = Atomics.load(this.state, CTRL.GEN);
    if (gen !== this.gen) {
      this.gen = gen;
      Atomics.store(this.state, CTRL.READ, Atomics.load(this.state, CTRL.WRITE));
      Atomics.store(this.state, CTRL.MIN_OCC, 0x7fffffff);
      Atomics.store(this.state, CTRL.MAX_OCC, 0);
    }
    const occStart =
      Atomics.load(this.state, CTRL.WRITE) - Atomics.load(this.state, CTRL.READ);
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
        Atomics.store(this.state, CTRL.READ, r + 1);
      }
    }
    if (underran) {
      Atomics.add(this.state, CTRL.UNDERRUNS, 1);
    }
    const occEnd =
      Atomics.load(this.state, CTRL.WRITE) - Atomics.load(this.state, CTRL.READ);
    if (occEnd < Atomics.load(this.state, CTRL.MIN_OCC)) {
      Atomics.store(this.state, CTRL.MIN_OCC, occEnd);
    }
    Atomics.notify(this.state, CTRL.IDLE);
    return true;
  }
}

registerProcessor('rust-playback-sink', RustPlaybackSink);
