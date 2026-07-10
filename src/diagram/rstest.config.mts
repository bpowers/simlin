// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import path from 'node:path';
import { defineConfig, defineInlineProject } from '@rstest/core';

const here = import.meta.dirname;
const engineSrc = path.resolve(here, '../engine/src');
const coreSrc = path.resolve(here, '../core');

// Jest let each file pick its environment with an `@jest-environment` docblock;
// rstest has no such pragma, so the split is declared once, here. These run
// without a DOM on purpose -- they cover the pure functional cores (the
// project-controller is documented as "zero React, zero DOM"), and handing them
// a jsdom global would silently let a DOM dependency creep back in unnoticed.
const NODE_ENV_TESTS = [
  'tests/editor-applyPatch.test.ts',
  'tests/editor-input.test.ts',
  'tests/hosted-web-editor-delete.test.ts',
  'tests/hosted-web-editor-load-errors.test.ts',
  'tests/hosted-web-editor-save.test.ts',
  'tests/merge-live-view.test.ts',
  'tests/module-creation.test.ts',
  'tests/module-details-utils.test.ts',
  'tests/module-navigation.test.ts',
  'tests/module-patch.test.ts',
  'tests/module-wiring.test.ts',
  'tests/project-controller-connector-sync.test.ts',
  'tests/project-controller.test.ts',
  'tests/svg-rendering.test.ts',
];

const resolve = {
  // These tests drive the engine's and core's *source*, as jest's
  // moduleNameMapper did. `prefer-alias` does not merely reorder alias ahead of
  // tsconfig `paths` -- Rsbuild wires rspack's tsconfig-paths resolver ONLY under
  // `prefer-tsconfig`, so here `paths` is off entirely. Every @simlin/* specifier
  // that should resolve to source therefore needs an explicit alias; anything
  // missed falls through package.json "exports" to built lib/ output, where a
  // stale build silently passes the tests.
  aliasStrategy: 'prefer-alias' as const,
  alias: {
    // Trailing `$` is an exact match, so specific entries precede prefixes.
    '@simlin/engine/internal/wasm$': path.join(engineSrc, 'internal/wasm.node.ts'),
    '@simlin/engine/internal/backend-factory$': path.join(engineSrc, 'backend-factory.node.ts'),
    '@simlin/engine$': path.join(engineSrc, 'index.ts'),
    // Bare `@simlin/core` would otherwise resolve the directory through its
    // package.json "main", i.e. back to lib/.
    '@simlin/core$': path.join(coreSrc, 'index.ts'),
    // Prefix entry: every `@simlin/core/<name>` lands on `../core/<name>.ts`.
    // jest listed datamodel/common/collections one by one and so quietly left
    // canonicalize and base64 on the built output; a prefix cannot rot that way.
    '@simlin/core': coreSrc,
  },
};

const output = {
  // Tests query rendered class names directly (`.eqnEditor`), which worked under
  // jest because a Proxy stub echoed each property name back. Rstest compiles the
  // real stylesheets, so keep the emitted identifier equal to the local name
  // instead of hashing it.
  cssModules: { localIdentName: '[local]' },
};

export default defineConfig({
  projects: [
    defineInlineProject({
      name: 'node',
      testEnvironment: 'node',
      include: NODE_ENV_TESTS,
      output,
      resolve,
    }),
    defineInlineProject({
      name: 'jsdom',
      testEnvironment: 'jsdom',
      include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
      exclude: NODE_ENV_TESTS,
      // Setup files are the only hook that runs before a test module's imports
      // are evaluated, which is what the engine's TextEncoder use demands.
      setupFiles: ['./tests/setup-testing-library.ts', './tests/setup-text-encoder.ts'],
      output,
      resolve,
    }),
  ],
});
