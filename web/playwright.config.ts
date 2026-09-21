import { defineConfig, devices } from '@playwright/test';

// The suite is tiered so pull requests get fast feedback without
// weakening coverage (the full run is ~15 min, dominated by real-time
// audio suites):
//
//   ui       — flows that do not rely on audio timing (browse, search,
//              settings, auth, offline install, backend selection).
//              Parallelisable; CI runs it with `--workers=2`.
//   visual   — screenshot baselines; isolated so rendering contention
//              from other projects can never perturb a diff.
//   playback — the real-audio timing suites (engine, worker, PCM
//              exactness, crossfade/differential, rate matrix). These
//              must run sequentially: they assert wall-clock behaviour
//              of the WebAudio graph and starve under CPU contention.
//   soak     — long stress suites (underrun recovery, soak, and the
//              perf harness outside Playwright). Nightly / manual only.
//
// Selection:
//   npx playwright test                        # everything (local default)
//   npx playwright test --project=ui --workers=2
//   npx playwright test --project=playback
//   npx playwright test --project=soak
// (npm scripts test:e2e:* wrap these.)

const PLAYBACK_FILES =
  /specs\/(playback|representations|waveform|lyrics-playback|rust-audio|rust-worker|rust-playback|rust-differential|rust-rate-matrix|rust-error-verify|rust-observation|rust-offline)\.spec\.ts$/;
const SOAK_FILES = /specs\/rust-(soak|underrun)\.spec\.ts$/;
const VISUAL_FILES = /specs\/visual\.spec\.ts$/;

export default defineConfig({
  testDir: 'tests/e2e/specs',
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  // CI runners are slow enough that ~1 in 30 browser-timing assertions can
  // miss; retry there so one flaky poll doesn't fail the whole push.
  retries: process.env.CI ? 2 : 0,
  reporter: [['list']],
  use: {
    baseURL: process.env.MUSICPACK_E2E_URL ?? 'http://127.0.0.1:8099',
    trace: 'on-first-retry',
    ...devices['Desktop Chrome'],
  },
  projects: [
    {
      name: 'ui',
      testIgnore: [PLAYBACK_FILES, SOAK_FILES, VISUAL_FILES],
    },
    {
      name: 'visual',
      testMatch: [VISUAL_FILES],
    },
    {
      name: 'playback',
      testMatch: [PLAYBACK_FILES],
    },
    {
      name: 'soak',
      testMatch: [SOAK_FILES],
    },
  ],
  webServer: {
    command: 'bash tests/e2e/start-server.sh',
    url: 'http://127.0.0.1:8099/api/v1/health',
    // Local iteration loop: MUSICPACK_E2E_REUSE=1 reuses an already-running
    // harness (skips fixture rebuild + verify between runs — target a
    // single spec with --grep while the harness stays up). Never reuse
    // under CI: every run must prove the harness from scratch.
    reuseExistingServer: !process.env.CI && process.env.MUSICPACK_E2E_REUSE === '1',
    timeout: 30_000,
  },
});
