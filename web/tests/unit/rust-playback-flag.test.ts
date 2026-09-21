// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// R4.4: playback backend selection. Rust is the decode backend for every
// codec it supports — online (`http-range`) and offline (OPFS `local-file`)
// alike. The frozen Emscripten Musepack decoder is reachable only through the
// test-only oracle hook (`setOracleDecoder`), never from product config.

import { afterEach, describe, expect, it } from 'vitest';
import {
  chooseBackend,
  createWebEngine,
  oracleDecoderEnabled,
  setOracleDecoder,
} from '../../app/src/lib/playback/controller';
import { RustPlaybackEngine } from '../../app/src/lib/playback/rust-playback-engine';
import type { PlaybackItem } from '../../player-core/src/types';

function item(
  codec: string,
  mimeType = 'audio/musepack',
  kind: 'http-range' | 'local-file' = 'http-range',
): PlaybackItem {
  return {
    id: 't1',
    trackId: 1,
    source: { kind, url: kind === 'local-file' ? 't.7.audio' : '/a.mpc', byteSize: 100 },
    title: 'T',
    artist: 'A',
    albumTitle: 'AL',
    codec,
    mimeType,
  } as unknown as PlaybackItem;
}

afterEach(() => setOracleDecoder(null));

describe('playback backend selection (R4.4)', () => {
  it('routes every Rust-decodable codec to Rust, online and offline', () => {
    expect(oracleDecoderEnabled()).toBe(false);
    for (const kind of ['http-range', 'local-file'] as const) {
      expect(chooseBackend(item('musepack-sv8', 'audio/musepack', kind))).toBe('rust');
      expect(chooseBackend(item('musepack-sv7', 'audio/musepack', kind))).toBe('rust');
      expect(chooseBackend(item('flac', 'audio/flac', kind))).toBe('rust');
      expect(chooseBackend(item('wav', 'audio/wav', kind))).toBe('rust');
    }
  });

  it('offline (OPFS) Musepack no longer falls back to the frozen decoder', () => {
    // The historical exception (local-file -> legacy demand engine) is gone:
    // the same Rust decoder serves committed OPFS bytes through the range
    // callback.
    const local = item('musepack-sv8', 'audio/musepack', 'local-file');
    expect(chooseBackend(local)).toBe('rust');
  });

  it('the frozen decoder is reachable only through the test oracle hook', () => {
    setOracleDecoder(true);
    expect(oracleDecoderEnabled()).toBe(true);
    expect(chooseBackend(item('musepack-sv8'))).toBe('musepack');
    // The oracle lane restores the historical routing: FLAC/WAV fall to the
    // browser-native backend (which has no DOM probe under Node -> unsupported).
    expect(() => chooseBackend(item('flac', 'audio/flac'))).toThrow(/not supported/i);
  });

  it('leaves codecs Rust does not decode on their native family', () => {
    // No DOM in Node, so a non-Rust native codec resolves to "unsupported"
    // through the browser capability probe.
    expect(() => chooseBackend(item('mp3', 'audio/mpeg'))).toThrow(/not supported/i);
  });

  it('createWebEngine("rust") constructs the Rust adapter', () => {
    const engine = createWebEngine('rust', {
      primed: () => undefined,
      buffering: () => undefined,
      eos: () => undefined,
      error: () => undefined,
      tick: () => undefined,
    });
    expect(engine).toBeInstanceOf(RustPlaybackEngine);
    expect(engine.capabilities.crossfade).toBe(true);
  });
});
