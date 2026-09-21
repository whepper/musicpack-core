// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { writable } from '../store';
import { ApiClient } from '../api/client';
import { NetworkError } from '../api/errors';

export type AuthState = 'checking' | 'authenticated' | 'unauthenticated' | 'offline';

/** Injected offline-content probe: true when the catalog holds at least
 *  one installed package (set by bootstrap; avoids an import cycle). */
let offlineContentProbe: () => boolean = () => false;
export function setOfflineContentProbe(probe: () => boolean): void {
  offlineContentProbe = probe;
}
function hasOfflineContent(): boolean {
  try {
    return offlineContentProbe();
  } catch {
    return false;
  }
}

export interface SessionModel {
  state: AuthState;
  /** Human-readable reason for an auth failure (shown on the sign-in screen). */
  message?: string;
}

const initial: SessionModel = { state: 'checking' };

function createSessionStore(api: ApiClient) {
  const store = writable<SessionModel>(initial);

  return {
    ...store,
    async boot(): Promise<void> {
      try {
        await api.session();
        store.set({ state: 'authenticated' });
      } catch (e) {
        // Offline boot (plan §8): a NETWORK failure is not a signed-out
        // state. If the browser holds offline content, enter 'offline'
        // mode so installed packages remain usable; only an actual 401
        // forces sign-in again. (navigator.onLine is unreliable across
        // platforms — Playwright's setOffline keeps it true — so it is
        // deliberately NOT consulted.)
        if (e instanceof NetworkError && hasOfflineContent()) {
          store.set({ state: 'offline' });
          return;
        }
        store.set({
          state: 'unauthenticated',
          message:
            e instanceof Error && e.message.includes('expired')
              ? undefined
              : undefined,
        });
      }
    },
    async authenticate(token: string): Promise<void> {
      await api.createSession(token);
      store.set({ state: 'authenticated' });
    },
    /** Reconnect probe for the 'online' event while in AuthState 'offline'.
     *  Contract: success promotes to 'authenticated'; any failure leaves
     *  state UNCHANGED — it must never demote an offline session to
     *  sign-in. */
    async reattempt(): Promise<boolean> {
      if (store.get().state !== 'offline') return false;
      try {
        await api.session();
        store.set({ state: 'authenticated' });
        return true;
      } catch {
        return false;
      }
    },
    async logout(): Promise<void> {
      try {
        await api.logout();
      } catch {
        /* best effort — the session may already be gone */
      } finally {
        store.set({ state: 'unauthenticated' });
      }
    },
    expire(reason = 'Your session has expired. Please sign in again.'): void {
      store.set({ state: 'unauthenticated', message: reason });
    },
  };
}

export type SessionStore = ReturnType<typeof createSessionStore>;

export { createSessionStore };

let sessionStore: SessionStore | null = null;

export function bindSession(api: ApiClient): SessionStore {
  sessionStore = createSessionStore(api);
  api.onUnauthorized = () => {
    // The auth check (GET /session) deliberately does not retrigger expiry
    // loops; guard against re-entrancy.
    if (sessionStore && sessionStore.get().state === 'authenticated') {
      sessionStore.expire();
    }
  };
  return sessionStore;
}

export function getSession(): SessionStore {
  if (!sessionStore) throw new Error('session not bound (call bindSession first)');
  return sessionStore;
}
