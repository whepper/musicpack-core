// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Minimal History-API router for the SPA (server provides SPA fallback for
// deep links). Routes:
//   /                     albums shelf
//   /albums/:id           album detail (editions)
//   /albums/:id?release=N deep-link to a specific edition
//   /albums/:id?section=S deep-link to an album section
//   /tracks/:id           track detail (technical inspector)
//   /search?q=            collection search (albums + artists)
//   /artists              artist list
//   /artists/:id          artist detail
//   /queue                now playing / queue
//   /settings             playback quality + storage
import { writable, type Readable } from './store';

export interface Route {
  name: string;
  params: Record<string, string>;
  query: URLSearchParams;
}

interface RouteDef {
  pattern: RegExp;
  name: string;
  keys: string[];
}

const DEFINITIONS: RouteDef[] = [
  { pattern: /^\/$/, name: 'albums', keys: [] },
  { pattern: /^\/albums\/?$/, name: 'albums', keys: [] },
  { pattern: /^\/albums\/([0-9]+)$/, name: 'album', keys: ['id'] },
  { pattern: /^\/tracks\/([0-9]+)$/, name: 'track', keys: ['id'] },
  { pattern: /^\/search\/?$/, name: 'search', keys: [] },
  { pattern: /^\/artists\/?$/, name: 'artists', keys: [] },
  { pattern: /^\/artists\/([0-9]+)$/, name: 'artist', keys: ['id'] },
  { pattern: /^\/queue\/?$/, name: 'queue', keys: [] },
  { pattern: /^\/settings\/?$/, name: 'settings', keys: [] },
];

export function parseRoute(path: string): Route | null {
  const qIndex = path.indexOf('?');
  const pathOnly = qIndex >= 0 ? path.slice(0, qIndex) : path;
  const query = new URLSearchParams(qIndex >= 0 ? path.slice(qIndex + 1) : '');
  for (const def of DEFINITIONS) {
    const m = def.pattern.exec(pathOnly);
    if (m) {
      const params: Record<string, string> = {};
      def.keys.forEach((k, i) => {
        params[k] = m[i + 1] ?? '';
      });
      return { name: def.name, params, query };
    }
  }
  return { name: 'notfound', params: {}, query };
}

export function createRouter() {
  // Include the search string: deep links like /albums/2?release=7&section=audio
  // must resolve their query on a hard load, not only on in-app navigations.
  const current = writable<Route>(
    parseRoute(location.pathname + location.search) ??
      { name: 'notfound', params: {}, query: new URLSearchParams() },
  );

  function go(path: string): void {
    history.pushState({}, '', path);
    current.set(parseRoute(path) ?? { name: 'notfound', params: {}, query: new URLSearchParams() });
  }

  window.addEventListener('popstate', () => {
    current.set(parseRoute(location.pathname) ?? { name: 'notfound', params: {}, query: new URLSearchParams() });
  });

  // In-app anchors must be SPA navigations: a plain <a href> would reload
  // the whole document and wipe the in-memory queue/player state.
  document.addEventListener('click', (ev) => {
    if (ev.defaultPrevented || ev.button !== 0) return;
    if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
    const target = ev.target instanceof Element ? ev.target : null;
    const a = target?.closest('a');
    if (!a) return;
    if (a.target && a.target !== '_self') return;
    if (a.hasAttribute('download')) return;
    const href = a.getAttribute('href');
    if (!href || !href.startsWith('/')) return; // external, hash, mailto, ...
    ev.preventDefault();
    go(href);
  });

  return {
    route: current as Readable<Route>,
    go,
    /** Navigate without pushing history (internal updates). */
    replace(path: string): void {
      history.replaceState({}, '', path);
      current.set(parseRoute(path) ?? { name: 'notfound', params: {}, query: new URLSearchParams() });
    },
  };
}

export type Router = ReturnType<typeof createRouter>;
