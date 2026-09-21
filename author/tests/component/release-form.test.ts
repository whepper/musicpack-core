// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import ReleaseForm from '../../app/src/lib/ui/ReleaseForm.svelte';
import { draftStore } from '../../app/src/lib/bootstrap';
import { render, fireInput, fireChange, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'Original Title', artists: [{ name: 'Artist' }] },
    release: { releaseDate: '1987-01-01', edition: '1987 CD' },
    media: [{ disc: 1, format: 'CD', tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

let view: RenderResult;

describe('ReleaseForm', () => {
  beforeEach(() => {
    draftStore.clear();
    draftStore.setDraft(draft());
  });
  afterEach(() => view?.cleanup());

  it('edits the album title', async () => {
    view = render(ReleaseForm);
    const title = view.query('#f-title');
    expect(title).not.toBeNull();
    await fireInput(title!, 'Renamed Album');
    expect(draftStore.draft.get()?.album.title).toBe('Renamed Album');
  });

  it('edits release-group and specific-release fields without merging', async () => {
    view = render(ReleaseForm);
    await fireChange(view.query('#f-type')!, 'compilation');
    await fireInput(view.query('#f-ed')!, '2016 Remaster');
    const d = draftStore.draft.get()!;
    expect(d.album.releaseType).toBe('compilation');
    expect(d.release?.edition).toBe('2016 Remaster');
    expect(d.release?.releaseDate).toBe('1987-01-01');
  });

  it('edits the source block independently of the release', async () => {
    view = render(ReleaseForm);
    await fireInput(view.query('#f-store')!, 'Deezer');
    const d = draftStore.draft.get()!;
    expect(d.source?.store).toBe('Deezer');
    expect(d.release?.edition).toBe('1987 CD');
  });
});
