// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Main-thread lyrics binding over `musicpack-wasm` (R3.2 surface:
// lyrics_open / lyrics_doc / lyrics_active_line / lyrics_close,
// docs/musicpack-lyrics-v1.md §9-§10). Parsing and timing stay in Rust —
// this adapter only loads the generated glue and narrows it to the port
// the lyrics controller consumes.
//
// The playback path runs its WASM inside rust-playback.worker.js; lyrics
// are a main-thread concern (one parse per track, one integer lookup per
// position tick), so this loader brings up a second, independent module
// instance from the same generated artifacts in `public/rust/`
// (scripts/build-wasm.mjs). Load discipline under the app CSP
// (index.html: script-src 'self' 'wasm-unsafe-eval'):
//
//   1. inject the generated glue as a classic <script> (self ✓);
//   2. inject the generated `lyrics-bindings.js` capture, which publishes
//      the glue's global-lexical `wasm_bindgen` as a window property
//      (inline capture would need unsafe-inline — blocked);
//   3. fetch the module bytes and `initSync` (WebAssembly.Module runs
//      under 'wasm-unsafe-eval' ✓ — the same pattern the playback worker
//      uses inside its own scope).
//
// Not unit-tested (DOM/network glue, like native-backend's loader path);
// the controller consumes the port through an injected fake, and the e2e
// suite exercises the real module end to end.

/** The narrow wasm surface the lyrics controller needs. */
export interface LyricsCore {
  /** Parses LRC bytes under the strict profile; returns a handle.
   *  Throws on malformed input. */
  open(bytes: Uint8Array): number;
  /** The document's static content as canonical JSON. */
  doc(handle: number): string;
  /** Normative timing lookup (spec §9): active line index or −1. */
  activeLine(handle: number, positionMs: number): number;
  /** Releases a document; closed handles must not be used again. */
  close(handle: number): void;
}

/** Minimal shape of the wasm-bindgen no-modules module object we rely on.
 *  `lyrics_active_line`'s `position_ms` is an i64 across the wasm ABI —
 *  wasm-bindgen surfaces that as `bigint`, so the adapter converts the
 *  controller's floored number at the edge (spec §9 keeps the JS-side
 *  contract in milliseconds as numbers). */
interface WasmBindgenModule {
  initSync(module_or_path: { module: WebAssembly.Module }): unknown;
  lyrics_open(bytes: Uint8Array): number;
  lyrics_doc(handle: number): string;
  lyrics_active_line(handle: number, position_ms: bigint): number;
  lyrics_close(handle: number): void;
}

declare global {
  interface Window {
    /** Published by the generated `rust/lyrics-bindings.js` capture. */
    __musicpackLyricsWasm?: WasmBindgenModule;
  }
}

export interface WasmLyricsCoreOptions {
  /** Base URL for the generated artifacts (default: same origin). */
  base?: string;
  /** Injection seams (tests / alternative hosts). */
  loadScript?: (url: string) => Promise<void>;
  loadWasmBytes?: (url: string) => Promise<ArrayBuffer>;
}

function injectScript(url: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const el = document.createElement('script');
    el.src = url;
    el.async = false;
    el.onload = () => resolve();
    el.onerror = () => reject(new Error(`cannot load ${url}`));
    document.head.appendChild(el);
  });
}

function fetchBytes(url: string): Promise<ArrayBuffer> {
  return fetch(url).then((r) => {
    if (!r.ok) throw new Error(`${url} → ${r.status}`);
    return r.arrayBuffer();
  });
}

/** Creates a memoized loader for the lyrics binding. All failures reject —
 *  the controller owns the degrade-to-error policy. */
export function createWasmLyricsCore(
  opts: WasmLyricsCoreOptions = {},
): () => Promise<LyricsCore> {
  const base = opts.base ?? '';
  const loadScript = opts.loadScript ?? injectScript;
  const loadWasmBytes = opts.loadWasmBytes ?? fetchBytes;
  let instance: Promise<LyricsCore> | null = null;
  return () => {
    instance ??= (async () => {
      // Sequential: the capture reads the glue's lexical binding, so the
      // glue must be evaluated first.
      await loadScript(`${base}/rust/musicpack_wasm.js`);
      await loadScript(`${base}/rust/lyrics-bindings.js`);
      const mod = window.__musicpackLyricsWasm;
      if (!mod) throw new Error('lyrics bindings not published');
      const bytes = await loadWasmBytes(`${base}/rust/musicpack_wasm_bg.wasm`);
      mod.initSync({ module: new WebAssembly.Module(bytes) });
      return {
        open: (bytes) => mod.lyrics_open(bytes),
        doc: (handle) => mod.lyrics_doc(handle),
        activeLine: (handle, positionMs) => mod.lyrics_active_line(handle, BigInt(positionMs)),
        close: (handle) => mod.lyrics_close(handle),
      } satisfies LyricsCore;
    })();
    return instance;
  };
}
