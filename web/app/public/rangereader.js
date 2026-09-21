// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * Decoder-side demand reader for the Phase 5 browser demo.
 *
 * Installs synchronous Module.mpcRangeRead/Seek/Tell (the wasm imports behind
 * mpc_wasm_open_range) that coordinate with a network Worker over a
 * SharedArrayBuffer mailbox using the two-flag REQ/RES handshake (see
 * reader_mailbox.js). The decoder blocks only until the network worker
 * serves the requested bytes from its block cache (or one HTTP Range fetch),
 * so playback starts without the full file and seeking fetches only the
 * ranges the decoder needs.
 *
 * Requires cross-origin isolation (COOP/COEP), which the Phase 5 server's
 * --static-dir provides for the reference demo.
 */
(function (global) {
  'use strict';

  async function installDemandReader(Module, url, totalSize, token) {
    const M = global.MusicPackMailbox;
    const HEAPU8 = Module.HEAPU8;

    const sab = new SharedArrayBuffer(M.DATA_OFFSET + M.DATA_CAP);
    const state = new Int32Array(sab);
    const data = new Uint8Array(sab, M.DATA_OFFSET, M.DATA_CAP);

    const networker = new Worker('networker.js');
    const ready = new Promise((resolve) => {
      networker.onmessage = (ev) => {
        if (ev.data && ev.data.type === 'ready') resolve();
      };
    });
    networker.postMessage({ type: 'open', sab, url, size: totalSize, token });
    await ready;

    let readerPos = 0;
    let lastError = 0;

    Module.mpcRangeRead = function (ptr, size) {
      const pos = readerPos;
      const want = Math.min(size, M.DATA_CAP, totalSize - pos);
      if (want <= 0) return 0;
      Atomics.store(state, M.POS_LO, pos >>> 0);
      Atomics.store(state, M.POS_HI, Math.floor(pos / 4294967296));
      Atomics.store(state, M.LEN, want);
      Atomics.store(state, M.RES, 0);          /* ensure a clean response */
      Atomics.store(state, M.REQ, 1);          /* request */
      Atomics.notify(state, M.REQ);
      while (Atomics.load(state, M.RES) === 0)
        Atomics.wait(state, M.RES, 0);         /* wait for the response */
      if (Atomics.load(state, M.ERROR) !== 0) {
        lastError = Atomics.load(state, M.ERROR);
        Atomics.store(state, M.RES, 0);
        return 0;
      }
      const n = Atomics.load(state, M.DONE_LEN);
      HEAPU8.set(data.subarray(0, n), ptr);
      readerPos += n;
      Atomics.store(state, M.RES, 0);          /* consume the response */
      return n;
    };
    Module.mpcRangeSeek = function (offset) {
      readerPos = offset;
      return 1;
    };
    Module.mpcRangeTell = function () {
      return readerPos;
    };

    return {
      pos: () => readerPos,
      served: () => Atomics.load(state, M.SERVED),
      lastError: () => lastError,
      close: () => {
        networker.terminate();
        Module.mpcRangeRead = Module.mpcRangeSeek = Module.mpcRangeTell = null;
      },
    };
  }

  global.MusicPackRange = { installDemandReader };
})(typeof self !== 'undefined' ? self : this);
