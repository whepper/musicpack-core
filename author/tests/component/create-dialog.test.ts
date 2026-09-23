// @vitest-environment jsdom
// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import CreateDialog from '../../app/src/lib/ui/CreateDialog.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { createOpen, createResult, DEFAULT_QUALITY, encodeQuality } from '../../app/src/lib/authoring-state';
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
    encodeQuality.set(DEFAULT_QUALITY);
    draftStore.setDraft(draft());
  });
  afterEach(() => {
    view?.cleanup();
    view = undefined;
    createOpen.set(false);
    createResult.set(null);
    encodeQuality.set(DEFAULT_QUALITY);
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

describe('CreateDialog quality threading', () => {
  beforeEach(() => {
    createOpen.set(true);
    createResult.set(null);
    encodeQuality.set(DEFAULT_QUALITY);
    draftStore.setDraft(draft());
  });
  afterEach(() => {
    view?.cleanup();
    view = undefined;
    createOpen.set(false);
    createResult.set(null);
    encodeQuality.set(DEFAULT_QUALITY);
    vi.restoreAllMocks();
  });

  it('creates the package at the selected quality (no silent q6 fallback)', async () => {
    // The user picked q7 in the EncodePanel but never ran the encode
    // stage: package creation must still build at q7, never at the
    // hard-coded default.
    encodeQuality.set('7.0');
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    vi.spyOn(api, 'createPackage').mockResolvedValue({
      ok: true,
      outputPath: '/out/Artist - A.mpack',
      replaced: false,
      verify: { errors: 0, warnings: 0 },
    });

    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Choose output…'));
    await tick();
    await click(buttonByText(view, 'Create'));
    await vi.waitFor(() => {
      expect(api.createPackage).toHaveBeenCalledWith(expect.anything(), '/out/Artist - A.mpack', {
        replace: false,
        syncTags: false,
        quality: '7.0',
      });
    });
  });

  it('threads the selected quality into .mpak creation too', async () => {
    encodeQuality.set('5.0');
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    vi.spyOn(api, 'createMpak').mockResolvedValue({ ok: true, outputPath: '/out/Artist - A.mpak' });

    view = render(CreateDialog);
    await tick();
    await click(view.query('input[value="mpak"]')!);
    await click(buttonByText(view, 'Choose output…'));
    await tick();
    await click(buttonByText(view, 'Create .mpak'));
    await vi.waitFor(() => {
      expect(api.createMpak).toHaveBeenCalledWith(
        expect.anything(),
        '/out/Artist - A.mpak',
        '5.0',
      );
    });
  });
});
