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
        // wouter ships as ESM-only ("type":"module") which CommonJS ts-jest cannot
        // process. The diagram package uses a stub for the same reason; here we
        // resolve to the real ESM source via ts-jest so routing tests can verify
        // wouter's <Switch> behavior. Mapping the package entry directly to the
        // .js source bypasses the package.json "exports" condition resolution
        // that breaks under CJS Jest.
        '^wouter$': '<rootDir>/../../node_modules/.pnpm/wouter@3.9.0_react@19.2.4/node_modules/wouter/src/index.js',
        '^wouter/(.*)$': '<rootDir>/../../node_modules/.pnpm/wouter@3.9.0_react@19.2.4/node_modules/wouter/src/$1',
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
