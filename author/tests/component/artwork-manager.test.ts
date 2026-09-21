// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import ArtworkManager from '../../app/src/lib/ui/ArtworkManager.svelte';
import { api, draftStore } from '../../app/src/lib/bootstrap';
import { render, tick, type RenderResult } from './helpers';
import type { Draft } from '../../app/src/lib/types';

const TINY_PNG =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music/A',
    album: { title: 'A', artists: [{ name: 'Artist' }] },
    media: [{ disc: 1, tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [
      { role: 'front', path: 'cover.jpg' },
      { role: 'back', path: '' },
    ],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

let view: RenderResult;
const realReadImage = api.readImage.bind(api);
let readImageCalls = 0;
let readImageImpl: (p: string) => Promise<unknown> = async () => ({
  mime: 'image/jpeg',
  dataBase64: TINY_PNG,
});

describe('ArtworkManager thumbnails', () => {
  beforeEach(() => {
    draftStore.clear();
    readImageCalls = 0;
    readImageImpl = async () => ({ mime: 'image/jpeg', dataBase64: TINY_PNG });
    (api as unknown as Record<string, unknown>).readImage = async (p: string) => {
      readImageCalls += 1;
      return readImageImpl(p);
    };
    draftStore.setDraft(draft());
  });
  afterEach(() => {
    view?.cleanup();
    (api as unknown as Record<string, unknown>).readImage = realReadImage;
  });

  function thumb(index: number): HTMLElement {
    return view.queryAll('.thumb')[index]!;
  }

  /** Flush microtasks plus one macrotask so readImage promises resolve and
   * Svelte applies the resulting DOM updates. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 4; i++) {
      await tick();
      await Promise.resolve();
    }
    await new Promise((r) => setTimeout(r, 0));
    await tick();
  }

  it('renders a real thumbnail for file-backed artwork entries', async () => {
    view = render(ArtworkManager, {});
    await settle();
    const img = thumb(0).querySelector('img');
    expect(img).not.toBeNull();
    expect(img!.getAttribute('src')).toBe(`data:image/jpeg;base64,${TINY_PNG}`);
  });

  it('keeps the embedded label for pathless entries', async () => {
    view = render(ArtworkManager, {});
    await settle();
    expect(thumb(1).textContent?.trim()).toBe('embedded');
  });

  it('falls back to the filename when the image cannot be read, without retrying', async () => {
    readImageImpl = async () => {
      throw new Error('nope');
    };
    view = render(ArtworkManager, {});
    await settle();
    expect(thumb(0).textContent?.trim()).toBe('cover.jpg');
    expect(readImageCalls).toBe(1);

    // a later effect run must not re-fetch the failed path
    draftStore.updateAlbum(() => {});
    await settle();
    expect(readImageCalls).toBe(1);
  });
});
