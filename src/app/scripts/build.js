'use strict';

process.env.BABEL_ENV = 'production';
process.env.NODE_ENV = 'production';

process.on('unhandledRejection', err => {
  throw err;
});

// Ensure environment variables are read.
require('../config/env');

const fs = require('fs');
const pc = require('picocolors');
const { createRsbuild } = require('@rsbuild/core');
const {
  checkRequiredFiles,
  measureFileSizesBeforeBuild,
  printFileSizesAfterBuild,
} = require('../config/build-utils');

const paths = require('../config/paths');

const WARN_AFTER_BUNDLE_GZIP_SIZE = 512 * 1024;
const WARN_AFTER_CHUNK_GZIP_SIZE = 1024 * 1024;

// Warn and crash if required files are missing
if (!checkRequiredFiles([paths.appHtml, paths.appIndexJs])) {
  process.exit(1);
}

async function build() {
  const previousFileSizes = measureFileSizesBeforeBuild(paths.appBuild);

  // Clean the output directory
  fs.rmSync(paths.appBuild, { recursive: true, force: true });
  fs.mkdirSync(paths.appBuild, { recursive: true });

  console.log('Creating an optimized production build...');

  let rsbuild;
  try {
    const rsbuildConfig = require('../config/rsbuild/rsbuild.config.js');
    rsbuild = await createRsbuild({
      cwd: paths.appPath,
      rsbuildConfig,
    });
  } catch (err) {
    console.log(pc.red('Failed to load Rsbuild config.\n'));
    throw err;
  }

  try {
    const { stats } = await rsbuild.build();

    const errors = stats.errors || [];
    const warnings = stats.warnings || [];

    if (errors.length) {
      // Only keep the first error to reduce noise.
      const msg = typeof errors[0] === 'string' ? errors[0] : errors[0].message || String(errors[0]);
      console.log(pc.red('Failed to compile.\n'));
      console.log(msg + '\n');
      process.exit(1);
    }

    if (warnings.length) {
      console.log(pc.yellow('Compiled with warnings.\n'));
      for (const w of warnings) {
        const msg = typeof w === 'string' ? w : w.message || String(w);
        console.log(msg + '\n');
      }
    } else {
      console.log(pc.green('Compiled successfully.\n'));
    }

    console.log('File sizes after gzip:\n');
    printFileSizesAfterBuild(
      stats,
      previousFileSizes,
      paths.appBuild,
      WARN_AFTER_BUNDLE_GZIP_SIZE,
      WARN_AFTER_CHUNK_GZIP_SIZE
    );
    console.log();
  } catch (err) {
    console.log(pc.red('Failed to compile.\n'));
    console.log((err.message || err) + '\n');
    process.exit(1);
  }
}

build().catch(err => {
  if (err && err.message) {
    console.log(err.message);
  }
  process.exit(1);
});
