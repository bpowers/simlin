// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/** @type {import('jest').Config} */
const config = {
  preset: 'ts-jest',
  testEnvironment: 'jsdom',
  testMatch: ['<rootDir>/tests/**/*.test.ts', '<rootDir>/tests/**/*.test.tsx'],
  moduleFileExtensions: ['ts', 'tsx', 'js'],
  moduleNameMapper: {
    '\\.css$': '<rootDir>/tests/css-module-stub.ts',
    '^@simlin/engine/internal/wasm$': '<rootDir>/../engine/src/internal/wasm.node.ts',
    '^@simlin/engine$': '<rootDir>/../engine/src/index.ts',
    '^@simlin/core/datamodel$': '<rootDir>/../core/datamodel.ts',
    '^@simlin/core/common$': '<rootDir>/../core/common.ts',
    '^@simlin/core/collections$': '<rootDir>/../core/collections.ts',
  },
};

module.exports = config;
