// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Host codec-capability rules (Phase 4). THE single place that knows which
// codecs this browser plays; both backend resolution (controller.chooseBackend)
// and representation selection feed on it, so the two can never disagree.
// DOM-guarded so Node unit tests and SSR stay safe (false = unknown).

/** Musepack family → the WASM engine handles it natively. */
export function isMusepackCodec(codec?: string): boolean {
  const c = codec ?? '';
  return c === 'musepack' || c === 'musepack-sv7' || c === 'musepack-sv8';
}

/** Browser-native decode support for a MIME hint. */
export function browserSupportsMime(mimeType?: string): boolean {
  const mime = mimeType ?? '';
  if (!mime || typeof document === 'undefined') return false;
  return document.createElement('audio').canPlayType(mime) !== '';
}

/** Playability of one audio object (primary or representation): musepack
 *  always acceptable, everything else by MIME probe. */
export function browserCanPlay(c: { codec?: string; mimeType?: string }): boolean {
  return isMusepackCodec(c.codec) || browserSupportsMime(c.mimeType);
}
