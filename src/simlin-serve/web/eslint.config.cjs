const { createConfig } = require('../../../eslint.config.shared');

module.exports = createConfig({
  react: true,
  ignorePatterns: ['dist/', 'node_modules/'],
});
