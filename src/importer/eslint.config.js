const { createConfig } = require('../../eslint.config.shared');

module.exports = createConfig({
  project: './tsconfig.json',
  ignorePatterns: [
    'lib/',
    'lib.browser/',
    'core/',
  ],
});