const { createConfig } = require('./eslint.config.shared');

module.exports = createConfig({
  react: true,
  ignorePatterns: [
    'node_modules/',
    'build/',
    'dist/',
    'lib/',
    'lib.browser/',
    'lib.module/',
    'src/engine-v2/',
    'src/system-dynamics-engine/',
    'src/schemas/',
    // Rust target directories
    'target/',
    // Generated files
    '*.pb.js',
    '*.pb.ts',
    '*.pb.d.ts',
    // Build outputs
    'public/',
    'website/build/',
    'src/app/build/',
    'src/app/build-component/',
  ],
});