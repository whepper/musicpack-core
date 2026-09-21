// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * Decoder-side local reader for offline Musepack playback.
 *
 * Structural mirror of rangereader.js (the HTTP demand reader): installs
 * synchronous Module.mpcRangeRead/Seek/Tell imports that serve 64 KiB
 * blocks from a locally installed OPFS file instead of the network. The
 * wasm decoder is byte-source agnostic — it sees exactly the same read
 * contract, so open/seek/eos semantics are identical online and offline.
 *
 * The file handle is opened with createSyncAccessHandle, which is legal in
 * dedicated workers (this script runs inside decoder.worker.js). Reads copy
 * straight from the handle into the mailbox region — no fetch, no SAB.
 *
 * Requires cross-origin isolation only because the surrounding worker page
 * already uses SharedArrayBuffer for the online path; this reader adds none.
 */
(function (global) {
  'use strict';

  const READ_CHUNK = 64 * 1024;

  async function installLocalReader(Module, key, totalSize) {
    const HEAPU8 = Module.HEAPU8;
    const root = await navigator.storage.getDirectory();
    const base = await root.getDirectoryHandle('musicpack-offline-v1');
    const releases = await base.getDirectoryHandle('releases');
    const fh = await releases.getFileHandle(key, { create: false });
    const access = await fh.createSyncAccessHandle();
    const size = access.getSize();
    if (totalSize >= 0 && size !== totalSize) {
      access.close();
      throw new Error('local file size mismatch: ' + size + ' != ' + totalSize);
    }
    const scratch = new Uint8Array(READ_CHUNK);

    let readerPos = 0;
    let lastError = 0;

    Module.mpcRangeRead = function (ptr, want) {
      if (want <= 0) return 0;
      // Clamp to EOF; a short read signals EOS to the decoder contract
      // exactly like the HTTP demand reader does.
      const n = Math.min(want, READ_CHUNK, size - readerPos);
      if (n <= 0) return 0;
      const got = access.read(scratch, { at: readerPos });
      if (got <= 0) {
        lastError = 1;
        return 0;
      }
      HEAPU8.set(scratch.subarray(0, Math.min(n, got)), ptr);
      readerPos += Math.min(n, got);
      return Math.min(n, got);
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
      served: () => readerPos,
      lastError: () => lastError,
      close: () => {
        try { access.close(); } catch (e) { void e; }
        Module.mpcRangeRead = Module.mpcRangeSeek = Module.mpcRangeTell = null;
      },
    };
  }

  global.MusicPackLocal = { installLocalReader };
})(typeof self !== 'undefined' ? self : this);
