const { createConfig } = require('../../eslint.config.shared');

const configs = createConfig({
  project: './tsconfig.json',
  ignorePatterns: [
    'lib/',
    'public/',
    'default_projects/',
    'schemas/*_pb.js',
    'schemas/*.d.ts',
  ],
});

// The vendored seshcookie library (see seshcookie/seshcookie.ts for
// provenance) is kept byte-close to upstream for easy reconciliation;
// relax the two house rules its upstream style trips instead of
// patching the source.
configs.push({
  files: ['seshcookie/**/*.ts'],
  languageOptions: {
    globals: {
      // the test drives real HTTP requests with node's global fetch
      fetch: 'readonly',
      Response: 'readonly',
    },
  },
  rules: {
    '@typescript-eslint/no-explicit-any': 'off',
    '@typescript-eslint/no-unused-vars': ['warn', { argsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' }],
  },
});

module.exports = configs;
