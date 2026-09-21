// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import WorkflowNav from '../../app/src/lib/ui/WorkflowNav.svelte';
import { draftStore } from '../../app/src/lib/bootstrap';
import { STAGES, activeStage, validation } from '../../app/src/lib/authoring-state';
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

describe('WorkflowNav', () => {
  beforeEach(() => {
    draftStore.clear();
    validation.set(null);
    activeStage.set('identity');
  });
  afterEach(() => view?.cleanup());

  it('renders one tab per workflow stage', () => {
    view = render(WorkflowNav);
    const tabs = view.queryAll('[role="tab"]');
    expect(tabs.length).toBe(STAGES.length);
    expect(view.text('#stage-tab-identity')).toContain('Identity');
    expect(view.text('#stage-tab-validate')).toContain('Validate');
  });

  it('activates a stage on click and reflects it in aria-selected', async () => {
    view = render(WorkflowNav);
    await click(view.query('#stage-tab-tracks')!);
    expect(activeStage.get()).toBe('tracks');
    expect(view.query('#stage-tab-tracks')!.getAttribute('aria-selected')).toBe('true');
    expect(view.query('#stage-tab-identity')!.getAttribute('aria-selected')).toBe('false');
  });

  it('moves the selection with arrow keys (roving tabindex)', async () => {
    view = render(WorkflowNav);
    const list = view.query('[role="tablist"]')!;
    list.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
    );
    await tick();
    expect(activeStage.get()).toBe('release');
  });

  it('marks completed stages complete and pending stages not started', () => {
    draftStore.setDraft(draft());
    view = render(WorkflowNav);
    // .mpc track: nothing to encode; metadata + audio + artwork are filled
    expect(view.query('#stage-tab-tracks')!.getAttribute('aria-label')).toBe(
      'Tracks — complete',
    );
    expect(view.query('#stage-tab-artwork')!.getAttribute('aria-label')).toBe(
      'Artwork — complete',
    );
    expect(view.query('#stage-tab-identity')!.getAttribute('aria-label')).toBe(
      'Identity — not started',
    );
    expect(view.query('#stage-tab-validate')!.getAttribute('aria-label')).toBe(
      'Validate — not started',
    );
  });

  it('marks the Validate stage complete only once validation passes', () => {
    draftStore.setDraft(draft());
    validation.set({ ok: false, errors: ['x'], warnings: [] });
    view = render(WorkflowNav);
    expect(view.query('#stage-tab-validate')!.getAttribute('aria-label')).toBe(
      'Validate — needs attention',
    );

    view.cleanup();
    validation.set({ ok: true, errors: [], warnings: [] });
    view = render(WorkflowNav);
    expect(view.query('#stage-tab-validate')!.getAttribute('aria-label')).toBe(
      'Validate — complete',
    );
  });
});
