// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Shared draft-validation runner used by both the status bar button and the
// album view's auto-validation effect, so there is exactly one place that
// invokes `validate-draft` and stores the verdict.

import { api, draft } from './bootstrap';
import { validating, validation } from './authoring-state';
import type { ValidationResult } from './types';

export async function runValidation(): Promise<void> {
  const d = draft.get();
  if (!d) return;
  validating.set(true);
  try {
    validation.set(await api.validateDraft(d));
  } catch (e) {
    validation.set({
      ok: false,
      errors: [e instanceof Error ? e.message : 'Validation failed'],
      warnings: [],
    });
  } finally {
    validating.set(false);
  }
}
