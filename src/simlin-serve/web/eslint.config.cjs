const { createConfig } = require('../../../eslint.config.shared');

const configs = createConfig({
  react: true,
  ignorePatterns: ['dist/', 'node_modules/'],
});

// `sessionStorage` is a standard browser global; the shared config doesn't
// list it, so add it locally rather than mutating the shared file.
const baseConfig = configs.find((c) => c.files && c.files.includes('**/*.ts'));
if (baseConfig) {
  baseConfig.languageOptions = {
    ...baseConfig.languageOptions,
    globals: {
      ...baseConfig.languageOptions.globals,
      sessionStorage: 'readonly',
      localStorage: 'readonly',
      HTMLElement: 'readonly',
      HTMLInputElement: 'readonly',
      Response: 'readonly',
      RequestInit: 'readonly',
    },
  };
}

module.exports = configs;
