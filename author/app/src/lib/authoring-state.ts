// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Shared authoring-session state (validation result, create flow, encode
// staging) shared by the album view and the fixed footer/status bar. Plain
// TS + writable.

import { writable } from './store';
import type { CreateResult, ValidationResult } from './types';

// ---- workflow stages ------------------------------------------------------
// The album is authored one stage at a time; the stepper in WorkflowNav and
// the stage containers in AlbumAuthoring both key off this single value.

export type StageId =
  | 'identity'
  | 'release'
  | 'tracks'
  | 'artwork'
  | 'encode'
  | 'sonic'
  | 'waveform'
  | 'validate';

export const STAGES: readonly { id: StageId; label: string }[] = [
  { id: 'identity', label: 'Identity' },
  { id: 'release', label: 'Release' },
  { id: 'tracks', label: 'Tracks' },
  { id: 'artwork', label: 'Artwork' },
  { id: 'encode', label: 'Encode' },
  { id: 'sonic', label: 'Sonic' },
  { id: 'waveform', label: 'Waveform' },
  { id: 'validate', label: 'Validate' },
];

export const activeStage = writable<StageId>('identity');

export const validation = writable<ValidationResult | null>(null);
export const validating = writable<boolean>(false);
/** True when the draft changed after the last successful validation, so the
 * UI can mark a previous green result as outdated instead of silently stale. */
export const validationDirty = writable<boolean>(false);
/** The long task currently running (encode | sonic | waveform | null), for
 * the global busy indicator in the status bar. */
export const activeTask = writable<string | null>(null);
export const createOpen = writable<boolean>(false);
export const createResult = writable<CreateResult | null>(null);

/** The encode staging directory for the current draft (set after a successful
 * encode, cleared after the package build or when a new album is loaded). */
export const encodeStaging = writable<string | null>(null);

export function setEncodeStaging(dir: string | null): void {
  encodeStaging.set(dir);
}

export function invalidateValidation(): void {
  validation.set(null);
  validationDirty.set(false);
}
