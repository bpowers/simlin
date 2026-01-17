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
    '^@system-dynamics/engine2/internal/wasm$': '<rootDir>/../engine2/lib/internal/wasm.node.js',
    '^@system-dynamics/engine2/(.*)$': '<rootDir>/../engine2/lib/$1.js',
    '^@system-dynamics/engine2$': '<rootDir>/../engine2/lib/index.js',
  },
};

module.exports = config;
