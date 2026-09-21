import { describe, it, expect } from 'vitest';
import { parseRoute } from '../../app/src/lib/router';
import { fmtTime, yearOf, formatDate, countryName, mediumLabel, collectorLine, formatBytes, qualityLine, codecName } from '../../app/src/lib/format';

describe('router', () => {
  it('parses all route shapes', () => {
    expect(parseRoute('/')?.name).toBe('albums');
    expect(parseRoute('/albums')?.name).toBe('albums');
    expect(parseRoute('/albums/42')).toMatchObject({ name: 'album', params: { id: '42' } });
    expect(parseRoute('/albums/42?release=7')?.query.get('release')).toBe('7');
    expect(parseRoute('/artists')?.name).toBe('artists');
    expect(parseRoute('/artists/3')).toMatchObject({ name: 'artist', params: { id: '3' } });
    expect(parseRoute('/queue')?.name).toBe('queue');
  });

  it('rejects non-numeric ids and unknown routes', () => {
    expect(parseRoute('/albums/abc')?.name).toBe('notfound');
    expect(parseRoute('/admin')?.name).toBe('notfound');
  });
});

describe('format', () => {
  it('formats times', () => {
    expect(fmtTime(0)).toBe('0:00');
    expect(fmtTime(65)).toBe('1:05');
    expect(fmtTime(3675)).toBe('1:01:15');
    expect(fmtTime(-3)).toBe('0:00');
  });

  it('extracts years from partial dates', () => {
    expect(yearOf('1986-06-16')).toBe('1986');
    expect(yearOf('1986')).toBe('1986');
    expect(yearOf(undefined)).toBe('');
  });

  it('formats dates and countries', () => {
    expect(formatDate('1986-06-16')).toBe('16 Jun 1986');
    expect(countryName('XE')).toBe('Europe');
    expect(countryName('XX')).toBe('XX');
  });

  it('maps medium formats', () => {
    expect(mediumLabel('CD')).toBe('CD');
    expect(mediumLabel('SACD')).toBe('SACD');
    expect(mediumLabel('Digital')).toBe('Digital');
    expect(mediumLabel('Vinyl, LP')).toBe('Vinyl');
  });

  it('builds collector lines', () => {
    expect(collectorLine({ year: '1990', media: ['CD'], releaseCount: 3 })).toBe('1990 · CD · 3 versions');
    expect(collectorLine({ year: '1986' })).toBe('1986');
    expect(collectorLine({})).toBe('');
  });

  it('formats byte sizes compactly', () => {
    expect(formatBytes(undefined)).toBe('');
    expect(formatBytes(null)).toBe('');
    expect(formatBytes(0)).toBe('');
    expect(formatBytes(-5)).toBe('');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024)).toBe('1 kB');
    expect(formatBytes(422_000)).toBe('412 kB');
    expect(formatBytes(40_265_318)).toBe('38.4 MB');
    expect(formatBytes(140_000_000)).toBe('134 MB'); // >=100 MB rounds to whole
    expect(formatBytes(1_250_000_000)).toBe('1.16 GB');
  });

  it('builds quality lines from existing metadata only', () => {
    // Musepack: family label only (rate/channels add nothing there).
    expect(qualityLine({ codec: 'musepack-sv8' })).toBe('MPC');
    expect(
      qualityLine({ codec: 'musepack-sv8', sampleRate: 44100, channels: 2 }),
    ).toBe('MPC');
    // Lossless with probed facts.
    expect(qualityLine({ codec: 'flac', sampleRate: 48000 })).toBe('FLAC · 48 kHz');
    expect(
      qualityLine({ codec: 'flac', sampleRate: 44100, channels: 2 }),
    ).toBe('FLAC · 44.1 kHz · stereo');
    expect(
      qualityLine({ codec: 'wav', sampleRate: 96000, channels: 6 }),
    ).toBe('WAV · 96 kHz · 6 ch');
    // Manifest label wins over the derived codec name.
    expect(qualityLine({ label: 'FLAC 24/96', codec: 'flac' })).toBe('FLAC 24/96');
    expect(qualityLine({ label: '  ', codec: 'flac' })).toBe('FLAC');
    // Absent/unknown metadata degrades honestly; never invents detail.
    expect(qualityLine({ codec: 'flac' })).toBe('FLAC');
    expect(qualityLine(undefined)).toBe('');
  });

  it('separates the editorial codec name from the compact label', () => {
    // Editorial voice: the full family name.
    expect(codecName('musepack-sv8')).toBe('Musepack');
    expect(codecName('musepack-sv7')).toBe('Musepack');
    expect(codecName('Musepack')).toBe('Musepack');
    // Everything else falls through to the compact label unchanged.
    expect(codecName('flac')).toBe('FLAC');
    expect(codecName('mp3')).toBe('MP3');
    expect(codecName('unknown-xyz')).toBe('UNKNOWN-XYZ');
    expect(codecName(undefined)).toBe('');
  });
});
