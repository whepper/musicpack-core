// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { createSimilarityStore } from '../../app/src/lib/state/similarity';
import { ApiClient } from '../../app/src/lib/api/client';

function jsonResponse(status: number, body: unknown): Response {
  return new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

/** Build a store whose API client is backed by a mocked fetch. */
function makeStore() {
  const fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
  const api = new ApiClient();
  const store = createSimilarityStore(api);
  return { store, fetchMock };
}

describe('similarity store', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('loadForTrack sets available + ready when the index is populated', async () => {
    const { store, fetchMock } = makeStore();
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(200, {
          available: true,
          sets: [
            {
              profileId: 'p1',
              profileFingerprint: 'ab'.repeat(32),
              dimensions: 4,
              encoding: 'f32le',
              vectorCount: 2,
              state: 'active',
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse(200, {
          profileId: 'p1',
          profileFingerprint: 'ab'.repeat(32),
          count: 1,
          neighbors: [
            {
              track: {
                id: 7,
                number: 1,
                title: 'T',
                artists: [],
                codec: { codec: 'flac', mimeType: 'audio/flac' },
                audio: { id: 7, size: 1, sha256: 'cd'.repeat(32), url: '/x' },
              },
              release: { id: 2, title: 'R', albumId: 2 },
              score: 0.5,
              rank: 1,
            },
          ],
        }),
      );

    await store.loadForTrack(30);

    expect(store.capability.get().state).toBe('available');
    const tracks = store.tracks.get();
    expect(tracks.state).toBe('ready');
    if (tracks.state === 'ready') {
      expect(tracks.neighbors).toHaveLength(1);
      expect(tracks.neighbors[0]?.track.id).toBe(7);
      expect(tracks.profileId).toBe('p1');
    }
  });

  it('loadForTrack sets unavailable when the capability is absent', async () => {
    const { store, fetchMock } = makeStore();
    fetchMock.mockResolvedValueOnce(
      jsonResponse(200, { available: false, sets: [] }),
    );

    await store.loadForTrack(30);

    expect(store.capability.get().state).toBe('unavailable');
    // The tracks state must be the explicit unavailable state — never an
    // empty list and never a perpetual "loading".
    expect(store.tracks.get().state).toBe('unavailable');
  });

  it('loadForTrack sets unavailable when the track has no vector (404)', async () => {
    const { store, fetchMock } = makeStore();
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(200, {
          available: true,
          sets: [
            {
              profileId: 'p1',
              profileFingerprint: 'ab'.repeat(32),
              dimensions: 4,
              encoding: 'f32le',
              vectorCount: 2,
              state: 'active',
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse(404, {
          error: { code: 'similarity_unavailable', message: 'track has no similarity vector in this profile' },
        }),
      );

    await store.loadForTrack(30);

    expect(store.capability.get().state).toBe('available');
    expect(store.tracks.get().state).toBe('unavailable');
  });

  it('loadSimilar maps a non-404 failure to an error state', async () => {
    const { store, fetchMock } = makeStore();
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(200, {
          available: true,
          sets: [
            {
              profileId: 'p1',
              profileFingerprint: 'ab'.repeat(32),
              dimensions: 4,
              encoding: 'f32le',
              vectorCount: 2,
              state: 'active',
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse(500, { error: { code: 'internal', message: 'query failed' } }),
      );

    await store.loadForTrack(30);

    const tracks = store.tracks.get();
    expect(tracks.state).toBe('error');
    if (tracks.state === 'error') {
      expect(tracks.message).toContain('query failed');
    }
  });

  it('loadStatus maps a network failure to an error capability', async () => {
    const { store, fetchMock } = makeStore();
    fetchMock.mockRejectedValue(new TypeError('fetch failed'));

    await store.loadStatus();

    const cap = store.capability.get();
    expect(cap.state).toBe('error');
    if (cap.state === 'error') {
      expect(cap.message).toBeTruthy();
    }
  });
});
