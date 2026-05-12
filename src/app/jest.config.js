const path = require('node:path');

// wouter ships as ESM-only ("type":"module"), which CommonJS Jest's resolver
// mishandles via the package.json "exports" conditions, so we map "wouter" and
// its subpaths ("wouter/memory-location", etc.) straight at the .js sources and
// let ts-jest down-level them (transformIgnorePatterns is [] so node_modules is
// transformed). Derive the path from require.resolve() rather than hardcoding
// the pnpm virtual-store directory: that directory's name embeds the React
// version hash, so a hardcoded `wouter@3.9.0_react@<X>` path silently breaks
// every app test the moment the lockfile bumps React (see issue #523).
// require.resolve('wouter') runs from this file, where wouter is a direct
// dependency, and lands on `<store>/wouter/src/index.js`.
const wouterEntry = require.resolve('wouter');
const wouterSrcDir = path.dirname(wouterEntry);

/** @type {import('jest').Config} */
const config = {
  // The legacy build-utils tests use plain JS in a Node env. The new TSX tests
  // for app components need ts-jest + jsdom + testing-library. We register two
  // Jest projects so a single `jest` invocation runs both.
  projects: [
    {
      displayName: 'build-utils',
      testEnvironment: 'node',
      testMatch: ['<rootDir>/tests/**/*.test.js'],
    },
    {
      displayName: 'app',
      preset: 'ts-jest',
      testEnvironment: 'jsdom',
      testMatch: ['<rootDir>/tests/**/*.test.ts', '<rootDir>/tests/**/*.test.tsx'],
      moduleFileExtensions: ['ts', 'tsx', 'js'],
      moduleNameMapper: {
        '\\.css$': '<rootDir>/tests/css-module-stub.ts',
        // See the wouterEntry/wouterSrcDir comment above for why these point at
        // the real .js sources instead of letting Jest resolve "wouter".
        '^wouter$': wouterEntry,
        '^wouter/(.*)$': path.join(wouterSrcDir, '$1'),
        '^@simlin/engine/internal/wasm$': '<rootDir>/../engine/src/internal/wasm.node.ts',
        '^@simlin/engine/internal/backend-factory$': '<rootDir>/../engine/src/backend-factory.node.ts',
        '^@simlin/engine$': '<rootDir>/../engine/src/index.ts',
        '^@simlin/core/datamodel$': '<rootDir>/../core/datamodel.ts',
        '^@simlin/core/common$': '<rootDir>/../core/common.ts',
        '^@simlin/core/collections$': '<rootDir>/../core/collections.ts',
      },
      transform: {
        // ts-jest in v30 treats `isolatedModules` as a tsconfig setting; pass
        // a shaped tsconfig override here instead of the deprecated top-level
        // option so we don't spam deprecation warnings on every run.
        '\\.m?jsx?$': [
          'ts-jest',
          { useESM: false, tsconfig: { isolatedModules: true }, diagnostics: false },
        ],
        '\\.tsx?$': ['ts-jest', { useESM: false, tsconfig: { isolatedModules: true } }],
      },
      transformIgnorePatterns: [],
    },
  ],
};

module.exports = config;
