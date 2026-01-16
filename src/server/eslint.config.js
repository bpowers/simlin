const { createConfig } = require('../../eslint.config.shared');

module.exports = createConfig({
  project: './tsconfig.json',
  ignorePatterns: [
    'lib/',
    'public/',
    'default_projects/',
    'schemas/*_pb.js',
    'schemas/*.d.ts',
  ],
});
