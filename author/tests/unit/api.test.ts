import { describe, it, expect, vi } from 'vitest';
import { AuthorApi, type InvokeFn } from '../../app/src/lib/api';
import type { Draft } from '../../app/src/lib/types';

function draft(): Draft {
  return {
    schema: 'musicpack-draft',
    version: 1,
    sourceRoot: '/music',
    album: { title: 'A', artists: [{ name: 'B' }] },
    media: [{ disc: 1, tracks: [{ track: 1, title: 'T', audioPath: '1.mpc' }] }],
    artwork: [],
    booklet: [],
    lyrics: [],
    extras: [],
  };
}

function makeApi(calls: unknown[]) {
  const invoke: InvokeFn = async (cmd, args) => {
    calls.push({ cmd, args });
    return {};
  };
  const plugins = {
    pickDirectory: vi.fn(async () => '/picked'),
    pickImageFile: vi.fn(async () => null),
    pickLyricsFile: vi.fn(async () => null),
    pickOutputDirectory: vi.fn(async () => null),
    revealInFinder: vi.fn(async () => undefined),
  };
  return { api: new AuthorApi(invoke, plugins), plugins };
}

describe('AuthorApi command surface', () => {
  it('inspects an album by path', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.inspectAlbum('/album');
    expect(calls[0]).toEqual({ cmd: 'inspect_album', args: { path: '/album' } });
  });

  it('validates a draft as serialized JSON', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    const d = draft();
    await api.validateDraft(d);
    expect(calls[0]).toEqual({
      cmd: 'validate_draft',
      args: { draftJson: JSON.stringify(d) },
    });
  });

  it('identifies by mbid (nulls for unused options)', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.identifyDraft(draft(), { mbid: 'aaaa' });
    expect(calls[0]).toEqual({
      cmd: 'identify_draft',
      args: { draftJson: expect.any(String), mbid: 'aaaa', barcode: null, mbJson: null },
    });
  });

  it('passes mbJson through for an offline candidate apply', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.identifyDraft(draft(), { mbJson: '{"id":"x"}' });
    expect(calls[0]).toMatchObject({ cmd: 'identify_draft', args: { mbJson: '{"id":"x"}' } });
  });

  it('creates a package at the chosen output', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.createPackage(draft(), '/out.mpack');
    expect(calls[0]).toMatchObject({
      cmd: 'create_package',
      args: { outputDir: '/out.mpack' },
    });
  });

  it('passes replace and sync-tags for an in-place package save', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.createPackage(draft(), '/existing.mpack', { replace: true, syncTags: true });
    expect(calls[0]).toMatchObject({
      cmd: 'create_package',
      args: { outputDir: '/existing.mpack', replace: true, syncTags: true },
    });
  });

  it('verifies a package and reads an image', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.verifyPackage('/out.mpack');
    await api.readImage('/art.png');
    expect(calls[0]).toEqual({ cmd: 'verify_package', args: { path: '/out.mpack' } });
    expect(calls[1]).toEqual({ cmd: 'read_image', args: { path: '/art.png' } });
  });

  it('creates a single-file .mpak from a draft', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    const d = draft();
    await api.createMpak(d, '/out/album.mpak');
    expect(calls[0]).toEqual({
      cmd: 'create_mpak',
      args: { draftJson: JSON.stringify(d), outputMpak: '/out/album.mpak' },
    });
  });

  it('packs an existing .mpack directory into a .mpak', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.packPackage('/existing.mpack', '/out/album.mpak');
    expect(calls[0]).toEqual({
      cmd: 'pack_package',
      args: { inputDir: '/existing.mpack', outputMpak: '/out/album.mpak' },
    });
  });

  it('runs and cancels sonic analysis', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    const d = draft();
    await api.sonicAnalyze(d);
    await api.sonicCancel();
    expect(calls[0]).toEqual({
      cmd: 'sonic_analyze',
      args: { draftJson: JSON.stringify(d) },
    });
    expect(calls[1]).toEqual({ cmd: 'sonic_cancel', args: {} });
  });

  it('runs and cancels waveform generation', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    const d = draft();
    await api.waveformAnalyze(d);
    await api.waveformCancel();
    expect(calls[0]).toEqual({
      cmd: 'waveform_analyze',
      args: { draftJson: JSON.stringify(d) },
    });
    expect(calls[1]).toEqual({ cmd: 'waveform_cancel', args: {} });
  });

  it('encodes tracks, cancels and cleans staging', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    const d = draft();
    await api.encodeTracks(d, '6.0');
    await api.encodeCancel();
    await api.cleanupStaging('/tmp/stage');
    expect(calls[0]).toEqual({
      cmd: 'encode_tracks',
      args: { draftJson: JSON.stringify(d), quality: '6.0' },
    });
    expect(calls[1]).toEqual({ cmd: 'encode_cancel', args: {} });
    expect(calls[2]).toEqual({ cmd: 'cleanup_staging', args: { path: '/tmp/stage' } });
  });

  it('reports the sonic model status', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.sonicModelStatus();
    expect(calls[0]).toEqual({ cmd: 'sonic_model_status', args: {} });
  });

  it('fetches the backend capability handshake', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.backendInfo();
    expect(calls[0]).toEqual({ cmd: 'backend_info', args: {} });
  });

  it('delegates dialogs and reveal to the plugin facade', async () => {
    const calls: unknown[] = [];
    const { api, plugins } = makeApi(calls);
    expect(await api.pickDirectory()).toBe('/picked');
    expect(plugins.pickDirectory).toHaveBeenCalledOnce();
    await api.revealInFinder('/x');
    expect(plugins.revealInFinder).toHaveBeenCalledWith('/x');
  });

  it('probes a lyric file by path (R3.5)', async () => {
    const calls: unknown[] = [];
    const { api } = makeApi(calls);
    await api.lyricsProbe('/music/lyrics/a.lrc');
    expect(calls[0]).toEqual({ cmd: 'lyrics_probe', args: { path: '/music/lyrics/a.lrc' } });
  });

  it('delegates lyric file picking to the plugin facade (R3.5)', async () => {
    const calls: unknown[] = [];
    const { api, plugins } = makeApi(calls);
    (plugins.pickLyricsFile as unknown as { mockResolvedValue(v: string | null): void }).mockResolvedValue(
      '/music/lyrics/a.lrc',
    );
    await expect(api.pickLyricsFile()).resolves.toBe('/music/lyrics/a.lrc');
    expect(plugins.pickLyricsFile).toHaveBeenCalledOnce();
  });
});
