// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The user's audio-representation preference (Phase 4): one tiny web-app
// setting under its own localStorage key. Deliberately OUTSIDE the
// player-core session snapshot — the snapshot schema stays frozen and the
// core never learns about representations.
//
// Changing the preference affects FUTURE item construction only; already-
// built queue items (including a restored session) keep their resolved
// sources. Storage is injectable for tests; production uses localStorage
// with SSR guards.

import { writable, type Writable } from '../store';
import { parseAudioPreference, type AudioPreference } from './representation-selection';

const STORAGE_KEY = 'musicpack.audio-preference.v1';

/** Minimal storage seam (localStorage-shaped). */
export interface PreferenceStorage {
  get(): string | null;
  set(value: string | null): void;
}

function defaultStorage(): PreferenceStorage {
  return {
    get: () => (typeof localStorage === 'undefined' ? null : localStorage.getItem(STORAGE_KEY)),
    set: (value) => {
      if (typeof localStorage === 'undefined') return;
      if (value === null) localStorage.removeItem(STORAGE_KEY);
      else localStorage.setItem(STORAGE_KEY, value);
    },
  };
}

export interface AudioPreferenceStore {
  /** Reactive view (UI consumes this later; no UI exists yet). */
  readonly preference: Writable<AudioPreference>;
  get(): AudioPreference;
  /** Sets AND persists. Invalid values are coerced to the default. */
  set(p: AudioPreference): void;
}

export function createAudioPreferenceStore(storage?: PreferenceStorage): AudioPreferenceStore {
  const store = storage ?? defaultStorage();
  const initial = parseAudioPreference(loadRaw(store)) ?? { mode: 'default' };
  const pref = writable<AudioPreference>(initial);
  return {
    preference: pref,
    get: () => pref.get(),
    set(p) {
      const valid = parseAudioPreference(p) ?? { mode: 'default' };
      pref.set(valid);
      try {
        store.set(JSON.stringify(valid));
      } catch {
        /* persistence is best-effort; the in-memory value still applies */
      }
    },
  };
}

function loadRaw(store: PreferenceStorage): unknown {
  try {
    const raw = store.get();
    return raw === null ? null : (JSON.parse(raw) as unknown);
  } catch {
    return null; // corrupt payload → default
  }
}

export { STORAGE_KEY as AUDIO_PREFERENCE_STORAGE_KEY };
