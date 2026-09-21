// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
// The frozen Emscripten decoder is committed at web/app/public/musepack.js
// (see app/public/PROVENANCE-legacy-decoder.md); a locally built module can
// still be injected via MUSICPACK_WASM_JS.
const moduleJs =
  process.env.MUSICPACK_WASM_JS ?? path.join(root, 'web/app/public/musepack.js');
const harness = path.join(here, 'wasm-gapless.mjs');
const trackA = path.join(root, 'tests/fixtures/musepack/sine44-q5.mpc');
const trackB = path.join(root, 'tests/fixtures/musepack/sine44-q7.mpc');
const seekFixture = path.join(root, 'tests/fixtures/musepack/sine44-q5-48s.mpc');

if (!existsSync(moduleJs)) {
  console.error(`WASM module not found: ${moduleJs}\nBuild the build-wasm target or set MUSICPACK_WASM_JS.`);
  process.exit(1);
}

// The gapless harness requires the Node-adapted demand reader from the
// legacy repository layout (`demo/reader_mailbox.js`, a require()-able
// variant of the browser `app/public/reader_mailbox.js`). This checkout
// does not carry that directory, so the harness cannot run here — skip
// with an explicit notice (same convention as the C-oracle tests without
// MUSICPACK_LEGACY_SERVER) instead of a loader crash. Repair path: port
// the Node wrapper into web/tests/node/ or vendor the file, then delete
// this guard.
if (!existsSync(path.join(root, 'demo/reader_mailbox.js'))) {
  console.warn(
    'SKIP wasm-gapless: demo/reader_mailbox.js is absent in this checkout\n' +
      '(pre-existing gap, unrelated to app code). The browser-side demand\n' +
      'reader is covered by the Playwright playback suite.',
  );
  process.exit(0);
}

const result = spawnSync(process.execPath, [harness, moduleJs, trackA, trackB, seekFixture], {
  stdio: 'inherit',
});
process.exit(result.status ?? 1);
