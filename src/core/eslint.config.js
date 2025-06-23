const { createConfig } = require('../../eslint.config.shared');

module.exports = createConfig({
  project: './tsconfig.browser.json',
  ignorePatterns: [
    'pb/',
    'lib/',
    'lib.browser/',
    'lib.module/',
  ],
});