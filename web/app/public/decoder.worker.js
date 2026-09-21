// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * MusicPack web client decoder worker (Phase 6).
 *
 * Classic worker that owns one libmusepack WASM instance plus the Phase 5
 * demand-driven range reader (networker + SharedArrayBuffer mailbox). The
 * playback controller keeps TWO of these: the current track's worker and a
 * second worker already opened on the NEXT track, so gapless album playback
 * continues at the exact sample boundary.
 *
 * Protocol (main -> worker):
 *   { type:'open', url, size, token?, generation } open a track
 *   { type:'play', generation }          decode one credited PCM chunk
 *   { type:'pause' }                     stop decoding
 *   { type:'seek', sample }              seek; stale decode output is dropped
 *   { type:'close' }                     free the handle + reader
 * (worker -> main):
 *   { type:'info', rate, channels, version, lengthSamples }
 *   { type:'pcm', samples: Float32Array }   interleaved, transferred
 *   { type:'seeked', sample }
 *   { type:'eos' }
 *   { type:'error', message }
 */

importScripts('musepack.js');
importScripts('reader_mailbox.js');
importScripts('rangereader.js');
importScripts('localreader.js');

const FRAMES_PER_CHUNK = 8 * 1152; // 8 decoder frames per message

let Module = null;
let handle = -1;
let heapPtr = 0;
let pcmPtr = 0;
let channels = 2;
let playing = false;
let eos = false;
let pumping = false;
let demandReader = null;
let generation = 0;
let reportGeneration = 0;

function post(msg, transfer) {
  self.postMessage(
    msg,
    transfer ? [msg.samples.buffer] : undefined,
  );
}

async function init() {
  if (Module) return;
  Module = await createMusepackModule();
}

async function open(url, size, token, localKey) {
  if (handle >= 0) destroy();
  handle = Module._mpc_wasm_create();
  if (handle < 0) throw new Error('mpc_wasm_create failed');

  /* Offline path: the item's source is a locally installed OPFS file
     (keyed by name). Same read contract, zero network. */
  if (localKey) {
    demandReader = await MusicPackLocal.installLocalReader(Module, localKey, size);
  } else {
    demandReader = await MusicPackRange.installDemandReader(Module, url, size,
                                                            token);
  }
  const err = Module._mpc_wasm_open_range(handle, size);
  if (err !== 0) {
    const le = demandReader.lastError();
    demandReader.close();
    demandReader = null;
    throw new Error('mpc_wasm_open_range returned ' + err +
                    (le ? ` (reader error ${le})` : ''));
  }
  pcmPtr = Module._malloc(FRAMES_PER_CHUNK * channels * 4);
  postInfo();
}

function postInfo() {
  channels = Module._mpc_wasm_channels(handle);
  post({
    type: 'info',
    rate: Module._mpc_wasm_sample_rate(handle),
    channels,
    version: Module._mpc_wasm_stream_version(handle),
    lengthSamples: Module._mpc_wasm_length_samples(handle),
    generation: reportGeneration,
  });
  postStats();
}

function postStats() {
  if (demandReader) {
    post({ type: 'stats', served: demandReader.served(), generation: reportGeneration });
  }
}

function destroy() {
  if (handle >= 0) {
    if (pcmPtr) Module._free(pcmPtr);
    if (heapPtr) Module._free(heapPtr);
    Module._mpc_wasm_destroy(handle);
  }
  if (demandReader) {
    demandReader.close();
    demandReader = null;
  }
  handle = -1; heapPtr = 0; pcmPtr = 0;
}

async function readChunk() {
  const gen = generation;
  const outputGeneration = reportGeneration;
  const frames = await Module._mpc_wasm_read(handle, pcmPtr, FRAMES_PER_CHUNK);
  if (gen !== generation) return; /* a seek arrived; drop stale decode */
  if (frames < 0) {
    if (frames === -5) { /* MUSEPACK_ERR_EOF */
      eos = true;
      post({ type: 'eos', generation: outputGeneration });
    } else {
      let msg = 'decoder error ' + frames;
      if (demandReader && demandReader.lastError())
        msg += ` (stream read error ${demandReader.lastError()})`;
      post({ type: 'error', message: msg, generation: outputGeneration });
    }
    return;
  }
  if (frames === 0) {
    eos = true;
    post({ type: 'eos', generation: outputGeneration });
    return;
  }
  const view = new Float32Array(Module.HEAPF32.buffer, pcmPtr, frames * channels);
  post({ type: 'pcm', samples: view.slice(), generation: outputGeneration }, true);
}

async function pump() {
  if (!playing || eos || handle < 0) {
    pumping = false;
    return;
  }
  try {
    await readChunk();
  } finally {
    pumping = false;
  }
}

self.onmessage = async (ev) => {
  const msg = ev.data;
  try {
    await init();
    switch (msg.type) {
      case 'open':
        generation++;
        reportGeneration = msg.generation;
        playing = false;
        eos = false;
        await open(msg.url, msg.size, msg.token || null, msg.localKey || null);
        break;
      case 'play':
        reportGeneration = msg.generation;
        playing = true;
        if (!pumping) {
          pumping = true;
          pump();
        }
        break;
      case 'pause':
        if (msg.generation === reportGeneration) playing = false;
        break;
      case 'seek':
        if (handle >= 0) {
          const seekGeneration = ++generation;
          const outputGeneration = msg.generation;
          reportGeneration = outputGeneration;
          await Module._mpc_wasm_seek_sample(handle, msg.sample);
          if (seekGeneration !== generation) break;
          eos = false;
          post({ type: 'seeked', sample: msg.sample, generation: outputGeneration });
          postStats();
        }
        break;
      case 'close':
        generation++;
        playing = false;
        eos = false;
        pumping = false;
        destroy();
        /* Ack so the outer worker can be terminated only after the nested
           demand-reader/network worker is gone (no per-track leak). */
        post({ type: 'closed' });
        break;
    }
  } catch (e) {
    post({ type: 'error', message: String(e), generation: reportGeneration });
  }
};
