const { createConfig } = require('../../eslint.config.shared');

const configs = createConfig({
  react: true,
  ignorePatterns: ['dist/', 'node_modules/', 'e2e/.output/', 'playwright-report/', 'test-results/'],
});

// Standard browser globals the shared config does not list; added locally
// (as simlin-serve/web does) rather than widening the shared file.
const baseConfig = configs.find((c) => c.files && c.files.includes('**/*.ts'));
if (baseConfig) {
  baseConfig.languageOptions = {
    ...baseConfig.languageOptions,
    globals: {
      ...baseConfig.languageOptions.globals,
      WebAssembly: 'readonly',
      HTMLElement: 'readonly',
      AbortController: 'readonly',
      AbortSignal: 'readonly',
      Blob: 'readonly',
      Response: 'readonly',
    },
  };
}

module.exports = configs;
