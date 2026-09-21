// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import TrackList from '../../app/src/lib/ui/TrackList.svelte';
import { draftStore } from '../../app/src/lib/bootstrap';
import { render, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

function twoDiscDraft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/Two Discs',
    album: { title: 'Two Discs', artists: [{ name: 'Artist' }] },
    media: [
      {
        disc: 1,
        format: 'CD',
        tracks: [
          { track: 1, title: 'First Track', audioPath: '01 - First Track.mpc', codec: 'musepack-sv8', duration: 252.3 },
          { track: 2, title: 'Second Track', audioPath: '02 - Second Track.mpc', codec: 'musepack-sv8' },
        ],
      },
      { disc: 2, format: 'CD', tracks: [{ track: 1, title: 'Disc Two Opener', audioPath: '02 - Disc Two Opener.mpc' }] },
    ],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

let view: RenderResult;

describe('TrackList', () => {
  beforeEach(() => {
    draftStore.clear();
  });
  afterEach(() => view?.cleanup());

  it('renders discs and tracks with codec and duration', () => {
    draftStore.setDraft(twoDiscDraft());
    view = render(TrackList);
    const text = view.target.textContent ?? '';
    expect(text).toContain('Disc 1');
    expect(text).toContain('Disc 2');
    expect(text).toContain('First Track');
    expect(text).toContain('Second Track');
    expect(text).toContain('Disc Two Opener');
    expect(text).toContain('4:12');
    expect(text).toContain('MPC');
    expect(text).toContain('01 - First Track.mpc');
  });

  it('keeps the authored disc and track order', () => {
    draftStore.setDraft(twoDiscDraft());
    view = render(TrackList);
    const rows = view.queryAll('.track .tt').map((el) => (el.textContent ?? '').trim());
    expect(rows.length).toBe(3);
    expect(rows[0]).toContain('First Track');
    expect(rows[1]).toContain('Second Track');
    expect(rows[2]).toContain('Disc Two Opener');
  });
});
