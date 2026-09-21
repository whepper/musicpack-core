import { describe, it, expect } from 'vitest';
import {
  fmtTime,
  yearOf,
  formatDate,
  artistLine,
  codecLabel,
  sanitizeFilename,
  defaultPackageName,
  needsEncoding,
} from '../../app/src/lib/format';

describe('format helpers', () => {
  it('formats durations as M:SS', () => {
    expect(fmtTime(252.3)).toBe('4:12');
    expect(fmtTime(61)).toBe('1:01');
    expect(fmtTime(0)).toBe('0:00');
    expect(fmtTime(-1)).toBe('–');
    expect(fmtTime(undefined)).toBe('–');
  });

  it('formats dates and years', () => {
    expect(yearOf('1986-06-16')).toBe('1986');
    expect(formatDate('1986-06-16')).toBe('16 Jun 1986');
    expect(formatDate('1986')).toBe('1986');
    expect(formatDate('')).toBe('');
  });

  it('joins artist credits', () => {
    expect(artistLine([{ name: 'A', role: 'main' }, { name: 'B', role: 'featuring' }])).toBe(
      'A, B (featuring)',
    );
  });

  it('labels codecs', () => {
    expect(codecLabel('musepack-sv8', 8)).toBe('MPC');
    expect(codecLabel('flac')).toBe('FLAC');
    expect(codecLabel()).toBe('?');
  });

  it('sanitizes filesystem-invalid characters only', () => {
    expect(sanitizeFilename('A/B:C*D?E"F<G>H|I')).toBe('A-B-C-D-E-F-G-H-I');
    expect(sanitizeFilename('  Neon   Skyline  ')).toBe('Neon Skyline');
  });

  it('builds a sensible default package name from metadata', () => {
    expect(
      defaultPackageName({ album: { title: 'Neon Skyline', artists: [{ name: 'The Signal', role: 'main' }] } }),
    ).toBe('The Signal - Neon Skyline');
    expect(
      defaultPackageName({ album: { title: 'A/B', artists: [{ name: 'X' }] } }),
    ).toBe('X - A-B');
  });

  it('detects tracks that still need encoding', () => {
    expect(needsEncoding({ codec: 'flac', audioPath: '1.flac' })).toBe(true);
    expect(needsEncoding({ codec: 'wav', audioPath: '1.wav' })).toBe(true);
    expect(needsEncoding({ codec: undefined, audioPath: '2.flac' })).toBe(true);
    expect(needsEncoding({ codec: 'musepack-sv8', audioPath: '1.mpc' })).toBe(false);
    expect(needsEncoding({ codec: undefined, audioPath: '1.mpc' })).toBe(false);
    expect(needsEncoding({ codec: 'ogg', audioPath: '1.ogg' })).toBe(false);
  });
});
