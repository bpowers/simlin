const { createConfig } = require('../../eslint.config.shared');

module.exports = createConfig({
  react: true,
  project: ['./tsconfig.browser.json', './tsconfig.tests.json'],
  ignorePatterns: [
    'src/engine-v2/',
    'src/system-dynamics-engine/',
    'src/schemas/',
    'build/',
    'build-component/',
    'scripts/',
    'config/',
  ],
});