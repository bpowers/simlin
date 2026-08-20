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
      // Setup files run before a test module's imports are evaluated, which is
      // the only point early enough for a stub that App.tsx's module body must
      // observe (see tests/fetch-stub.ts).
      setupFiles: ['./tests/setup-testing-library.ts', './tests/setup-fetch.ts'],
      output: {
        // Tests query rendered class names directly (`.breadcrumbLink`), which
        // worked under jest because a Proxy stub echoed each property name back.
        // Rstest compiles the real stylesheets, so keep the emitted identifier
        // equal to the local name instead of hashing it.
        cssModules: { localIdentName: '[local]' },
      },
      resolve: {
        // The app's tests drive engine/core *sources* (not their built output),
        // as jest's moduleNameMapper did. `prefer-alias` switches rsbuild's
        // tsconfig-paths resolution off entirely (it is wired only under
        // `prefer-tsconfig`), so anything not aliased here resolves through
        // package.json "exports" to built lib/ output instead.
        aliasStrategy: 'prefer-alias',
        alias: {
          // Trailing `$` is an exact match, so specific entries precede prefixes.
          '@simlin/engine/internal/wasm$': path.join(engineSrc, 'internal/wasm.node.ts'),
          '@simlin/engine/internal/backend-factory$': path.join(engineSrc, 'backend-factory.direct.ts'),
          '@simlin/engine$': path.join(engineSrc, 'index.ts'),
          // Bare `@simlin/core` would otherwise resolve the directory through its
          // package.json "main", i.e. back to lib/.
          '@simlin/core$': path.join(coreSrc, 'index.ts'),
          // Prefix entry: every `@simlin/core/<name>` lands on `../core/<name>.ts`.
          // jest listed subpaths one by one and so quietly left base64 on the
          // built output; a prefix cannot rot that way.
          '@simlin/core': coreSrc,
        },
      },
    }),
  ],
});
