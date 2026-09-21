// @vitest-environment jsdom
// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import CreateDialog from '../../app/src/lib/ui/CreateDialog.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { createOpen, createResult } from '../../app/src/lib/authoring-state';
import { render, click, tick, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

function draft(openedFrom?: string): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'A', artists: [{ name: 'Artist' }] },
    media: [{ disc: 1, tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
    ...(openedFrom ? { openedFrom } : {}),
  };
}

function buttonByText(r: RenderResult, text: string): HTMLButtonElement {
  const b = Array.from(r.target.querySelectorAll('button')).find((el) =>
    (el.textContent ?? '').includes(text),
  );
  if (!b) throw new Error(`no button containing "${text}"`);
  return b as HTMLButtonElement;
}

let view: RenderResult | undefined;

describe('CreateDialog packaging format', () => {
  beforeEach(() => {
    createOpen.set(true);
    createResult.set(null);
    draftStore.setDraft(draft());
  });
  afterEach(() => {
    view?.cleanup();
    view = undefined;
    createOpen.set(false);
    createResult.set(null);
    vi.restoreAllMocks();
  });

  it('defaults to a .mpack directory output', async () => {
    view = render(CreateDialog);
    await tick();
    expect(view.text('.muted')).toContain('.mpack');
    expect(view.text('.muted')).toContain('directory');
    expect((view.query('input[value="mpack"]') as HTMLInputElement).checked).toBe(true);
  });

  it('switching to .mpak changes the hint, path extension and create label', async () => {
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    view = render(CreateDialog);
    await tick();

    await click(view.query('input[value="mpak"]')!);
    expect(view.text('.muted')).toContain('.mpak');
    expect(view.text('.muted')).toContain('container');

    await click(buttonByText(view, 'Choose output…'));
    await tick();
    expect(view.text('.path')).toBe('/out/Artist - A.mpak');
    expect(buttonByText(view, 'Create .mpak').textContent).toContain('Create .mpak');
  });

  it('keeps .mpack as the path extension when that format is selected', async () => {
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Choose output…'));
    await tick();
    expect(view.text('.path')).toBe('/out/Artist - A.mpack');
    expect(buttonByText(view, 'Create').textContent).toContain('Create');
  });

  it('offers Export as .mpak for an opened package and packs the source', async () => {
    draftStore.setDraft(draft('/existing/pkg.mpack'));
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    vi.spyOn(api, 'packPackage').mockResolvedValue({ ok: true, outputPath: '/out/Artist - A.mpak' });
    vi.spyOn(api, 'verifyPackage').mockResolvedValue({ ok: true, errors: [], warnings: [] });

    view = render(CreateDialog);
    await tick();
    expect(buttonByText(view, 'Save changes')).toBeTruthy();

    await click(buttonByText(view, 'Export as .mpak…'));
    await vi.waitFor(() => {
      expect(view!.text('.path')).toBe('/out/Artist - A.mpak');
    });
    expect(api.packPackage).toHaveBeenCalledWith('/existing/pkg.mpack', '/out/Artist - A.mpak');
    // The source directory is never the output target.
    expect(view.text('.path')).not.toContain('/existing/pkg.mpack');
  });

  it('surfaces a pack failure without a successful output', async () => {
    draftStore.setDraft(draft('/existing/pkg.mpack'));
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    vi.spyOn(api, 'packPackage').mockRejectedValue(new Error('source package failed verification'));

    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Export as .mpak…'));
    await vi.waitFor(() => {
      expect(view!.text('.error-banner')).toContain('source package failed verification');
    });
    expect(view.text('h2')).toBe('Package creation failed');
  });
});
