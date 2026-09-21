// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * Network worker for the demand-driven Musepack reader (Phase 5).
 *
 * Owns the audio URL, the Authorization token, a block-aligned byte cache,
 * and the fetch() logic. It shares a SharedArrayBuffer mailbox with the
 * decoder worker using the two-flag REQ/RES handshake (see
 * reader_mailbox.js): when the decoder requests a byte range it fetches (or
 * serves from cache) the covering 64 KiB block and copies the requested
 * slice into the mailbox data region.
 *
 * Network failures are reported through the mailbox as typed error codes;
 * the decoder then surfaces a clear playback error instead of feeding
 * corrupted bytes.
 */

importScripts('reader_mailbox.js');

const M = self.MusicPackMailbox;

let state = null; // Int32Array over the whole SAB (control words)
let data = null;  // Uint8Array over the data region
let url = null;
let token = null;
let totalSize = 0;
const cache = new Map(); // blockBase -> Uint8Array

function respond(ok, code, detail) {
  Atomics.store(state, M.ERROR, ok ? 0 : code);
  Atomics.store(state, M.ERROR2, detail || 0);
  Atomics.store(state, M.RES, 1);
  Atomics.notify(state, M.RES);
}

async function fetchBlock(base) {
  const end = Math.min(totalSize - 1, base + M.BLOCK - 1);
  const headers = { Range: `bytes=${base}-${end}` };
  if (token) headers.Authorization = `Bearer ${token}`;
  let res, body;
  try {
    res = await fetch(url, { headers, signal: AbortSignal.timeout(10000) });
    body = new Uint8Array(await res.arrayBuffer());
  } catch (e) {
    /* timeout / connection loss / aborted mid-body */
    respond(false, M.ERR_NETWORK, 0);
    throw e;
  }
  if (res.status === 206) {
    const cr = res.headers.get('Content-Range') || '';
    const m = /bytes (\d+)-(\d+)\/(\d+)/.exec(cr);
    if (!m) { respond(false, M.ERR_RANGE, 0); throw new Error('missing Content-Range'); }
    if (Number(m[1]) !== base) { respond(false, M.ERR_RANGE, 1); throw new Error('range start mismatch'); }
    if (Number(m[2]) - Number(m[1]) + 1 !== body.length) {
      respond(false, M.ERR_TRUNCATED, 0); throw new Error('truncated 206');
    }
    Atomics.add(state, M.SERVED, body.length);
    return body;
  }
  if (res.status === 200) {
    /* server ignored Range -> cannot do demand-driven reads */
    respond(false, M.ERR_200, 200);
    throw new Error('unexpected 200');
  }
  respond(false, M.ERR_HTTP, res.status);
  throw new Error(`HTTP ${res.status}`);
}

async function handleRequest() {
  const pos = Atomics.load(state, M.POS_HI) * 4294967296 +
              (Atomics.load(state, M.POS_LO) >>> 0);
  const want = Math.min(Atomics.load(state, M.LEN), M.DATA_CAP,
                        totalSize - pos);
  if (want <= 0) {
    Atomics.store(state, M.DONE_LEN, 0);
    respond(true);
    return;
  }
  const base = Math.floor(pos / M.BLOCK) * M.BLOCK;
  let block = cache.get(base);
  if (!block) {
    if (cache.size >= M.MAX_BLOCKS) cache.delete(cache.keys().next().value);
    try {
      block = await fetchBlock(base);      /* respond() on failure + throw */
      cache.set(base, block);
    } catch (e) {
      return;
    }
  }
  const off = pos - base;
  const n = Math.min(want, block.length - off);
  data.set(block.subarray(off, off + n), 0);
  Atomics.store(state, M.DONE_LEN, n);
  respond(true);
}

self.onmessage = (ev) => {
  if (ev.data && ev.data.type === 'open') {
    state = new Int32Array(ev.data.sab);
    data = new Uint8Array(ev.data.sab, M.DATA_OFFSET, M.DATA_CAP);
    url = ev.data.url;
    token = ev.data.token || null;
    totalSize = ev.data.size;
    self.postMessage({ type: 'ready' });
    runLoop();
  }
};

async function runLoop() {
  for (;;) {
    while (Atomics.load(state, M.REQ) === 0)
      Atomics.wait(state, M.REQ, 0);
    Atomics.store(state, M.REQ, 0);   /* acknowledge + reserve the request */
    try {
      await handleRequest();           /* always ends by responding (RES=1) */
    } catch (e) {
      if (Atomics.load(state, M.RES) === 0)
        respond(false, M.ERR_NETWORK, 0);
    }
  }
}
