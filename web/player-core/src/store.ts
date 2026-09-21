// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Minimal observable store (player-core M3). Svelte-store-compatible
// `subscribe` semantics; plain TS so the core is unit-testable without any
// UI runtime. Mirrors web/app/src/lib/store.ts exactly (web keeps its own
// copy for its non-player stores).

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
