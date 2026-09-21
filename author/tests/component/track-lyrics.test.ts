// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import TrackLyrics from '../../app/src/lib/ui/TrackLyrics.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { render, tick, type RenderResult } from './helpers';
import type { Draft, Track } from '../../app/src/lib/types';

function draftWith(tracks: Track[]): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/Album',
    album: { title: 'Album', artists: [{ name: 'Artist' }] },
    media: [{ disc: 1, tracks }],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

const TRACK: Track = { track: 1, title: 'One', audioPath: '01.mpc' };

let view: RenderResult;
const realPickLyricsFile = api.pickLyricsFile.bind(api);
const realLyricsProbe = api.lyricsProbe.bind(api);
let picked: string | null = null;
let probeImpl: (p: string) => Promise<unknown> = async () => ({ ok: true, synced: true, lines: 4 });

describe('TrackLyrics', () => {
  beforeEach(() => {
    draftStore.clear();
    picked = null;
    probeImpl = async () => ({ ok: true, synced: true, lines: 4 });
    (api as unknown as Record<string, unknown>).pickLyricsFile = async () => picked;
    (api as unknown as Record<string, unknown>).lyricsProbe = async (p: string) => probeImpl(p);
  });
  afterEach(() => {
    view?.cleanup();
    (api as unknown as Record<string, unknown>).pickLyricsFile = realPickLyricsFile;
    (api as unknown as Record<string, unknown>).lyricsProbe = realLyricsProbe;
  });

  async function settle(): Promise<void> {
    for (let i = 0; i < 4; i++) {
      await tick();
      await new Promise((r) => setTimeout(r, 0));
    }
  }

  function mountTrack(track: Track = TRACK) {
    draftStore.setDraft(draftWith([{ ...track }]));
    view = render(TrackLyrics, { disc: 0, index: 0, sourceRoot: '/music/Album' });
  }

  it('shows the empty state for a lyric-less track', () => {
    mountTrack();
    expect(view.target.textContent).toContain('No lyrics.');
    expect(view.query('button')?.textContent).toContain('Add lyrics');
  });

  it('renders attached refs with filename, lang and probe summary', async () => {
    mountTrack({ ...TRACK, lyrics: [{ path: 'lyrics/one.lrc', lang: 'en' }] });
    await settle();
    const text = view.target.textContent ?? '';
    expect(text).toContain('one.lrc');
    expect(text).toContain('en');
    expect(text).toContain('synced · 4 lines');
  });

  it('adds a picked file under the album directory', async () => {
    mountTrack();
    picked = '/music/Album/lyrics/one.lrc';
    view.query('button')!.click();
    await settle();
    const refs = draftStore.draft.get()!.media[0]!.tracks[0]!.lyrics;
    expect(refs).toEqual([{ path: 'lyrics/one.lrc' }]);
    expect(view.target.textContent).toContain('one.lrc');
  });

  it('refuses files outside the album directory with a note', async () => {
    mountTrack();
    picked = '/elsewhere/song.lrc';
    view.query('button')!.click();
    await settle();
    expect(draftStore.draft.get()!.media[0]!.tracks[0]!.lyrics).toBeUndefined();
    expect(view.target.textContent).toContain('inside the album directory');
  });

  it('refuses invalid files with the probe message and attaches nothing', async () => {
    mountTrack();
    picked = '/music/Album/lyrics/bad.lrc';
    probeImpl = async () => ({ ok: false, error: { code: 'invalid_lyrics', message: 'malformed lyrics: bad' } });
    view.query('button')!.click();
    await settle();
    expect(draftStore.draft.get()!.media[0]!.tracks[0]!.lyrics).toBeUndefined();
    expect(view.target.textContent).toContain('malformed lyrics: bad');
  });

  it('removes a ref and clears the key', async () => {
    mountTrack({ ...TRACK, lyrics: [{ path: 'lyrics/one.lrc', lang: 'en' }] });
    await settle();
    const remove = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Remove'))!;
    remove.click();
    await settle();
    expect(draftStore.draft.get()!.media[0]!.tracks[0]!.lyrics).toBeUndefined();
    expect(view.target.textContent).toContain('No lyrics.');
  });

  it('edits the language without touching the path', async () => {
    mountTrack({ ...TRACK, lyrics: [{ path: 'lyrics/one.lrc' }] });
    await settle();
    const input = view.query('input[aria-label="Lyric language for lyrics/one.lrc"]') as HTMLInputElement;
    input.value = 'fr';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    expect(draftStore.draft.get()!.media[0]!.tracks[0]!.lyrics).toEqual([
      { path: 'lyrics/one.lrc', lang: 'fr' },
    ]);
  });
});
