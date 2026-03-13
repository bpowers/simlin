// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/** @type {import('jest').Config} */
const config = {
  testEnvironment: 'node',
  testMatch: ['<rootDir>/tests/**/*.test.ts'],
  moduleFileExtensions: ['ts', 'js'],
  // Extend ts-jest to also transform ESM .js files (e.g. uuid v13)
  transform: {
    '^.+\\.[tj]sx?$': ['ts-jest', { tsconfig: { allowJs: true } }],
  },
  moduleNameMapper: {
    '^@simlin/engine/internal/wasm$': '<rootDir>/../engine/lib/internal/wasm.node.js',
    '^@simlin/engine/internal/backend-factory$': '<rootDir>/../engine/lib/backend-factory.node.js',
    '^@simlin/engine/(.*)$': '<rootDir>/../engine/lib/$1.js',
    '^@simlin/engine$': '<rootDir>/../engine/lib/index.js',
    '^@simlin/core/(.*)$': '<rootDir>/../core/lib/$1.js',
    '^@simlin/core$': '<rootDir>/../core/lib/index.js',
  },
  // pnpm nests packages under .pnpm/<pkg>/node_modules/<pkg>/, so a simple
  // negative lookahead for the package name at the first node_modules/ won't
  // work.  Instead, allow transformation whenever the full path contains the
  // package name.
  transformIgnorePatterns: ['/node_modules/(?!.*(jose|uuid)/)'],
};

module.exports = config;
