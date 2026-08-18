// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defineConfig, devices } from '@playwright/test';

// The JupyterLab journey for pysimlin's notebook editor (design AC4.2):
// global-setup launches `jupyter lab` from the pysimlin venv on a random
// port with a random token and a temporary root directory, the spec drives
// the real JupyterLab UI (cells, kernel, the anywidget-hosted Editor), and
// global-teardown shuts the server down.
//
// Run with `make e2e` from src/pysimlin (or, from the repo root,
// `node_modules/.bin/playwright test -c src/pysimlin/e2e/playwright.config.ts`).
// Prerequisites: the pysimlin venv synced with the dev and e2e extras and
// its CFFI extension built; the widget assets staged into simlin/_widget/;
// Playwright's chromium (`npx playwright install --with-deps chromium`).
// SIMLIN_E2E_PYTHON overrides the interpreter (default: ./.venv/bin/python);
// SIMLIN_E2E_KEEP_TMP keeps the server's temporary tree for inspection.
export default defineConfig({
  testDir: '.',
  testMatch: /.*\.spec\.ts/,
  outputDir: './.output/test-results',
  globalSetup: './global-setup.ts',
  globalTeardown: './global-teardown.ts',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  // The whole journey (kernel start, wasm compile, several cell executions,
  // a disk-poll round trip) fits well inside this; the budget is a backstop
  // against a wedged kernel, not a target.
  timeout: 180_000,
  expect: { timeout: 30_000 },
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    ...devices['Desktop Chrome'],
    viewport: { width: 1400, height: 1000 },
  },
});
