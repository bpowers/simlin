// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/** @type {import('jest').Config} */
const config = {
  preset: 'ts-jest',
  testEnvironment: 'jsdom',
  testMatch: ['<rootDir>/tests/**/*.test.ts'],
  moduleFileExtensions: ['ts', 'tsx', 'js'],
  moduleNameMapper: {
    '\\.css$': '<rootDir>/tests/css-module-stub.ts',
    '^@system-dynamics/engine2/internal/wasm$': '<rootDir>/../engine2/src/internal/wasm.node.ts',
  },
};

module.exports = config;
