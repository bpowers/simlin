// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig } from '@rstest/core';

const here = import.meta.dirname;

export default defineConfig({
  testEnvironment: 'jsdom',
  include: ['src/**/*.test.ts', 'src/**/*.test.tsx', 'build/**/*.test.ts'],
  setupFiles: ['./src/test-utils/setup-testing-library.ts'],
  resolve: {
    alias: {
      // The shell tests drive the real render()/initialize() lifecycle against
      // a fake anywidget model; the Editor itself would spin up the WASM engine
      // and is covered by the Playwright journey in e2e/ instead. Trailing `$`
      // keeps deep imports (theme.css) on the real package.
      '@simlin/diagram/Editor$': path.join(here, 'src/test-utils/editor-mock.tsx'),
      // The engine's ready() must not need a wasm artifact in unit tests.
      '@simlin/engine$': path.join(here, 'src/test-utils/engine-mock.ts'),
    },
  },
});
