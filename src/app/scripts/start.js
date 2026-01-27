'use strict';

process.env.BABEL_ENV = 'development';
process.env.NODE_ENV = 'development';

process.on('unhandledRejection', err => {
  throw err;
});

// Ensure environment variables are read.
require('../config/env');

const pc = require('picocolors');
const { createRsbuild } = require('@rsbuild/core');
const { checkRequiredFiles } = require('../config/build-utils');
const paths = require('../config/paths');

// Warn and crash if required files are missing
if (!checkRequiredFiles([paths.appHtml, paths.appIndexJs])) {
  process.exit(1);
}

const DEFAULT_PORT = parseInt(process.env.PORT, 10) || 3000;

async function startDevServer() {
  console.log(pc.cyan('Starting the development server...\n'));

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
    const { server } = await rsbuild.startDevServer({
      port: DEFAULT_PORT,
    });

    ['SIGINT', 'SIGTERM'].forEach(function (sig) {
      process.on(sig, async function () {
        await server.close();
        process.exit();
      });
    });

    if (process.env.CI !== 'true') {
      process.stdin.on('end', async function () {
        await server.close();
        process.exit();
      });
    }
  } catch (err) {
    console.log(pc.red('Failed to start development server.\n'));
    console.error(err);
    process.exit(1);
  }
}

startDevServer().catch(err => {
  if (err && err.message) {
    console.log(err.message);
  }
  process.exit(1);
});
