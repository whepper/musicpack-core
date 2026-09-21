// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Small formatting helpers for the record-shelf UI.

import type { Track } from './api/types';

export function fmtTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
  return `${m}:${String(sec).padStart(2, '0')}`;
}

/** "1986" or "1986-06-16" -> "1986" (short year, honoring partial dates). */
export function yearOf(date?: string): string {
  if (!date) return '';
  return date.slice(0, 4);
}

/** "1986-06-16" -> "16 Jun 1986". */
export function formatDate(date?: string): string {
  if (!date) return '';
  const [y, m, d] = date.split('-');
  if (!y) return date;
  const month = new Date(Number(y), (Number(m) || 1) - 1, 1).toLocaleString('en', { month: 'short' });
  return d ? `${Number(d)} ${month} ${y}` : `${month} ${y}`;
}

/** Country code -> display ("XE" -> "Europe"). */
export function countryName(code?: string): string {
  if (!code) return '';
  const map: Record<string, string> = {
    XE: 'Europe',
    XW: 'Worldwide',
    XN: 'Northern Europe',
    US: 'United States',
    UK: 'United Kingdom',
    GB: 'United Kingdom',
    DE: 'Germany',
    FR: 'France',
    JP: 'Japan',
    NL: 'Netherlands',
  };
  return map[code] ?? code;
}

export function mediumLabel(format?: string): string {
  if (!format) return 'Digital';
  const lower = format.toLowerCase();
  if (lower.includes('vinyl') || lower.includes('lp')) return 'Vinyl';
  if (lower.includes('sacd')) return 'SACD';
  if (lower.includes('dvd')) return 'DVD';
  if (lower.includes('blu-ray') || lower.includes('blu ray')) return 'Blu-ray';
  if (lower.includes('cd')) return 'CD';
  if (lower.includes('digital') || lower.includes('download')) return 'Digital';
  return format;
}

/** Compact collector line for the shelf: "1986 · CD · 2 versions". */
export function collectorLine(opts: {
  year?: string;
  media?: string[];
  releaseCount?: number;
}): string {
  const parts: string[] = [];
  if (opts.year) parts.push(opts.year);
  if (opts.media?.length) parts.push(opts.media.map(mediumLabel).join('/'));
  if (opts.releaseCount !== undefined && opts.releaseCount > 1) {
    parts.push(`${opts.releaseCount} version${opts.releaseCount > 1 ? 's' : ''}`);
  }
  return parts.join(' · ');
}

/** Short readable codec label for the track row. */
export function codecLabel(codec?: string): string {
  if (!codec) return '';
  const lower = codec.toLowerCase();
  if (lower === 'musepack-sv8' || lower === 'musepack-sv7' || lower === 'musepack')
    return 'MPC';
  if (lower === 'flac') return 'FLAC';
  if (lower === 'wav') return 'WAV';
  if (lower === 'aiff' || lower === 'aif') return 'AIFF';
  if (lower === 'mp3') return 'MP3';
  if (lower === 'aac' || lower === 'm4a') return 'AAC';
  if (lower === 'ogg' || lower === 'vorbis') return 'OGG';
  if (lower === 'opus') return 'Opus';
  return codec.toUpperCase();
}

/** Full display name for a codec — the voice used by tiles, cards and
 *  key/value rows ("Musepack", "FLAC"). `codecLabel` stays the compact
 *  chip voice ("MPC") for dense tables, meta lines and the player bar.
 *  Pick by context, never hand-roll the mapping at call sites. */
export function codecName(codec?: string): string {
  if (!codec) return '';
  if (isMusepackFamily(codec)) return 'Musepack';
  return codecLabel(codec);
}

/** Byte size -> compact display ("412 kB", "38.4 MB", "1.2 GB"). Null or
 *  non-positive input yields '' so callers can omit the segment entirely. */
export function formatBytes(bytes?: number | null): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes <= 0) {
    return '';
  }
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${Math.round(kb)} kB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb >= 100 ? Math.round(mb) : Math.round(mb * 10) / 10} MB`;
  const gb = mb / 1024;
  return `${Math.round(gb * 100) / 100} GB`;
}

function isMusepackFamily(codec: string): boolean {
  const c = codec.toLowerCase();
  return c === 'musepack' || c === 'musepack-sv7' || c === 'musepack-sv8';
}

/** Format line for what is actually sounding: the playback codec hint when
 *  present, otherwise the primary track codec; rate/channels come from the
 *  selected representation when one is playing. Same facts as the player
 *  bar has always shown — shared by the bar, mobile player and the
 *  Now Playing page. */
export function playingFormatLine(item: {
  codec?: string;
  representationId?: number;
  track: Track;
}): string {
  const rep = (item.track.representations ?? []).find(
    (r) => r.id === item.representationId,
  );
  return qualityLine({
    codec: item.codec ?? item.track.codec.codec,
    sampleRate: rep?.codec.sampleRate ?? item.track.codec.sampleRate,
    channels: rep?.codec.channels ?? item.track.codec.channels,
    label: rep?.label,
  });
}

/** Human-readable quality line for one audio representation, built ONLY
 *  from metadata the API actually carries (codec family, sample rate,
 *  channels, manifest label). Bit depth / encoder quality are not probed
 *  anywhere in the stack and are deliberately never invented here.
 *  Example outputs: "MPC", "FLAC · 48 kHz", "FLAC · 44.1 kHz · stereo". */
export function qualityLine(
  opts: { codec?: string; sampleRate?: number; channels?: number; label?: string } | undefined,
): string {
  if (!opts) return '';
  const base = opts.label?.trim() || codecLabel(opts.codec);
  if (!base) return '';
  const parts: string[] = [base];
  // Musepack SV8 is fixed-rate/fixed-schedule; rate/channel detail adds
  // nothing there, while lossless formats are exactly where it matters.
  if (
    opts.sampleRate && opts.sampleRate > 0 && opts.codec &&
    !isMusepackFamily(opts.codec)
  ) {
    parts.push(`${(opts.sampleRate / 1000).toString().replace(/\.0$/, '')} kHz`);
    if (opts.channels === 2) parts.push('stereo');
    else if (opts.channels && opts.channels > 2) parts.push(`${opts.channels} ch`);
  }
  return parts.join(' · ');
}
