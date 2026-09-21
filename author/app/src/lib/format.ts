// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Small pure formatting helpers (shared style with the web client's
// lib/format.ts — no date/time library).

export function fmtTime(seconds?: number): string {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) {
    return '–';
  }
  const total = Math.round(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

export function yearOf(date?: string): string {
  if (!date) return '';
  return date.slice(0, 4);
}

export function formatDate(date?: string): string {
  if (!date) return '';
  const [y, m, d] = date.split('-');
  if (!y) return '';
  if (!m) return y;
  if (!d) {
    const month = new Date(Number(y), Number(m) - 1, 1).toLocaleString('en', {
      month: 'long',
    });
    return `${month} ${y}`;
  }
  const month = new Date(Number(y), Number(m) - 1, 1).toLocaleString('en', {
    month: 'short',
  });
  return `${d} ${month} ${y}`;
}

export function artistLine(artists: { name: string; role?: string }[]): string {
  return artists
    .map((a) => (a.role && a.role !== 'main' ? `${a.name} (${a.role})` : a.name))
    .join(', ');
}

export function codecLabel(codec?: string, version?: number): string {
  if (!codec) return '?';
  if (codec.startsWith('musepack')) return 'MPC';
  if (codec === 'flac') return 'FLAC';
  if (codec === 'wav') return 'WAV';
  if (codec === 'ogg') return 'OGG';
  return codec.toUpperCase();
}

/** Replaces filesystem-invalid characters with '-' without otherwise
 * rewriting the string (visible album metadata is left untouched). */
export function sanitizeFilename(name: string): string {
  return name.replace(/[\/\\:*?"<>|]/g, '-').replace(/[ \t]+/g, ' ').trim();
}

/** A sensible default `.mpack` package name from normalized album metadata.
 * The manifest, not the filename, remains the authoritative identity. */
export function defaultPackageName(d: {
  album: { title: string; artists: { name: string; role?: string }[] };
}): string {
  const artist = artistLine(d.album.artists) || 'Unknown Artist';
  const title = d.album.title.trim() || 'Untitled';
  return sanitizeFilename(`${artist} - ${title}`);
}

/** Whether a track still needs encoding (a lossless FLAC/WAV source). */
export function needsEncoding(track: {
  codec?: string;
  audioPath: string;
}): boolean {
  const c = track.codec ?? '';
  if (c === 'flac' || c === 'wav') return true;
  if (c.startsWith('musepack') || c === 'mpc') return false;
  return /\.(flac|wav)$/i.test(track.audioPath);
}
