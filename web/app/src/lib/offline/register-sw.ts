// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Service worker registration (plan §8): registers the static-shell worker
// after load. The worker caches only the immutable app shell; it never
// intercepts /api/**. Registration failures are non-fatal: the app runs
// online-only exactly as before.

export function registerServiceWorker(): void {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return;
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch(() => {
      /* offline reload unsupported in this browser/context — fine */
    });
  });
}
