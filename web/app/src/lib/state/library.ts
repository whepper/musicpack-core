// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { writable } from '../store';
import type { ApiClient } from '../api/client';
import type { AlbumDetail, AlbumSummary, ArtistDetail, ReleaseDetail } from '../api/types';

const PAGE = 50;

export interface ShelfState {
  albums: AlbumSummary[];
  total: number;
  offset: number;
  loading: boolean;
  done: boolean;
  error?: string;
  q: string;
  sort: string;
  refreshKey: number;
}

export function createLibraryStore(api: ApiClient) {
  const shelf = writable<ShelfState>({
    albums: [],
    total: 0,
    offset: 0,
    loading: false,
    done: false,
    q: '',
    sort: '',
    refreshKey: 0,
  });

  const albumCache = new Map<string, AlbumDetail>();
  const releaseCache = new Map<string, ReleaseDetail>();
  const artistCache = new Map<string, ArtistDetail>();
  /** albumId -> currently selected releaseId */
  const selection = new Map<number, number>();

  let pending: Promise<void> | null = null;

  async function fetchPage(q: string, sort: string, offset: number): Promise<void> {
    const page = await api.albums({ limit: PAGE, offset, q, sort });
    shelf.update((s) => {
      const known = new Set(s.albums.map((a) => a.id));
      const fresh = page.albums.filter((a) => !known.has(a.id));
      return {
        ...s,
        albums: [...s.albums, ...fresh],
        total: page.total,
        offset: offset + fresh.length,
        done: s.albums.length + fresh.length >= page.total || fresh.length === 0,
      };
    });
  }

  return {
    shelf,
    /** Reset and start a new search/sort. */
    async browse(opts: { q?: string; sort?: string } = {}): Promise<void> {
      const q = (opts.q ?? '').trim();
      const sort = opts.sort ?? '';
      shelf.update((s) => ({ ...s, q, sort, albums: [], offset: 0, done: false, loading: true, error: undefined, refreshKey: s.refreshKey + 1 }));
      try {
        await fetchPage(q, sort, 0);
      } catch (e) {
        shelf.update((s) => ({ ...s, error: (e as Error).message, loading: false }));
      } finally {
        shelf.update((s) => ({ ...s, loading: false }));
      }
    },
    async loadMore(): Promise<void> {
      const s = shelf.get();
      if (s.loading || s.done || pending) return;
      pending = (async () => {
        shelf.update((st) => ({ ...st, loading: true }));
        try {
          await fetchPage(s.q, s.sort, s.offset);
        } catch (e) {
          shelf.update((st) => ({ ...st, error: (e as Error).message }));
        } finally {
          shelf.update((st) => ({ ...st, loading: false }));
          pending = null;
        }
      })();
      return pending;
    },

    async albumDetail(id: number | string): Promise<AlbumDetail> {
      const key = String(id);
      const cached = albumCache.get(key);
      if (cached) return cached;
      const detail = await api.album(id);
      albumCache.set(key, detail);
      return detail;
    },

    async releaseDetail(id: number | string): Promise<ReleaseDetail> {
      const key = String(id);
      const cached = releaseCache.get(key);
      if (cached) return cached;
      const detail = await api.release(id);
      releaseCache.set(key, detail);
      return detail;
    },

    /** Bypasses the cache and overwrites it: update/reinstall actions must
     *  consume the server's CURRENT content hashes, never the ones captured
     *  when the page was first opened. */
    async refreshRelease(id: number | string): Promise<ReleaseDetail> {
      const key = String(id);
      const detail = await api.release(id);
      releaseCache.set(key, detail);
      return detail;
    },

    async artistDetail(id: number | string): Promise<ArtistDetail> {
      const key = String(id);
      const cached = artistCache.get(key);
      if (cached) return cached;
      const detail = await api.artist(id);
      artistCache.set(key, detail);
      return detail;
    },

    /** Default release for an album (first), or the stored selection. */
    selectRelease(albumId: number, releaseId: number): void {
      selection.set(albumId, releaseId);
    },
    selectedRelease(albumId: number, releases: Array<{ id: number }>): number {
      const sel = selection.get(albumId);
      if (sel !== undefined && releases.some((r) => r.id === sel)) return sel;
      const first = releases[0]?.id;
      if (first !== undefined) selection.set(albumId, first);
      return first ?? -1;
    },
  };
}

export type LibraryStore = ReturnType<typeof createLibraryStore>;
