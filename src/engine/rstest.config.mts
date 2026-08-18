// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defineConfig } from '@rstest/core';

import path from 'node:path';

export default defineConfig({
  testEnvironment: 'node',
  include: ['tests/**/*.test.ts'],
  // The two platform-specific specifiers (@simlin/engine/internal/wasm and
  // .../backend-factory) are already pinned to their Node flavors by the `paths`
  // in tsconfig.json, which Rsbuild honors by default -- no alias needed here.
  resolve: {
    alias: {
      // wasm.browser.ts statically imports the .wasm artifact, which only a
      // bundler with asyncWebAssembly can evaluate. Its source-resolution logic
      // is unit-tested (tests/wasm-browser.test.ts) against this stub standing
      // in for the bundler-instantiated module namespace.
      '../../core/libsimlin-browser.wasm': path.join(import.meta.dirname, 'tests/stubs/browser-wasm-stub.ts'),
    },
  },
});
