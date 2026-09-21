import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ApiClient } from '../../app/src/lib/api/client';
import { ApiError, NetworkError } from '../../app/src/lib/api/errors';

function jsonResponse(status: number, body: unknown): Response {
  return new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('ApiClient', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('parses album pages', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse(200, { albums: [{ id: 1, title: 'X', artists: [], releaseCount: 1 }], total: 1, limit: 50, offset: 0 }),
    );
    vi.stubGlobal('fetch', fetchMock);
    const api = new ApiClient();
    const page = await api.albums({ limit: 50 });
    expect(page.total).toBe(1);
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/albums?limit=50');
  });

  it('normalizes the flattened track-detail wire shape into track + context', async () => {
    // The endpoint spreads the Track fields at the top level and appends
    // `context`; the client owns that quirk, callers see { track, context }.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        jsonResponse(200, {
          id: 30,
          number: 1,
          title: 'Big in Japan',
          artists: [],
          codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack', streamVersion: 8, sampleRate: 44100, channels: 2 },
          audio: { id: 30, size: 493369, sha256: 'ab'.repeat(32), url: '/api/v1/tracks/30/audio' },
          context: { disc: 1, albumId: 5, albumTitle: 'Long Player', releaseId: 8, releaseEdition: '1986 Original CD' },
        }),
      ),
    );
    const api = new ApiClient();
    const detail = await api.trackDetail(30);
    expect(detail.track.id).toBe(30);
    expect(detail.track.title).toBe('Big in Japan');
    expect(detail.track.codec.streamVersion).toBe(8);
    expect(detail.context.releaseId).toBe(8);
    expect(detail.context.albumTitle).toBe('Long Player');
    // no context leakage onto the track object itself
    expect('context' in detail.track).toBe(false);
  });

  it('carries track-detail lyrics refs through the normalization (R3.3 wire contract)', async () => {
    const lyrics = [
      { id: 12, url: '/api/v1/assets/12', size: 842, mimeType: 'text/plain', sha256: 'ab'.repeat(32), lang: 'en' },
      { id: 13, url: '/api/v1/assets/13', size: 640, mimeType: 'text/plain' },
    ];
    const detailBody = {
      id: 30,
      number: 1,
      title: 'Big in Japan',
      artists: [],
      codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
      audio: { id: 30, size: 1, url: '/api/v1/tracks/30/audio' },
      context: { disc: 1, albumId: 5, albumTitle: 'Long Player', releaseId: 8 },
      lyrics,
    };
    // Fresh Response per call (a Response body can be read only once).
    vi.stubGlobal(
      'fetch',
      vi.fn().mockImplementation((input: RequestInfo | URL) => {
        const url = String(input);
        if (url.includes('/api/v1/assets/')) {
          return Promise.resolve(new Response('lyric bytes', { status: 200 }));
        }
        return Promise.resolve(new Response(JSON.stringify(detailBody), { status: 200 }));
      }),
    );
    const api = new ApiClient();
    const detail = await api.trackDetail(30);
    // The wire field sits at the top level (after context); the client's
    // rest-spread keeps it on the track object verbatim — no invention.
    expect(detail.track.lyrics).toEqual(lyrics);
    expect(detail.track.lyrics?.[0]?.lang).toBe('en');
    expect(detail.track.lyrics?.[1]?.sha256).toBeUndefined();
    expect(detail.track.lyrics?.[1]?.lang).toBeUndefined();
    // raw fetch through the asset endpoint returns exact bytes
    const bytes = await api.assetBytes('/api/v1/assets/12');
    expect(Array.from(bytes)).toEqual(Array.from(new TextEncoder().encode('lyric bytes')));
  });

  it('leaves lyrics undefined when the server omits the field', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        jsonResponse(200, {
          id: 31,
          number: 2,
          title: 'The Van',
          artists: [],
          codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
          audio: { id: 31, size: 1, url: '/api/v1/tracks/31/audio' },
          context: { disc: 1, albumId: 5, albumTitle: 'Long Player', releaseId: 8 },
        }),
      ),
    );
    const api = new ApiClient();
    const detail = await api.trackDetail(31);
    expect(detail.track.lyrics).toBeUndefined();
  });

  it('assetBytes maps a failed asset fetch to a typed ApiError', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(404, { error: { code: 'not_found', message: 'Asset not found' } })),
    );
    const api = new ApiClient();
    await expect(api.assetBytes('/api/v1/assets/999')).rejects.toMatchObject({
      code: 'not_found',
      status: 404,
    });
  });

  it('maps typed server error codes to friendly ApiErrors', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(404, { error: { code: 'not_found', message: 'Track not found' } })),
    );
    const api = new ApiClient();
    await expect(api.album('999')).rejects.toMatchObject({
      code: 'not_found',
      status: 404,
      message: expect.stringContaining('no longer in the collection'),
    });
  });

  it('falls back to internal for a non-JSON error body', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('boom', { status: 500 })));
    const api = new ApiClient();
    await expect(api.albums()).rejects.toBeInstanceOf(ApiError);
  });

  it('throws NetworkError when fetch rejects', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('fetch failed')));
    const api = new ApiClient();
    await expect(api.albums()).rejects.toBeInstanceOf(NetworkError);
  });

  it('sends the bearer token when a token provider is configured', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { albums: [], total: 0, limit: 50, offset: 0 }));
    vi.stubGlobal('fetch', fetchMock);
    const api = new ApiClient({ token: () => 'mpk_test' });
    await api.albums();
    const headers = (fetchMock.mock.calls[0]?.[1] as RequestInit).headers as Headers;
    expect(headers.get('Authorization')).toBe('Bearer mpk_test');
  });

  it('notifies onUnauthorized on 401', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(401, { error: { code: 'unauthorized', message: 'x' } })),
    );
    const api = new ApiClient();
    const spy = vi.fn();
    api.onUnauthorized = spy;
    await expect(api.albums()).rejects.toBeInstanceOf(ApiError);
    expect(spy).toHaveBeenCalledOnce();
  });

  it('session exchange posts the token and uses cookies by default', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { status: 'authenticated' }));
    vi.stubGlobal('fetch', fetchMock);
    const api = new ApiClient();
    await api.createSession('mpk_secret');
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/session');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ token: 'mpk_secret' });
  });

  it('surfaces content hashes on representations, waveforms and assets (offline integrity)', async () => {
    const sha = 'a'.repeat(64);
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse(200, {
        id: 7,
        album: { id: 1, title: 'X', artists: [] },
        media: [
          {
            disc: 1,
            tracks: [
              {
                id: 55,
                number: 1,
                title: 'T',
                artists: [],
                codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
                audio: { id: 90, size: 10, sha256: sha, url: '/api/v1/tracks/55/audio' },
                representations: [
                  {
                    id: 91,
                    size: 20,
                    sha256: sha,
                    url: '/api/v1/tracks/55/representations/91/audio',
                    codec: { codec: 'flac', mimeType: 'audio/flac' },
                  },
                ],
                waveform: {
                  version: 1,
                  intervalMs: 100,
                  encoding: 'peak-rms-u8',
                  floorDb: -60,
                  points: 4,
                  sha256: sha,
                  url: '/api/v1/tracks/55/waveform',
                },
              },
            ],
          },
        ],
        artwork: [{ id: 7, kind: 'artwork', mimeType: 'image/jpeg', sha256: sha, url: '/api/v1/assets/7' }],
        assets: [],
      }),
    );
    vi.stubGlobal('fetch', fetchMock);
    const api = new ApiClient();
    const rel = await api.release(7);
    const track = rel.media[0]!.tracks[0]!;
    expect(track.representations?.[0]?.sha256).toBe(sha);
    expect(track.waveform?.sha256).toBe(sha);
    expect(rel.artwork[0]?.sha256).toBe(sha);
  });

  it('carries the release-level trackLyrics index verbatim (R3.6)', async () => {
    const sha = 'cd'.repeat(32);
    const trackLyrics = [
      { trackId: 55, id: 501, url: '/api/v1/assets/501', size: 42, sha256: sha, lang: 'en' },
      { trackId: 55, id: 503, url: '/api/v1/assets/503', size: 17, sha256: sha },
    ];
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        jsonResponse(200, {
          id: 7,
          album: { id: 3, title: 'T', artists: [] },
          media: [],
          artwork: [],
          assets: [],
          trackLyrics,
        }),
      ),
    );
    const api = new ApiClient();
    const rel = await api.release(7);
    expect(rel.trackLyrics).toEqual(trackLyrics);
    expect(rel.trackLyrics?.[0]?.lang).toBe('en');
    expect(rel.trackLyrics?.[1]?.lang).toBeUndefined();
  });

  it('leaves trackLyrics undefined when the release has no track lyrics (R3.6)', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        jsonResponse(200, {
          id: 8,
          album: { id: 3, title: 'T', artists: [] },
          media: [],
          artwork: [],
          assets: [],
        }),
      ),
    );
    const api = new ApiClient();
    const rel = await api.release(8);
    expect(rel.trackLyrics).toBeUndefined();
  });
});
