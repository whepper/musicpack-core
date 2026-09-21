// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Phase 4: the representation-selection policy. resolveAudio is pure and
// total; playability is injected so these tests run in Node without any
// browser API. The scan block pins the module's architecture (imports +
// no ambient globals) in the style of purity-gate.test.ts.

import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  LOSSLESS_CODECS,
  acceptAll,
  parseAudioPreference,
  resolveAudio,
} from '../../app/src/lib/state/representation-selection';
import type { RepresentationRef, Track } from '../../app/src/lib/api/types';

function rep(
  id: number,
  codec: string,
  mime = `audio/${codec}`,
  over: Partial<RepresentationRef> = {},
): RepresentationRef {
  return {
    id,
    size: 500 + id,
    url: `/api/v1/tracks/14/representations/${id}/audio`,
    codec: { codec, mimeType: mime },
    ...over,
  };
}

function track(over: Partial<Track> = {}): Track {
  return {
    id: 14,
    number: 2,
    title: 'T4',
    artists: [],
    codec: { codec: 'musepack-sv8', mimeType: 'audio/musepack' },
    audio: { id: 14, size: 1014, url: '/api/v1/tracks/14/audio' },
    ...over,
  };
}

const all = acceptAll;
const none = () => false;
/** Accepts only the given codecs (lowercased). */
const only =
  (...codecs: string[]) =>
  (c: { codec?: string }) =>
    codecs.includes((c.codec ?? '').toLowerCase());

describe('resolveAudio — default behavior', () => {
  it('undefined preference → primary', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, undefined, all)).toEqual({ representation: null });
  });

  it('{default} preference → primary even when alternates exist', () => {
    const t = track({ representations: [rep(77, 'flac'), rep(78, 'flac')] });
    expect(resolveAudio(t, { mode: 'default' }, all)).toEqual({ representation: null });
  });

  it('tracks without representations always resolve to primary', () => {
    const plain = track();
    expect(resolveAudio(plain, { mode: 'lossless' }, all)).toEqual({ representation: null });
    expect(resolveAudio(plain, { mode: 'codec', codec: 'flac' }, all)).toEqual({
      representation: null,
    });
    expect(resolveAudio(plain, undefined, none)).toEqual({ representation: null });
  });

  it.each([
    ['null', null],
    ['garbage object', { mode: 'shiny' }],
    ['representation with non-numeric id', { mode: 'representation', id: '77' }],
    ['codec with empty string', { mode: 'codec', codec: '' }],
    ['bare string', 'lossless'],
  ])('malformed preference %s falls back to primary', (_name, bad) => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, bad as never, all)).toEqual({ representation: null });
  });
});

describe('resolveAudio — explicit representation', () => {
  it('requested representation exists and is playable → that one', () => {
    const t = track({ representations: [rep(76, 'flac'), rep(77, 'wav')] });
    expect(resolveAudio(t, { mode: 'representation', id: 77 }, all)).toEqual({
      representation: rep(77, 'wav'),
    });
  });

  it('requested id absent → fallback to primary (no throw)', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, { mode: 'representation', id: 99 }, all)).toEqual({
      representation: null,
    });
  });

  it('requested id present but unplayable → fallback to primary', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, { mode: 'representation', id: 77 }, only('musepack-sv8'))).toEqual({
      representation: null,
    });
  });
});

describe('resolveAudio — codec and lossless preferences', () => {
  it('codec preference picks first playable match (case-insensitive)', () => {
    const t = track({ representations: [rep(75, 'mp3'), rep(76, 'FLAC'), rep(77, 'flac')] });
    const r = resolveAudio(t, { mode: 'codec', codec: 'flac' }, all);
    expect(r.representation?.id).toBe(76);
    expect(r.representation?.codec.codec).toBe('FLAC');
  });

  it('codec preference with no match → fallback to primary', () => {
    const t = track({ representations: [rep(75, 'mp3')] });
    expect(resolveAudio(t, { mode: 'codec', codec: 'ogg' }, all)).toEqual({
      representation: null,
    });
  });

  it('codec preference skips unplayable matches to later playable ones', () => {
    // canPlay rejects id 75's (distinct) mime; the next flac wins.
    const t = track({
      representations: [rep(75, 'flac', 'audio/x-broken'), rep(76, 'flac', 'audio/flac')],
    });
    const r = resolveAudio(t, { mode: 'codec', codec: 'flac' }, (c) => c.mimeType !== 'audio/x-broken');
    expect(r.representation?.id).toBe(76);
  });

  it('lossless picks the FIRST lossless candidate in manifest order', () => {
    const t = track({ representations: [rep(73, 'mp3'), rep(74, 'aiff'), rep(75, 'flac')] });
    expect(resolveAudio(t, { mode: 'lossless' }, all).representation?.id).toBe(74);
  });

  it('lossless ignores lossy candidates entirely', () => {
    const t = track({ representations: [rep(73, 'mp3'), rep(74, 'opus')] });
    expect(resolveAudio(t, { mode: 'lossless' }, all)).toEqual({ representation: null });
  });

  it('lossless set is exactly the agreed closed classification', () => {
    expect([...LOSSLESS_CODECS].sort()).toEqual(['aiff', 'flac', 'wav']);
  });
});

