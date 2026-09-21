// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
//
// Test-only Rust PCM producer worker (Phase 14F).
//
// Reuses the Phase 14E range source (real networker.js + SAB mailbox), then
// renders PCM from the Rust engine into a bounded SharedArrayBuffer ring that
// the `rust-pcm-sink` AudioWorklet consumes. Backpressure is explicit: the
// worker renders only while the ring has free frames and yields via
// `Atomics.wait` otherwise. Not part of production.
const CTRL = { WRITE: 0, READ: 1, UNDERRUNS: 2, EOS: 3, PAUSED: 4, IDLE: 5, CAP: 6 };
const BLOCK = 1024;

self.onmessage = async (e) => {
  const d = e.data;
  const post = (m) => self.postMessage(m);
  try {
    const b64ToBytes = (s) => {
      const bin = atob(s);
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out;
    };
    const wasm_bindgen = new Function(d.glue + '\n;return wasm_bindgen;')();
    wasm_bindgen.initSync({ module: new WebAssembly.Module(b64ToBytes(d.wasmB64)) });
    importScripts(d.origin + '/reader_mailbox.js');
    const M = self.MusicPackMailbox;
    const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);
    const netState = new Int32Array(sab);
    const netData = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
    const net = new Worker(d.origin + '/networker.js');
    const ready = new Promise((res, rej) => {
      net.onmessage = (ev) => {
        if (ev.data && ev.data.type === 'ready') res();
      };
      net.onerror = (err) => rej(new Error('networker: ' + (err.message || 'error')));
    });
    net.postMessage({ type: 'open', sab, url: d.url, size: d.size, token: d.token });
    await ready;

    const read = (_u, offset, len) => {
      const want = Math.min(len, M.DATA_CAP, d.size - offset);
      if (want <= 0) return new Uint8Array(0);
      Atomics.store(netState, M.POS_LO, offset >>> 0);
      Atomics.store(netState, M.POS_HI, Math.floor(offset / 4294967296));
      Atomics.store(netState, M.LEN, want);
      Atomics.store(netState, M.RES, 0);
      Atomics.store(netState, M.REQ, 1);
      Atomics.notify(netState, M.REQ);
      while (Atomics.load(netState, M.RES) === 0) Atomics.wait(netState, M.RES, 0);
      if (Atomics.load(netState, M.ERROR) !== 0) {
        const err = Atomics.load(netState, M.ERROR);
        Atomics.store(netState, M.RES, 0);
        throw new Error('range error ' + err);
      }
      const n = Atomics.load(netState, M.DONE_LEN);
      const out = new Uint8Array(n);
      out.set(netData.subarray(0, n));
      Atomics.store(netState, M.RES, 0);
      return out;
    };

    const state = new Int32Array(d.control);
    const ring = new Float32Array(d.data);
    const cap = d.frames;
    const ch = d.channels;
    const engine = wasm_bindgen.WasmEngine.newRangeSource(44100, ch, read);
    const item = JSON.stringify({ id: 't1', trackId: 1, url: d.url, codec: d.codec || 'musepack-sv8' });
    const info = JSON.parse(engine.open(item));
    engine.start();
    engine.play();
    post({ type: 'info', info, servedAtOpen: Atomics.load(netState, M.SERVED) });

    for (;;) {
      if (Atomics.load(state, CTRL.PAUSED) === 1) {
        Atomics.wait(state, CTRL.IDLE, 0, 10);
        continue;
      }
      if (engine.is_output_drained()) {
        if (Atomics.load(state, CTRL.READ) >= Atomics.load(state, CTRL.WRITE)) {
          Atomics.store(state, CTRL.EOS, 1);
          Atomics.notify(state, CTRL.EOS);
          break;
        }
        Atomics.wait(state, CTRL.IDLE, 0, 10);
        continue;
      }
      const w = Atomics.load(state, CTRL.WRITE);
      const r = Atomics.load(state, CTRL.READ);
      if (cap - (w - r) >= BLOCK) {
        const block = engine.render(BLOCK);
        let idx = (w % cap) * ch;
        for (let i = 0; i < block.length; i++) {
          ring[idx] = block[i];
          idx += 1;
          if (idx === cap * ch) idx = 0;
        }
        Atomics.store(state, CTRL.WRITE, w + BLOCK);
        Atomics.notify(state, CTRL.IDLE);
      } else {
        Atomics.wait(state, CTRL.IDLE, 0, 10);
      }
    }
    post({
      type: 'done',
      rendered: engine.rendered_samples(),
      served: Atomics.load(netState, M.SERVED),
      error: engine.take_error(),
    });
    net.terminate();
    engine.close();
  } catch (err) {
    post({ type: 'error', message: String(err) });
  }
};
