// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig } from '@rstest/core';

const here = import.meta.dirname;

export default defineConfig({
  testEnvironment: 'jsdom',
  include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  setupFiles: ['./src/test-utils/setup-testing-library.ts'],
  resolve: {
    alias: {
      // <EditorHost> would otherwise spin up the WASM engine. Trailing `$` keeps
      // this to the package root, leaving deep imports (e.g. theme.css) alone.
      '@simlin/diagram$': path.join(here, 'src/test-utils/diagram-mock.tsx'),
    },
  },
});