describe('resolveAudio — availability fallback', () => {
  it('unplayable primary + playable alternate → alternate rescues', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, undefined, only('musepack-x', 'flac')).representation?.id).toBe(77);
  });

  it('primary preferred whenever it IS playable (never displaced)', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, undefined, only('musepack-sv8'))).toEqual({ representation: null });
  });

  it('nothing playable at all → primary returned (today\'s failure path)', () => {
    const t = track({ representations: [rep(77, 'flac')] });
    expect(resolveAudio(t, { mode: 'lossless' }, none)).toEqual({ representation: null });
  });

  it('rescue walks manifest order past unplayable alternates', () => {
    const t = track({ representations: [rep(76, 'wav'), rep(77, 'flac')] });
    const r = resolveAudio(t, undefined, only('flac'));
    expect(r.representation?.id).toBe(77);
  });
});

describe('resolveAudio — determinism and purity', () => {
  it('repeated calls are identical and inputs are never mutated', () => {
    const t = track({ representations: [rep(76, 'wav'), rep(77, 'flac')] });
    const before = JSON.stringify(t);
    for (const pref of [
      undefined,
      { mode: 'default' } as const,
      { mode: 'lossless' } as const,
      { mode: 'codec', codec: 'FLAC' } as const,
      { mode: 'representation', id: 77 } as const,
    ]) {
      const a = resolveAudio(t, pref, all);
      const b = resolveAudio(t, pref, all);
      expect(a).toEqual(b);
    }
    expect(JSON.stringify(t)).toBe(before);
  });
});

describe('parseAudioPreference', () => {
  it('round-trips every valid mode', () => {
    expect(parseAudioPreference({ mode: 'default' })).toEqual({ mode: 'default' });
    expect(parseAudioPreference({ mode: 'lossless' })).toEqual({ mode: 'lossless' });
    expect(parseAudioPreference({ mode: 'representation', id: 7 })).toEqual({
      mode: 'representation',
      id: 7,
    });
    expect(parseAudioPreference({ mode: 'codec', codec: 'flac' })).toEqual({
      mode: 'codec',
      codec: 'flac',
    });
  });

  it('rejects junk as null', () => {
    expect(parseAudioPreference(undefined)).toBeNull();
    expect(parseAudioPreference('x')).toBeNull();
    expect(parseAudioPreference(42)).toBeNull();
    expect(parseAudioPreference({})).toBeNull();
    expect(parseAudioPreference({ mode: 'representation', id: NaN })).toBeNull();
  });
});

describe('architecture gate — selection stays at the web/domain boundary', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const policyPath = join(here, '../../app/src/lib/state/representation-selection.ts');
  const coreDir = join(here, '../../player-core/src');

  it('policy imports nothing but its api/types sibling (pure module)', () => {
    const src = readFileSync(policyPath, 'utf8');
    const imports = [...src.matchAll(/^import\s[^']*'([^']*)'/gm)].map((m) => m[1]);
    expect(imports.sort()).toEqual(['../api/types']);
  });

  it('policy touches no DOM/browser globals', () => {
    const src = readFileSync(policyPath, 'utf8')
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/^\s*\/\/.*$/gm, '');
    expect(src.match(/\b(window|document|localStorage|navigator|fetch|AudioContext)\b/)).toBeNull();
  });

  it('player-core never reaches into the web state layer', () => {
    // Dependency direction: libmusicpack-style — the core cannot know about
    // representations or any host store.
    const files: string[] = [];
    const walk = (dir: string): void => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name);
        if (statSync(p).isDirectory()) walk(p);
        else if (name.endsWith('.ts')) files.push(p);
      }
    };
    walk(coreDir);
    for (const f of files) {
      const src = readFileSync(f, 'utf8')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .replace(/^\s*\/\/.*$/gm, '');
      expect(src.match(/lib\/state|api\/types/), f).toBeNull();
    }
  });
});
