import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createSessionStore } from '../../app/src/lib/auth/session';
import { ApiClient } from '../../app/src/lib/api/client';

function api(overrides: Record<string, unknown> = {}) {
  return {
    session: vi.fn(),
    createSession: vi.fn(),
    logout: vi.fn(),
    onUnauthorized: null,
    ...overrides,
  } as unknown as ApiClient;
}

describe('auth/session', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('starts checking, then authenticates when the probe succeeds', async () => {
    const a = api();
    (a.session as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({ status: 'authenticated' });
    const store = createSessionStore(a);
    expect(store.get().state).toBe('checking');
    await store.boot();
    expect(store.get().state).toBe('authenticated');
  });

  it('goes unauthenticated when the probe 401s', async () => {
    const a = api();
    (a.session as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('401'));
    const store = createSessionStore(a);
    await store.boot();
    expect(store.get().state).toBe('unauthenticated');
  });

  it('authenticate exchanges the token once and never stores it', async () => {
    const a = api();
    const createSession = (a.createSession as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: 'authenticated',
    });
    const store = createSessionStore(a);
    await store.authenticate('mpk_secret');
    expect(createSession).toHaveBeenCalledWith('mpk_secret');
    expect(store.get().state).toBe('authenticated');
  });

  it('logout always returns to unauthenticated even if the call fails', async () => {
    const a = api();
    (a.logout as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('down'));
    const store = createSessionStore(a);
    store.set({ state: 'authenticated' });
    await store.logout();
    expect(store.get().state).toBe('unauthenticated');
  });

  it('expire keeps the message for the re-auth screen', () => {
    const a = api();
    const store = createSessionStore(a);
    store.expire('Your session has expired. Please sign in again.');
    expect(store.get().state).toBe('unauthenticated');
    expect(store.get().message).toContain('expired');
  });
});
