// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/** @type {import('jest').Config} */
const config = {
  preset: 'ts-jest',
  testEnvironment: 'node',
  testMatch: ['<rootDir>/tests/**/*.test.ts'],
  moduleFileExtensions: ['ts', 'js'],
  moduleNameMapper: {
    // Mirror @simlin/engine's own exports aliases so a core test can drive the
    // engine at runtime: these internal subpaths resolve to platform-specific
    // node files that do not live under the literal subpath.
    '^@simlin/engine/internal/wasm$': '<rootDir>/../engine/lib/internal/wasm.node.js',
    '^@simlin/engine/internal/backend-factory$': '<rootDir>/../engine/lib/backend-factory.node.js',
    '^@simlin/engine/(.*)$': '<rootDir>/../engine/lib/$1.js',
    '^@simlin/engine$': '<rootDir>/../engine/lib/index.js',
  },
};

module.exports = config;
