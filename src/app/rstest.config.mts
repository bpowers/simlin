// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig, defineInlineProject } from '@rstest/core';

const here = import.meta.dirname;
const engineSrc = path.resolve(here, '../engine/src');
const coreSrc = path.resolve(here, '../core');

export default defineConfig({
  projects: [
    // The build scripts under config/ are plain CommonJS and are tested as such.
    defineInlineProject({
      name: 'build-utils',
      testEnvironment: 'node',
      include: ['tests/**/*.test.js'],
    }),
    defineInlineProject({
      name: 'app',
      testEnvironment: 'jsdom',
      include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
      setupFiles: ['./tests/setup-testing-library.ts'],
      output: {
        // Tests query rendered class names directly (`.breadcrumbLink`), which
        // worked under jest because a Proxy stub echoed each property name back.
        // Rstest compiles the real stylesheets, so keep the emitted identifier
        // equal to the local name instead of hashing it.
        cssModules: { localIdentName: '[local]' },
      },
      resolve: {
        // The app's tests drive engine/core *sources* (not their built output),
        // as jest's moduleNameMapper did. Rsbuild prefers tsconfig `paths` over
        // `alias` by default, and those paths point at package directories.
        aliasStrategy: 'prefer-alias',
        alias: {
          // Trailing `$` is an exact match, so specific entries precede prefixes.
          '@simlin/engine/internal/wasm$': path.join(engineSrc, 'internal/wasm.node.ts'),
          '@simlin/engine/internal/backend-factory$': path.join(engineSrc, 'backend-factory.node.ts'),
          '@simlin/engine$': path.join(engineSrc, 'index.ts'),
          '@simlin/core/datamodel$': path.join(coreSrc, 'datamodel.ts'),
          '@simlin/core/common$': path.join(coreSrc, 'common.ts'),
          '@simlin/core/collections$': path.join(coreSrc, 'collections.ts'),
        },
      },
    }),
  ],
});
