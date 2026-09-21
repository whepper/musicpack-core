// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Status-chip vocabulary, mapped strictly to the values the server actually
// emits (server/src/scanner.c::determine_status, VISIBLE filter in api.c):
//   packageStatus: 'valid' | 'warning' | 'checksum-failed' | 'conflict'
//   verifyStatus:  'unverified' | 'valid' | 'warning' | 'checksum-failed'
// Unknown values render neutrally with their raw string — never fake-green.

export type ChipTone = 'ok' | 'warn' | 'bad' | 'muted';

export interface Chip {
  label: string;
  tone: ChipTone;
  /** Leading glyph ("✓"); only for states the server actually asserts. */
  mark?: string;
}

export function packageStatusChip(value?: string): Chip {
  switch (value) {
    case 'valid':
      return { label: 'Valid', tone: 'ok' };
    case 'warning':
      return { label: 'Warning', tone: 'warn' };
    case 'checksum-failed':
      return { label: 'Checksum failed', tone: 'bad' };
    case 'conflict':
      return { label: 'Conflict', tone: 'bad' };
    case undefined:
    case '':
      return { label: 'Unknown', tone: 'muted' };
    default:
      return { label: value, tone: 'muted' };
  }
}

export function verifyStatusChip(value?: string): Chip {
  switch (value) {
    case 'valid':
      return { label: 'Verified', tone: 'ok', mark: '✓' };
    case 'warning':
      return { label: 'Verification warning', tone: 'warn' };
    case 'unverified':
      return { label: 'Unverified', tone: 'muted' };
    case 'checksum-failed':
      return { label: 'Checksum failed', tone: 'bad' };
    case undefined:
    case '':
      return { label: 'Unknown', tone: 'muted' };
    default:
      return { label: value, tone: 'muted' };
  }
}

/** MusicBrainz identity confidence (release-level fields on the API). */
export function identityChip(source?: string, confidence?: string): Chip | null {
  if (!confidence) return null;
  switch (confidence) {
    case 'exact':
      return { label: source ? `Exact match · ${source}` : 'Exact match', tone: 'ok', mark: '✓' };
    case 'confirmed':
      return { label: source ? `Confirmed · ${source}` : 'Confirmed', tone: 'ok' };
    case 'probable':
      return { label: 'Probable match', tone: 'muted' };
    case 'none':
      return { label: 'Unidentified', tone: 'muted' };
    default:
      return { label: confidence, tone: 'muted' };
  }
}

/** Client-side offline package lifecycle (offline/availability.ts states). */
export function offlineStateChip(state?: string): Chip | null {
  switch (state) {
    case 'installed':
      return { label: 'Offline ready', tone: 'ok', mark: '⤓' };
    case 'stale':
      return { label: 'Update available', tone: 'warn' };
    case 'damaged':
      return { label: 'Needs repair', tone: 'bad' };
    case 'downloading':
      return { label: 'Downloading', tone: 'muted' };
    case 'failed':
      return { label: 'Download failed', tone: 'bad' };
    default:
      return null;
  }
}
