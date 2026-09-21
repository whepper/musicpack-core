// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import AlbumAuthoring from '../../app/src/lib/ui/AlbumAuthoring.svelte';
import { draftStore } from '../../app/src/lib/bootstrap';
import { activeStage } from '../../app/src/lib/authoring-state';
import { render, click, tick, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'A', artists: [{ name: 'Artist' }] },
    media: [{ disc: 1, tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [{ role: 'front', path: 'cover.jpg' }],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

let view: RenderResult;

describe('AlbumAuthoring stage panels', () => {
  beforeEach(() => {
    document.title = '';
    activeStage.set('identity');
    draftStore.setDraft(draft());
  });
  afterEach(() => view?.cleanup());

  function panel(id: string): HTMLElement {
    return view.query(`#stage-panel-${id}`)!;
  }

  it('shows only the active stage and hides the rest', async () => {
    view = render(AlbumAuthoring, { onReset: () => {} });
    await tick();
    expect(panel('identity').hasAttribute('hidden')).toBe(false);
    expect(panel('release').hasAttribute('hidden')).toBe(true);
    expect(panel('validate').hasAttribute('hidden')).toBe(true);
  });

  it('switches the visible stage when the stepper is used', async () => {
    view = render(AlbumAuthoring, { onReset: () => {} });
    await click(view.query('#stage-tab-artwork')!);
    expect(activeStage.get()).toBe('artwork');
    expect(panel('artwork').hasAttribute('hidden')).toBe(false);
    expect(panel('identity').hasAttribute('hidden')).toBe(true);
  });

  it('keeps every stage mounted so state survives stage switches', async () => {
    view = render(AlbumAuthoring, { onReset: () => {} });
    await click(view.query('#stage-tab-tracks')!);
    // all eight panels remain in the DOM, hidden or not
    const ids = [
      'identity',
      'release',
      'tracks',
      'artwork',
      'encode',
      'sonic',
      'waveform',
      'validate',
    ];
    for (const id of ids) expect(view.query(`#stage-panel-${id}`)).not.toBeNull();
  });

  it('keeps the album header intact', () => {
    view = render(AlbumAuthoring, { onReset: () => {} });
    expect(view.text('.author-header h1')).toBe('A');
    expect(view.text('.author-header .artist')).toBe('Artist');
  });
});
