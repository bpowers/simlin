// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig } from '@rstest/core';

const here = import.meta.dirname;
const coreLib = path.resolve(here, '../core/lib');
const engineLib = path.resolve(here, '../engine/lib');

export default defineConfig({
  testEnvironment: 'node',
  // The vendored seshcookie library keeps its tests next to its source
  // (see seshcookie/seshcookie.ts for provenance).
  include: ['tests/**/*.test.ts', 'seshcookie/**/*.test.ts'],
  output: {
    // In the node environment rstest externalizes node_modules, so named
    // imports go through Node's ESM-CJS interop, which detects exports with
    // cjs-module-lexer. timestamp_pb's `goog.object.extend(exports, ...)` is
    // invisible to that static analysis, leaving `Timestamp` undefined at
    // runtime (production is unaffected: tsc emits plain `require`). Bundling
    // just this subpath routes it through rspack's dynamic CJS interop -- the
    // same path the local schemas/*_pb.js already take -- while the jspb
    // runtime it requires stays external, so there is still exactly one copy
    // of the google-protobuf runtime in the process.
    bundleDependencies: ['google-protobuf/google/protobuf/timestamp_pb'],
  },
  resolve: {
    // tsconfig `paths` maps @simlin/* onto source and Rsbuild prefers tsconfig
    // over `alias`; the server exercises the built output, as it did under jest.
    aliasStrategy: 'prefer-alias',
    alias: {
      // The engine's "exports" map offers `module` (browser) before `node`, and
      // rspack picks `module`, which drags in the Worker-based browser backend.
      // Pin the node flavor. Trailing `$` is an exact match, so the specific
      // entries must precede the prefix ones.
      '@simlin/engine/internal/wasm$': path.join(engineLib, 'internal/wasm.node.js'),
      '@simlin/engine/internal/backend-factory$': path.join(engineLib, 'backend-factory.node.js'),
      '@simlin/engine$': path.join(engineLib, 'index.js'),
      '@simlin/engine': engineLib,
      '@simlin/core$': path.join(coreLib, 'index.js'),
      '@simlin/core': coreLib,
    },
  },
});
