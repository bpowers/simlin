// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig } from '@rstest/core';

const engineLib = path.resolve(import.meta.dirname, '../engine/lib');

export default defineConfig({
  testEnvironment: 'node',
  include: ['tests/**/*.test.ts'],
  resolve: {
    // tsconfig `paths` maps @simlin/* onto source, and Rsbuild prefers tsconfig
    // over `alias` unless told otherwise. These tests drive the engine's built
    // output, so the aliases below must win.
    aliasStrategy: 'prefer-alias',
    alias: {
      // The engine's "exports" map offers a `module` (browser) and a `node`
      // condition; rspack picks `module`, which pulls in the Worker-based
      // browser backend and dies with "Worker is not defined" under Node.
      // Pin the node flavor explicitly, exactly as jest's moduleNameMapper did.
      // Trailing `$` means exact match, so these must precede the prefix entry.
      '@simlin/engine/internal/wasm$': path.join(engineLib, 'internal/wasm.node.js'),
      '@simlin/engine/internal/backend-factory$': path.join(engineLib, 'backend-factory.node.js'),
      '@simlin/engine$': path.join(engineLib, 'index.js'),
      '@simlin/engine': engineLib,
    },
  },
});
