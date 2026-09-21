// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import ValidationPanel from '../../app/src/lib/ui/ValidationPanel.svelte';
import StatusBar from '../../app/src/lib/ui/StatusBar.svelte';
import { draftStore } from '../../app/src/lib/bootstrap';
import { validation, createOpen } from '../../app/src/lib/authoring-state';
import { render, tick, type RenderResult } from './helpers';
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

describe('ValidationPanel', () => {
  afterEach(() => view?.cleanup());

  it('renders errors and warnings separately', () => {
    view = render(ValidationPanel, {
      result: {
        ok: false,
        errors: ['missing required album title', 'duplicate track number 2 on disc 1'],
        warnings: ['missing catalogue number', 'no artwork'],
      },
    });
    const text = view.target.textContent ?? '';
    expect(text).toContain('Errors');
    expect(text).toContain('missing required album title');
    expect(text).toContain('duplicate track number 2 on disc 1');
    expect(text).toContain('Warnings');
    expect(text).toContain('missing catalogue number');
    expect(text).toContain('no artwork');
  });
});

describe('StatusBar create button', () => {
  beforeEach(() => {
    draftStore.clear();
    validation.set(null);
    createOpen.set(false);
  });
  afterEach(() => view?.cleanup());

  it('is disabled until the draft validates green', async () => {
    draftStore.setDraft(draft());
    view = render(StatusBar);
    const createBtn = () =>
      view
        .queryAll('button')
        .find((b) => (b.textContent ?? '').includes('Create MusicPack')) as HTMLButtonElement | null;
    expect(createBtn()?.hasAttribute('disabled')).toBe(true);

    validation.set({ ok: true, errors: [], warnings: [] });
    await tick();
    expect(createBtn()?.hasAttribute('disabled')).toBe(false);

    validation.set({ ok: false, errors: ['no artist'], warnings: [] });
    await tick();
    expect(createBtn()?.hasAttribute('disabled')).toBe(true);
  });

  it('shows a Sonic Analysis chip reflecting the draft state', () => {
    draftStore.setDraft(draft());
    view = render(StatusBar);
    expect(view.target.textContent).toContain('Sonic · not analysed');
    const d = draftStore.draft.get()!;
    expect('analysis' in d).toBe(false);
    expect(d.schema).toBe('musicpack-draft');

    draftStore.updateSonicAnalysis((s) => {
      s.status = 'ready';
      s.profile = 'musicpack-sonic-openl3-v1';
    });
    view = render(StatusBar);
    expect(view.target.textContent).toContain('Sonic ✓');
  });
});
