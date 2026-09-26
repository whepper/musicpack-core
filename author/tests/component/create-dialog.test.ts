// @vitest-environment jsdom
// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import CreateDialog from '../../app/src/lib/ui/CreateDialog.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { createOpen, createResult, DEFAULT_QUALITY, encodeQuality } from '../../app/src/lib/authoring-state';
import { render, click, tick, type RenderResult } from './helpers';
import type { BuildProgress, CreateResult, Draft } from '../../app/src/lib/types';

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
      expect(api.createPackage).toHaveBeenCalledWith(
        expect.anything(),
        '/out/Artist - A.mpack',
        {
          replace: false,
          syncTags: false,
          quality: '7.0',
        },
        expect.any(Function),
      );
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
        expect.any(Function),
      );
    });
  });
});

describe('CreateDialog build progress', () => {
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

  /** Drives the dialog's progress callback the way the `build-progress`
   * event would. The build stays in flight until `finish()` so progress can
   * be observed while the command is genuinely pending. */
  function mockBuildWithProgress(): {
    emit: (p: Partial<BuildProgress>) => void;
    finish: () => void;
  } {
    let progress: ((p: BuildProgress) => void) | undefined;
    let resolveBuild: ((v: CreateResult) => void) | undefined;
    vi.spyOn(api, 'pickOutputDirectory').mockResolvedValue('/out');
    vi.spyOn(api, 'createPackage').mockImplementation((_d, _out, _opts, onProgress) => {
      progress = onProgress;
      return new Promise<CreateResult>((resolve) => {
        resolveBuild = resolve;
      });
    });
    return {
      emit: (p) => progress?.(p as BuildProgress),
      finish: () =>
        resolveBuild?.({
          ok: true,
          outputPath: '/out/Artist - A.mpack',
          replaced: false,
          verify: { errors: 0, warnings: 0 },
        }),
    };
  }

  it('shows the live phase while the package is being created', async () => {
    const build = mockBuildWithProgress();
    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Choose output…'));
    await tick();

    await click(buttonByText(view, 'Create'));
    await vi.waitFor(() => {
      expect(api.createPackage).toHaveBeenCalled();
    });

    // A track-granular phase narrates both the phase and the track count.
    build.emit({ phase: 'waveform', step: 2, steps: 6, done: 3, total: 12 });
    await tick();
    expect(view.text('.build-progress')).toContain('Generating waveforms');
    expect(view.text('.build-progress')).toContain('step 2 of 6');
    expect(view.text('.build-progress')).toContain('3 / 12 tracks');

    // Entering the package phase shows the phase label with no counter, until
    // the core builder reports which sub-stage it is in.
    build.emit({ phase: 'package', step: 5, steps: 6, done: 0, total: 0 });
    await tick();
    expect(view.text('.build-progress')).toContain('Building the package');
    expect(view.text('.build-progress')).not.toContain('tracks');

    build.finish();
  });

  it('narrates the package phase sub-stages with their own counters', async () => {
    // The package phase is the long one; the core builder reports its
    // sub-stages so the line moves instead of parking on one label.
    const build = mockBuildWithProgress();
    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Choose output…'));
    await tick();
    await click(buttonByText(view, 'Create'));
    await vi.waitFor(() => {
      expect(api.createPackage).toHaveBeenCalled();
    });

    build.emit({
      phase: 'package',
      step: 5,
      steps: 6,
      done: 7,
      total: 23,
      detail: 'assets',
      unit: 'assets',
    });
    await tick();
    expect(view.text('.build-progress')).toContain('Copying and hashing assets');
    expect(view.text('.build-progress')).toContain('7 / 23 assets');

    build.emit({
      phase: 'package',
      step: 5,
      steps: 6,
      done: 4,
      total: 10,
      detail: 'loudness',
      unit: 'tracks',
    });
    await tick();
    expect(view.text('.build-progress')).toContain('Measuring loudness');
    expect(view.text('.build-progress')).toContain('4 / 10 tracks');

    // Verification is a single step, so it carries no counter.
    build.emit({
      phase: 'package',
      step: 5,
      steps: 6,
      done: 1,
      total: 0,
      detail: 'verify',
    });
    await tick();
    expect(view.text('.build-progress')).toContain('Verifying the package');
    expect(view.text('.build-progress')).not.toContain('/');

    build.finish();
  });

  it('clears the progress line when the dialog is reopened', async () => {
    const build = mockBuildWithProgress();
    view = render(CreateDialog);
    await tick();
    await click(buttonByText(view, 'Choose output…'));
    await tick();
    await click(buttonByText(view, 'Create'));
    await vi.waitFor(() => {
      expect(api.createPackage).toHaveBeenCalled();
    });
    build.emit({ phase: 'package', step: 5, steps: 6, done: 0, total: 0 });
    await tick();
    expect(view.text('.build-progress')).toContain('step 5 of 6');

    // A finished build shows the result panel instead of the progress line…
    build.finish();
    await vi.waitFor(() => {
      expect(view!.text('h2')).toBe('Package created');
    });
    expect(view.query('.build-progress')).toBeNull();

    // …and reopening starts clean, with no stale phase left behind.
    await click(buttonByText(view, 'Close'));
    await tick();
    createOpen.set(true);
    await tick();
    expect(view.query('.build-progress')).toBeNull();
  });
});
