// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defineConfig, devices } from '@playwright/test';

// Static-page journey for the built widget bundle (dist/widget.js). No web
// server: the spec serves the harness page, the bundle, and the wasm from disk
// through page.route(), so `pnpm build` (here and in src/engine) is the only
// prerequisite. Run with `pnpm test:e2e` from src/notebook-widget.
export default defineConfig({
  testDir: '.',
  testMatch: /.*\.spec\.ts/,
  outputDir: './.output/test-results',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: process.env.CI ? 'github' : 'list',
  timeout: 120_000,
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    ...devices['Desktop Chrome'],
    viewport: { width: 1280, height: 900 },
  },
});
