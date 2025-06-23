const { createConfig } = require('../eslint.config.shared');

module.exports = createConfig({
  react: true,
  project: './tsconfig.json',
  ignorePatterns: [
    'build/',
    'docs/',
    'static/',
  ],
});