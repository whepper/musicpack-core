// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Purity-law gate (plan §A.1 / DoD). player-core must stay platform-
// independent: only relative imports (law 1) and no ambient browser/Node
// globals (law 2). This suite FAILS the build if a violation is introduced.

import { describe, expect, it } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const CORE_DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  '../../player-core/src',
);

function coreFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...coreFiles(p));
    else if (name.endsWith('.ts')) out.push(p);
  }
  return out;
}

const FORBIDDEN_IMPORT = /from\s+'(?!\.\/)[^']*'/; // non-relative imports
// Law 2: ambient globals the core must not touch. (Type-only references in
// comments are fine; this grep matches identifier usage.)
const FORBIDDEN_GLOBALS =
  /\b(window|document|localStorage|navigator|setTimeout|setInterval|clearTimeout|clearInterval|console|AudioContext|AudioWorklet|Worker|HTMLAudioElement|fetch)\b/;

// Date.now / Math.random must not appear outside comments/doc strings.
const FORBIDDEN_TIME_RANDOM = /\b(Date\.now|Math\.random|new Date\(\))\b/;

function stripComments(src: string): string {
  // remove /* */ blocks and // line comments (rough but sufficient: string
  // literals containing '//' would over-strip, which only weakens false
  // positives, never hides a violation).
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
}

describe('player-core purity laws (CI gate)', () => {
  const files = coreFiles(CORE_DIR);

  it('found the core sources', () => {
    expect(files.length).toBeGreaterThanOrEqual(8);
    expect(files.some((f) => f.endsWith('player.ts'))).toBe(true);
  });

  it.each(files.map((f) => [f.split('/').pop()!, f]))(
    'law 1: %s imports only relatively',
    (_name, path) => {
      const src = readFileSync(path, 'utf8');
      const imports = src.match(/^import[\s\S]*?from\s+'[^']*'/gm) ?? [];
      const offenders = imports.filter((imp) => FORBIDDEN_IMPORT.test(imp));
      expect(offenders, `${path}: ${offenders.join(' | ')}`).toEqual([]);
    },
  );

  it.each(files.map((f) => [f.split('/').pop()!, f]))(
    'law 2: %s touches no ambient globals',
    (_name, path) => {
      const src = stripComments(readFileSync(path, 'utf8'));
      const m = src.match(FORBIDDEN_GLOBALS);
      expect(m, `${path}: found ${m?.join(', ')}`).toBeNull();
      const tr = src.match(FORBIDDEN_TIME_RANDOM);
      expect(tr, `${path}: found ${tr?.join(', ')}`).toBeNull();
    },
  );
});
