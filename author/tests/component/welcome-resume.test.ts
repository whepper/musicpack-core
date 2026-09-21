// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import Welcome from '../../app/src/lib/ui/Welcome.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { render, click, tick, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'Saved album', artists: [{ name: 'Artist' }] },
    media: [{ disc: 1, tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

const realDraftLoad = api.draftLoad.bind(api);
const realRecentsList = api.recentsList.bind(api);

let view: RenderResult;

describe('Welcome resume draft', () => {
  beforeEach(() => {
    draftStore.clear();
    // The singleton api is constructed for Tauri; substitute the session
    // methods so the component can load a saved draft in jsdom.
    (api as unknown as Record<string, unknown>).draftLoad = async () =>
      JSON.stringify(draft());
    (api as unknown as Record<string, unknown>).recentsList = async () => [];
  });
  afterEach(() => {
    view?.cleanup();
    (api as unknown as Record<string, unknown>).draftLoad = realDraftLoad;
    (api as unknown as Record<string, unknown>).recentsList = realRecentsList;
  });

  it('offers and resumes the autosaved draft', async () => {
    view = render(Welcome, {
      onOpen: (_p: string) => {},
      onResume: (_d: Draft) => {},
    });
    await tick();
    await tick();
    expect(view.query('.resume-card')).not.toBeNull();

    const onResume = vi.fn();
    // re-render with a spy (cleanup + fresh mount keeps helpers simple)
    view.cleanup();
    view = render(Welcome, { onOpen: (_p: string) => {}, onResume });
    await tick();
    await tick();

    const btn = view
      .queryAll('button')
      .find((b) => (b.textContent ?? '').includes('Resume draft'))!;
    expect(btn).toBeDefined();
    await click(btn);

    expect(onResume).toHaveBeenCalledTimes(1);
    const received = onResume.mock.calls[0]![0] as Draft;
    expect(received.album.title).toBe('Saved album');

    // Regression: the resumed draft must be a plain object, not a $state
    // proxy — setDraft() uses structuredClone(), which throws on proxies
    // and made this button silently do nothing.
    expect(() => structuredClone(received)).not.toThrow();

    // simulating app.svelte wiring end-to-end
    draftStore.setDraft(received);
    expect(draftStore.draft.get()).not.toBeNull();
    expect(draftStore.draft.get()?.album.title).toBe('Saved album');
  });

  it('hides the offer after Discard', async () => {
    view = render(Welcome, {
      onOpen: (_p: string) => {},
      onResume: (_d: Draft) => {},
    });
    await tick();
    await tick();
    const discard = view
      .queryAll('button')
      .find((b) => (b.textContent ?? '').trim() === 'Discard')!;
    await click(discard);
    expect(view.query('.resume-card')).toBeNull();
  });
});
