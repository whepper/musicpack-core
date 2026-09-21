// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

/*
 * MusicPack static-shell service worker (plan §8).
 *
 * Responsibility boundary (deliberate): this worker caches ONLY the
 * immutable application shell — index.html, hashed build assets, the wasm
 * decoder module, the AudioWorklet bundle, and the classic worker scripts
 * the decoder constructs at runtime (decoder.worker.js, musepack.js,
 * reader_mailbox.js, rangereader.js, networker.js, localreader.js).
 *
 * It NEVER touches /api/** (all package data is served by the app from
 * OPFS/IndexedDB), never holds domain state, and never synthesizes
 * responses for anything but its precache list.
 */

const VERSION = 'v6';
const CACHE = `musicpack-shell-${VERSION}`;

const SHELL_ASSETS = [
  '/',
  '/index.html',
  '/manifest.json',
  '/musepack.js',
  '/musepack.wasm',
  '/decoder.worker.js',
  '/reader_mailbox.js',
  '/rangereader.js',
  '/networker.js',
  '/localreader.js',
  // R4.4: the Rust playback runtime must be available on a cold offline start.
  // Offline (OPFS) playback runs through these scripts; without them the
  // AudioWorklet module cannot load while offline.
  '/rust-playback-sink.js',
  '/rust-playback.worker.js',
  '/rust/musicpack_wasm.js',
  '/rust/musicpack_wasm_bg.wasm',
];

self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    // Fixed-name precache: workers + wasm + SPA entry.
    await Promise.allSettled(SHELL_ASSETS.map((a) => cache.add(new Request(a, { cache: 'reload' }))));
    // Hashed build assets never pass through this worker before it exists,
    // so discover them from index.html and precache them too. Without this
    // an offline reload could not boot the app.
    try {
      const indexRes = await fetch('/index.html', { cache: 'reload' });
      const html = await indexRes.text();
      const urls = [...html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)].map((m) => m[1]);
      await Promise.allSettled(urls.map((u) => cache.add(new Request(u, { cache: 'reload' }))));
    } catch (e) {
      void e;
    }
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) {
      if (key !== CACHE) await caches.delete(key);
    }
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET') return;
  // API traffic is never intercepted: auth, ranges and freshness rules
  // belong to the server.
  if (url.pathname.startsWith('/api/')) return;

  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    // Cache-first for everything same-origin under the shell scope: hashed
    // assets are immutable; fixed-name scripts change only with a new SW
    // version. Misses are fetched once, then cached if opaque-safe.
    const hit = await cache.match(event.request);
    if (hit) return hit;
    try {
      const res = await fetch(event.request);
      if (res.ok && url.origin === self.location.origin) {
        cache.put(event.request, res.clone()).catch(() => {});
      }
      return res;
    } catch (e) {
      // Navigation fallback: serve the cached SPA shell when offline.
      if (event.request.mode === 'navigate') {
        const shell = (await cache.match('/index.html')) ?? (await cache.match('/'));
        if (shell) return shell;
      }
      throw e;
    }
  })());
});
