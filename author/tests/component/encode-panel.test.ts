// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import EncodePanel from '../../app/src/lib/ui/EncodePanel.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { encodeStaging, setEncodeStaging } from '../../app/src/lib/authoring-state';
import { render, click, tick, type RenderResult } from './helpers';
import type { Draft, EncodeProgress } from '../../app/src/lib/types';

function draft(flac = true): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'Album', artists: [{ name: 'Artist' }] },
    media: [
      {
        disc: 1,
        tracks: [
          { track: 1, title: 'A', audioPath: '1.flac', codec: flac ? 'flac' : 'musepack-sv8' },
          { track: 2, title: 'B', audioPath: flac ? '2.flac' : '2.mpc', codec: flac ? 'flac' : 'musepack-sv8' },
        ],
      },
    ],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

function encodedDraft(): Draft {
  const d = draft();
  d.sourceRoot = '/tmp/stage';
  const tracks = d.media?.[0]?.tracks ?? [];
  d.media = [{
    disc: 1,
    tracks: tracks.map((t, i) => ({
      ...t,
      audioPath: `audio/0${i + 1} - ${t.title}.mpc`,
      codec: 'musepack-sv8',
      streamVersion: 8,
      sourceAudio: { codec: 'flac' },
    })),
  }];
  return d;
}

let view: RenderResult;

describe('EncodePanel', () => {
  beforeEach(() => {
    setEncodeStaging(null);
    draftStore.clear();
    draftStore.setDraft(draft());
    vi.restoreAllMocks();
  });
  afterEach(() => {
    view?.cleanup();
    setEncodeStaging(null);
  });

  it('is hidden when every track is already Musepack', () => {
    draftStore.setDraft(draft(false));
    view = render(EncodePanel);
    expect(view.target.textContent).not.toContain('Encode to Musepack');
  });

  it('encodes and swaps in the transformed draft', async () => {
    vi.spyOn(api, 'encodeTracks').mockImplementation(async (_d, _q, onProgress) => {
      onProgress?.({ event: 'stage', stage: 'encoding', done: 0, total: 2, disc: 1, track: 1, title: 'A' });
      onProgress?.({ event: 'track', done: 1, total: 2, disc: 1, track: 1, status: 'ok' });
      return { ok: true, outputDir: '/tmp/stage', tracks: 2, draft: encodedDraft() };
    });
    view = render(EncodePanel);
    const btn = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Encode to Musepack'))!;
    await click(btn);
    expect(api.encodeTracks).toHaveBeenCalledOnce();
    expect(encodeStaging.get()).toBe('/tmp/stage');
    const first = draftStore.draft.get()?.media?.[0]?.tracks?.[0];
    expect(first?.audioPath).toBe('audio/01 - A.mpc');
    expect(first?.codec).toBe('musepack-sv8');
    expect(view.target.textContent).toContain('ready to create the package');
  });

  it('shows progress while running', async () => {
    let resolve: (r: { ok: boolean; cancelled: boolean }) => void = () => {};
    vi.spyOn(api, 'encodeTracks').mockImplementation(async (_d, _q, onProgress) => {
      onProgress?.({ event: 'stage', stage: 'tagging', done: 1, total: 2, disc: 1, track: 2, title: 'B' });
      return new Promise((res) => {
        resolve = res;
      });
    });
    view = render(EncodePanel);
    const btn = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Encode to Musepack'))!;
    await click(btn);
    await tick();
    expect(view.target.textContent).toContain('Tagging');
    expect(view.target.textContent).toContain('1 / 2 tracks');
    resolve({ ok: true, cancelled: false });
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    const cancel = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Cancel'));
    expect(cancel).toBeFalsy();
  });

  it('shows an actionable error and allows retry', async () => {
    vi.spyOn(api, 'encodeTracks').mockRejectedValue(new Error('track 2 on disc 1 — mpcenc exited with code 1'));
    view = render(EncodePanel);
    const btn = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Encode to Musepack'))!;
    await click(btn);
    expect(view.target.textContent).toContain('mpcenc exited with code 1');
    expect(encodeStaging.get()).toBeNull();
  });

  it('clears staging on cancel', async () => {
    setEncodeStaging('/tmp/stage');
    vi.spyOn(api, 'encodeCancel').mockResolvedValue(undefined);
    vi.spyOn(api, 'encodeTracks').mockResolvedValue({ ok: false, cancelled: true });
    view = render(EncodePanel);
    const btn = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Encode to Musepack'))!;
    await click(btn);
    expect(encodeStaging.get()).toBeNull();
  });

  it('passes the selected quality through', async () => {
    vi.spyOn(api, 'encodeTracks').mockImplementation(async (_d, q) => {
      expect(q).toBe('6.0');
      return { ok: true, cancelled: false };
    });
    view = render(EncodePanel);
    const btn = view.queryAll('button').find((b) => (b.textContent ?? '').includes('Encode to Musepack'))!;
    await click(btn);
  });
});
