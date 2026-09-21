// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Minimal observable store (Svelte-store-compatible `subscribe`, but plain
// TS so the core logic is unit-testable without the Svelte runtime).

export interface Readable<T> {
  subscribe(fn: (value: T) => void): () => void;
  get(): T;
}

export interface Writable<T> extends Readable<T> {
  set(value: T): void;
  update(fn: (value: T) => T): void;
}

export function writable<T>(initial: T): Writable<T> {
  let value = initial;
  const subs = new Set<(v: T) => void>();
  return {
    subscribe(fn) {
      subs.add(fn);
      fn(value);
      return () => subs.delete(fn);
    },
    get: () => value,
    set(v) {
      value = v;
      for (const fn of [...subs]) fn(value);
    },
    update(fn) {
      value = fn(value);
      for (const f of [...subs]) f(value);
    },
  };
}

export function derived<T>(compute: () => T): Readable<T> & { refresh(): void } {
  const store = writable(compute());
  return {
    subscribe: store.subscribe,
    get: store.get,
    refresh: () => store.set(compute()),
  };
}
