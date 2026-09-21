// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * SharedArrayBuffer mailbox layout + error codes for the demand-driven
 * Musepack browser reader (Phase 5).
 *
 * Two workers coordinate over one SAB with a two-flag handshake so the
 * decoder's tight back-to-back reads can never deadlock:
 *
 *   decoder worker            network worker
 *   write pos/len
 *   REQ = 1, notify(REQ)  ->  wait until REQ == 1
 *                            read pos/len, REQ = 0 (acknowledge)
 *                            serve (block cache / HTTP Range)
 *                            RES = 1, notify(RES)
 *   wait until RES == 1  <-
 *   read data, RES = 0
 *
 * Control region (Int32 words) then a byte data region:
 *
 *   word 0  REQ        request pending (decoder sets, networker clears)
 *   word 1  RES        response ready / error (networker sets, decoder clears)
 *   word 2  POS_LO     requested absolute byte offset (low 32 bits)
 *   word 3  POS_HI     requested offset (high 32 bits)
 *   word 4  LEN        requested length
 *   word 5  DONE_LEN   bytes written into the data region
 *   word 6  ERROR      error code (0 = none) when RES is set after a failure
 *   word 7  ERROR2     error detail (e.g. HTTP status)
 *   word 8  SERVED     total bytes fetched (demo fetch-accounting)
 *
 * Loadable from classic workers (importScripts) and Node (require).
 */
(function (root, factory) {
  if (typeof module !== "undefined" && module.exports) {
    module.exports = factory();
  } else {
    root.MusicPackMailbox = factory();
  }
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";
  return {
    REQ: 0, RES: 1, POS_LO: 2, POS_HI: 3, LEN: 4, DONE_LEN: 5, ERROR: 6,
    CONTROL_WORDS: 64,             // 256 bytes of control
    DATA_OFFSET: 64 * 4,           // byte offset of the data region
    DATA_CAP: 256 * 1024,          // max bytes per response
    BLOCK: 64 * 1024,              // block-aligned fetch unit
    MAX_BLOCKS: 16,                // block cache entries (~1 MB)
    ERROR2: 7, SERVED: 8,

    ERR_NETWORK: 1,   // fetch threw / timeout / connection loss
    ERR_HTTP: 2,      // HTTP status (401/404/503/...), code in ERROR2
    ERR_200: 3,       // unexpected 200 when Range was requested
    ERR_RANGE: 4,     // bad / missing / mismatched Content-Range
    ERR_TRUNCATED: 5, // body shorter than the Content-Length / requested
    ERR_CLOSED: 6,    // network worker closed
  };
});
