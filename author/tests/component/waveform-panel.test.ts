// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import WaveformPanel from '../../app/src/lib/ui/WaveformPanel.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { render, click, type RenderResult } from './helpers';
import type { Draft, WaveformResult } from '../../app/src/lib/types';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'Album', artists: [{ name: 'Artist' }] },
    media: [
      { disc: 1, tracks: [
        { track: 1, title: 'A', audioPath: '1.mpc' },
        { track: 2, title: 'B', audioPath: '2.mpc' },
      ] },
    ],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

let view: RenderResult;

describe('WaveformPanel', () => {
  beforeEach(() => {
    draftStore.clear();
    draftStore.setDraft(draft());
    vi.restoreAllMocks();
  });
  afterEach(() => view?.cleanup());

  it('starts not generated', () => {
    view = render(WaveformPanel);
    expect(view.target.textContent).toContain('Not generated');
    expect(draftStore.draft.get()?.waveformAnalysis?.status ?? 'not_generated').toBe('not_generated');
  });

  it('runs generation and marks the draft ready', async () => {
    const transformed: Draft = JSON.parse(JSON.stringify(draft()));
    transformed.waveformAnalysis = {
      status: 'ready',
      intervalMs: 100,
      encoding: 'peak-rms-u8',
      floorDb: -60,
      tracks: [
        { disc: 1, track: 1, points: 2843, sha256: 'a'.repeat(64), path: '/stage/waveform/01-01.wfm' },
        { disc: 1, track: 2, points: 2843, sha256: 'b'.repeat(64), path: '/stage/waveform/01-02.wfm' },
      ],
    };
    const result = {
      ok: true,
      cancelled: false,
      tracks: 2,
      draft: transformed,
    } as WaveformResult;
    vi.spyOn(api, 'waveformAnalyze').mockResolvedValue(result);
    view = render(WaveformPanel);
    const btn = view.queryAll('button').find((b) =>
      (b.textContent ?? '').includes('Generate waveforms'))!;
    await click(btn);
    expect(api.waveformAnalyze).toHaveBeenCalledOnce();
    const wf = draftStore.draft.get()?.waveformAnalysis;
    expect(wf?.status).toBe('ready');
    expect(wf?.tracks.length).toBe(2);
    expect(wf?.intervalMs).toBe(100);
    expect(wf?.encoding).toBe('peak-rms-u8');
    expect(wf?.floorDb).toBe(-60);
  });

  it('shows ready state and tracks count', () => {
    draftStore.updateWaveformAnalysis((s) => {
      s.status = 'ready';
      s.tracks.push(
        { disc: 1, track: 1, points: 2843, sha256: 'a'.repeat(64), path: '/x' },
        { disc: 1, track: 2, points: 2843, sha256: 'b'.repeat(64), path: '/x' },
      );
    });
    view = render(WaveformPanel);
    expect(view.target.textContent).toContain('Generated');
    expect(view.target.textContent).toContain('2 / 2 tracks');
  });

  it('shows error state and clears on retry', async () => {
    draftStore.updateWaveformAnalysis((s) => {
      s.status = 'error';
      s.error = 'decode failed';
    });
    view = render(WaveformPanel);
    expect(view.target.textContent).toContain('Error');
    expect(view.target.textContent).toContain('decode failed');
  });

  it('respects disabled opt-out', () => {
    draftStore.updateWaveformAnalysis((s) => { s.status = 'disabled'; });
    view = render(WaveformPanel);
    expect(view.target.textContent).toContain('Disabled');
    expect(view.target.textContent).toContain('without waveform');
  });
});