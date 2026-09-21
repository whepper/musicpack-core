// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
//
// Test-only Rust/WASM decoder probe worker (Phase 14E).
//
// Not part of the production playback path: it is never instantiated by the
// application. A Playwright test spawns it directly to verify that the Rust
// `musicpack-wasm` decoder can consume the existing demand-range source through
// the real `networker.js`/`reader_mailbox.js` SAB mailbox. The Rust glue and
// module bytes are injected via `postMessage` (no bundled wasm).
self.onmessage = async (e) => {
  const d = e.data;
  const report = { ok: false, message: '' };
  const progress = (p) => self.postMessage({ progress: p });
  try {
    const b64ToBytes = (s) => {
      const bin = atob(s);
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out;
    };
    const wasm_bindgen = new Function(d.glue + '\n;return wasm_bindgen;')();
    wasm_bindgen.initSync({ module: new WebAssembly.Module(b64ToBytes(d.wasmB64)) });
    progress('wasm');
    importScripts(d.origin + '/reader_mailbox.js');
    const M = self.MusicPackMailbox;
    const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);
    const state = new Int32Array(sab);
    const data = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);
    const net = new Worker(d.origin + '/networker.js');
    const networkerReady = new Promise((res, rej) => {
      net.onmessage = (ev) => {
        if (ev.data && ev.data.type === 'ready') res();
      };
      net.onerror = (err) => rej(new Error('networker: ' + (err.message || 'error')));
    });
    net.postMessage({ type: 'open', sab, url: d.url, size: d.size, token: d.token });
    await networkerReady;
    progress('networker');
    let calls = 0;
    const read = (_u, offset, len) => {
      calls += 1;
      const want = Math.min(len, M.DATA_CAP, d.size - offset);
      if (want <= 0) return new Uint8Array(0);
      Atomics.store(state, M.POS_LO, offset >>> 0);
      Atomics.store(state, M.POS_HI, Math.floor(offset / 4294967296));
      Atomics.store(state, M.LEN, want);
      Atomics.store(state, M.RES, 0);
      Atomics.store(state, M.REQ, 1);
      Atomics.notify(state, M.REQ);
      while (Atomics.load(state, M.RES) === 0) Atomics.wait(state, M.RES, 0);
      if (Atomics.load(state, M.ERROR) !== 0) {
        const err = Atomics.load(state, M.ERROR);
        Atomics.store(state, M.RES, 0);
        throw new Error('range error ' + err);
      }
      const n = Atomics.load(state, M.DONE_LEN);
      const out = new Uint8Array(n);
      out.set(data.subarray(0, n));
      Atomics.store(state, M.RES, 0);
      return out;
    };
    const served = () => Atomics.load(state, M.SERVED);
    const engine = wasm_bindgen.WasmEngine.newRangeSource(44100, 2, read);
    const item = JSON.stringify({ id: 't1', trackId: 1, url: d.url, codec: 'musepack-sv8' });
    const info = JSON.parse(engine.open(item));
    const servedAtOpen = served();
    engine.start();
    engine.play();
    const renderUntil = (frames) => {
      for (let i = 0; i < 1000000; i++) {
        engine.render(1024);
        if (engine.rendered_samples() >= frames) break;
      }
    };
    renderUntil(44100);
    engine.seek(Math.floor(info.lengthSamples * 0.1));
    renderUntil(2048);
    const servedAfterSeek = served();
    engine.seek(0);
    const want = info.lengthSamples;
    const pcm = new Float32Array(want * 2);
    let produced = 0;
    for (let i = 0; i < 1000000 && produced < want; i++) {
      const block = engine.render(1024);
      const take = Math.min(block.length / 2, want - produced);
      pcm.set(block.subarray(0, take * 2), produced * 2);
      produced += take;
    }
    progress('decoded');
    const digest = await crypto.subtle.digest('SHA-256', new Uint8Array(pcm.buffer));
    const pcmSha = Array.from(new Uint8Array(digest))
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
    const servedAfter = served();
    const error = engine.take_error();
    engine.close();
    net.terminate();
    Object.assign(report, {
      ok: true,
      info,
      servedAtOpen,
      servedAfterSeek,
      servedAfter,
      calls,
      pcmSha,
      error,
      size: d.size,
    });
  } catch (err) {
    report.message = String(err);
  }
  self.postMessage(report);
};
