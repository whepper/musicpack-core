// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 4: the persisted audio-preference setting. Runs fully in Node via an
// injected localStorage-shaped storage — no browser involved.

import { describe, it, expect } from 'vitest';
import {
  AUDIO_PREFERENCE_STORAGE_KEY,
  createAudioPreferenceStore,
} from '../../app/src/lib/state/preferences';

function fakeStorage(initial: string | null = null): {
  backing: { get(): string | null; set(value: string | null): void };
  value(): string | null;
} {
  let current = initial;
  return {
    backing: {
      get: () => current,
      set: (v) => {
        current = v;
      },
    },
    value: () => current,
  };
}

describe('audio preference store (Phase 4 persistence)', () => {
  it('defaults when storage is empty', () => {
    const { backing } = fakeStorage();
    const store = createAudioPreferenceStore(backing);
    expect(store.get()).toEqual({ mode: 'default' });
  });

  it('loads a persisted preference', () => {
    const { backing } = fakeStorage(JSON.stringify({ mode: 'lossless' }));
    expect(createAudioPreferenceStore(backing).get()).toEqual({ mode: 'lossless' });
  });

  it('falls back to default on corrupt or invalid stored values', () => {
    for (const bad of ['not json', '{"mode":"shiny"}', '42', '{"mode":"codec"}', '[1]']) {
      const { backing } = fakeStorage(bad);
      expect(createAudioPreferenceStore(backing).get()).toEqual({ mode: 'default' });
    }
  });

  it('set() persists and affects future construction boundaries', () => {
    const { backing, value } = fakeStorage();
    const store = createAudioPreferenceStore(backing);
    store.set({ mode: 'representation', id: 91 });
    expect(store.get()).toEqual({ mode: 'representation', id: 91 });
    expect(value()).toBe(JSON.stringify({ mode: 'representation', id: 91 }));

    // "Reload": a fresh store instance reads the same storage.
    expect(createAudioPreferenceStore(backing).get()).toEqual({
      mode: 'representation',
      id: 91,
    });
  });

  it('set() with an invalid value coerces to default and persists it', () => {
    const { backing, value } = fakeStorage();
    const store = createAudioPreferenceStore(backing);
    // @ts-expect-error deliberately invalid input
    store.set({ mode: 'nope' });
    expect(store.get()).toEqual({ mode: 'default' });
    expect(value()).toBe(JSON.stringify({ mode: 'default' }));
  });

  it('the writable view mirrors set() for future UI consumers', () => {
    const { backing } = fakeStorage();
    const store = createAudioPreferenceStore(backing);
    const seen: unknown[] = [];
    const unsub = store.preference.subscribe((p) => seen.push(p));
    store.set({ mode: 'lossless' });
    unsub();
    expect(seen).toEqual([{ mode: 'default' }, { mode: 'lossless' }]);
  });

  it('uses its own dedicated storage key (snapshot schema untouched)', () => {
    expect(AUDIO_PREFERENCE_STORAGE_KEY).toBe('musicpack.audio-preference.v1');
  });

  it('survives a throwing storage without losing the in-memory value', () => {
    const store = createAudioPreferenceStore({
      get: () => null,
      set: () => {
        throw new Error('quota exceeded');
      },
    });
    store.set({ mode: 'codec', codec: 'flac' });
    expect(store.get()).toEqual({ mode: 'codec', codec: 'flac' });
  });
});
