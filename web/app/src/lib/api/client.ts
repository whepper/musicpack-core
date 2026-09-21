// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { ApiError, NetworkError } from './errors';
import type {
  AlbumDetail,
  AlbumPage,
  ArtistDetail,
  ArtistPage,
  LibraryStatus,
  ReleaseDetail,
  SessionInfo,
  Track,
  TrackDetail,
} from './types';

export interface ApiClientOptions {
  base?: string;
  /** Bearer token override (dev/CLI); null = session cookie. */
  token?: () => string | null;
}

export class ApiClient {
  readonly base: string;
  private readonly token?: () => string | null;
  /** Set by the auth layer: fires when an authenticated call gets a 401. */
  onUnauthorized: (() => void) | null = null;

  constructor(opts: ApiClientOptions = {}) {
    this.base = opts.base ?? '';
    this.token = opts.token;
  }

  /** Public base URL (used by non-client fetchers, e.g. waveform planner). */
  get baseUrl(): string {
    return this.base;
  }

  /** Current bearer token override, if any (null = session cookie). */
  getToken(): string | null {
    return this.token?.() ?? null;
  }

  async raw(path: string, init: RequestInit = {}, auth = true): Promise<Response> {
    const headers = new Headers(init.headers);
    const tok = this.token?.() ?? null;
    if (tok) headers.set('Authorization', `Bearer ${tok}`);
    let res: Response;
    try {
      res = await fetch(this.base + path, {
        ...init,
        headers,
        credentials: 'same-origin',
        signal: init.signal ?? AbortSignal.timeout(20000),
      });
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') {
        throw new NetworkError(e);
      }
      throw new NetworkError(e);
    }
    if (res.status === 401 && auth && this.onUnauthorized) {
      this.onUnauthorized();
    }
    return res;
  }

  async json<T>(path: string, init: RequestInit = {}, auth = true): Promise<T> {
    const res = await this.raw(path, init, auth);
    const body = res.status === 204 ? null : await res.text();
    if (!res.ok) {
      let code: ApiError['code'] = 'internal';
      let detail: string | undefined;
      try {
        const err = JSON.parse(body ?? '{}') as {
          error?: { code?: string; message?: string };
        };
        if (err.error?.code) code = err.error.code as ApiError['code'];
        detail = err.error?.message;
      } catch {
        /* non-JSON error body */
      }
      throw new ApiError(code, res.status, detail);
    }
    return body ? (JSON.parse(body) as T) : (undefined as T);
  }

  // ---- session ----------------------------------------------------------

  createSession(token: string): Promise<{ status: string }> {
    return this.json('/api/v1/session', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    }, false);
  }

  logout(): Promise<void> {
    return this.json('/api/v1/session', { method: 'DELETE' }, false);
  }

  session(): Promise<{ status: string; session?: SessionInfo }> {
    return this.json('/api/v1/session');
  }

  // ---- library ----------------------------------------------------------

  albums(params: {
    limit?: number;
    offset?: number;
    q?: string;
    sort?: string;
  } = {}): Promise<AlbumPage> {
    const q = new URLSearchParams();
    if (params.limit !== undefined) q.set('limit', String(params.limit));
    if (params.offset !== undefined) q.set('offset', String(params.offset));
    if (params.q) q.set('q', params.q);
    if (params.sort) q.set('sort', params.sort);
    const qs = q.toString();
    return this.json(`/api/v1/albums${qs ? `?${qs}` : ''}`);
  }

  album(id: number | string): Promise<AlbumDetail> {
    return this.json(`/api/v1/albums/${id}`);
  }

  release(id: number | string): Promise<ReleaseDetail> {
    return this.json(`/api/v1/releases/${id}`);
  }

  track(id: number | string): Promise<Track> {
    return this.json(`/api/v1/tracks/${id}`);
  }

  /** Track detail including the owning release/album context block —
   *  the bare `track()` endpoint stays available for callers that only
   *  need the playback fields. The endpoint spreads the Track fields at
   *  the top level and appends `context`; normalized here into
   *  `{ track, context }` so callers never see the wire quirk. */
  async trackDetail(id: number | string): Promise<TrackDetail> {
    const { context, ...track } = await this.json<
      Track & { context: TrackDetail['context'] }
    >(`/api/v1/tracks/${id}`);
    return { track, context };
  }

  artists(params: { limit?: number; offset?: number; q?: string } = {}): Promise<ArtistPage> {
    const q = new URLSearchParams();
    if (params.limit !== undefined) q.set('limit', String(params.limit));
    if (params.offset !== undefined) q.set('offset', String(params.offset));
    if (params.q) q.set('q', params.q);
    const qs = q.toString();
    return this.json(`/api/v1/artists${qs ? `?${qs}` : ''}`);
  }

  artist(id: number | string): Promise<ArtistDetail> {
    return this.json(`/api/v1/artists/${id}`);
  }

  /** Raw asset bytes (lyric documents, R3.4) through the existing asset
   *  endpoint contract. Throws ApiError/NetworkError like every call. */
  async assetBytes(url: string): Promise<Uint8Array> {
    const res = await this.raw(url);
    if (!res.ok) {
      let code: ApiError['code'] = 'internal';
      let detail: string | undefined;
      try {
        const err = JSON.parse(await res.text()) as { error?: { code?: string; message?: string } };
        if (err.error?.code) code = err.error.code as ApiError['code'];
        detail = err.error?.message;
      } catch {
        /* non-JSON error body */
      }
      throw new ApiError(code, res.status, detail);
    }
    return new Uint8Array(await res.arrayBuffer());
  }

  libraryStatus(): Promise<LibraryStatus> {
    return this.json('/api/v1/library/status');
  }
}

export type { Track };
