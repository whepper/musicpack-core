// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Minimal observable store primitives (Svelte-store-compatible `subscribe`,
// but plain TS so the core logic is unit-testable without the Svelte
// runtime). Mirrors the web client's lib/store.ts.

export interface Readable<T> {
  subscribe(fn: (value: T) => void): () => void;
  get(): T;
}

export interface Writable<T> extends Readable<T> {
  set(value: T): void;
  update(fn: (value: T) => T): void;
}

type Listener<T> = (value: T) => void;

export function writable<T>(initial: T): Writable<T> {
  let value = initial;
  const listeners = new Set<Listener<T>>();
  return {
    subscribe(fn: Listener<T>): () => void {
      listeners.add(fn);
      fn(value); // synchronous first emit, per the Svelte store contract
      return () => listeners.delete(fn);
    },
    get() {
      return value;
    },
    set(next: T) {
      if (Object.is(value, next)) return;
      value = next;
      for (const fn of listeners) fn(value);
    },
    update(fn: (value: T) => T) {
      this.set(fn(value));
    },
  };
}

export function derived<T>(compute: () => T): Readable<T> & { refresh(): void } {
  const store = writable<T>(compute());
  return {
    subscribe: store.subscribe.bind(store),
    get: store.get.bind(store),
    refresh() {
      store.set(compute());
    },
  };
}
