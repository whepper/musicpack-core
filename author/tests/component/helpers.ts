// Minimal Svelte 5 mount harness for component tests (jsdom). Avoids
// @testing-library/svelte, which still targets the legacy component API and
// fights Svelte 5 runes; the web client deliberately keeps component tests
// out and this keeps them light and framework-native.

import { mount, tick, unmount } from 'svelte';
import type { Component } from 'svelte';

export interface RenderResult {
  target: HTMLElement;
  /** queries scoped to the mounted component's root */
  query: <E extends Element = HTMLElement>(selector: string) => E | null;
  queryAll: <E extends Element = HTMLElement>(selector: string) => E[];
  text: (selector: string) => string | null;
  cleanup(): void;
}

export function render<T extends Record<string, unknown>>(
  Component: Component<T>,
  props: T = {} as T,
): RenderResult {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const instance = mount(Component, { target, props });
  const result: RenderResult = {
    target,
    query: (sel) => target.querySelector(sel),
    queryAll: (sel) => Array.from(target.querySelectorAll(sel)),
    text: (sel) => {
      const el = target.querySelector(sel);
      return el ? (el.textContent ?? '').trim() : null;
    },
    cleanup: () => {
      void instance;
      unmount(instance as never);
      target.remove();
    },
  };
  return result;
}

/** Sets a form control value and fires the Svelte-relevant event. */
export async function fireInput(el: Element, value: string): Promise<void> {
  const input = el as HTMLInputElement;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await tick();
}

export async function fireChange(el: Element, value: string): Promise<void> {
  const input = el as HTMLSelectElement;
  input.value = value;
  input.dispatchEvent(new Event('change', { bubbles: true }));
  await tick();
}

export async function click(el: Element): Promise<void> {
  el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await tick();
}

export { tick };
