const { createConfig } = require('../../eslint.config.shared');

module.exports = createConfig({
  react: true,
  project: './tsconfig.browser.json',
  ignorePatterns: [
    'lib/',
    'lib.browser/',
  ],
});