'use strict';

// Do this as the first thing so that any code reading it knows the right env.
process.env.BABEL_ENV = 'development';
process.env.NODE_ENV = 'development';

// Makes the script crash on unhandled rejections instead of silently
// ignoring them. In the future, promise rejections that are not handled will
// terminate the Node.js process with a non-zero exit code.
process.on('unhandledRejection', err => {
  throw err;
});

// Ensure environment variables are read.
require('../config/env');

const { createRsbuild } = require('@rsbuild/core');
const fs = require('fs');
const chalk = require('react-dev-utils/chalk');
const checkRequiredFiles = require('react-dev-utils/checkRequiredFiles');
const {
  choosePort,
  prepareProxy,
  prepareUrls,
} = require('react-dev-utils/WebpackDevServerUtils');
const openBrowser = require('react-dev-utils/openBrowser');
const paths = require('../config/paths');
const { checkBrowsers } = require('react-dev-utils/browsersHelper');

const useYarn = fs.existsSync(paths.yarnLockFile);
const isInteractive = process.stdout.isTTY;

// Warn and crash if required files are missing
if (!checkRequiredFiles([paths.appHtml, paths.appIndexJs])) {
  process.exit(1);
}

// Tools like Cloud9 rely on this.
const DEFAULT_PORT = parseInt(process.env.PORT, 10) || 3000;
const HOST = process.env.HOST || '0.0.0.0';

if (process.env.HOST) {
  console.log(
    chalk.cyan(
      `Attempting to bind to HOST environment variable: ${chalk.yellow(
        chalk.bold(process.env.HOST)
      )}`
    )
  );
  console.log(
    `If this was unintentional, check that you haven't mistakenly set it in your shell.`
  );
  console.log(
    `Learn more here: ${chalk.yellow('https://bit.ly/CRA-advanced-config')}`
  );
  console.log();
}

// We require that you explicitly set browsers and do not fall back to
// browserslist defaults.
checkBrowsers(paths.appPath, isInteractive)
  .then(() => {
    // We attempt to use the default port but if it is busy, we offer the user to
    // run on a different port. `choosePort()` Promise resolves to the next free port.
    return choosePort(HOST, DEFAULT_PORT);
  })
  .then(port => {
    if (port == null) {
      // We have not found a port.
      return;
    }

    const protocol = process.env.HTTPS === 'true' ? 'https' : 'http';
    const appName = require(paths.appPackageJson).name;
    const urls = prepareUrls(
      protocol,
      HOST,
      port,
      paths.publicUrlOrPath.slice(0, -1)
    );

    // Start the development server
    startDevServer(port, urls, protocol);
  })
  .catch(err => {
    if (err && err.message) {
      console.log(err.message);
    }
    process.exit(1);
  });

async function startDevServer(port, urls, protocol) {
  console.log(chalk.cyan('Starting the development server...\n'));

  let rsbuild;
  try {
    const configPath = require.resolve('../config/rsbuild/rsbuild.config.js');
    const rsbuildConfig = require(configPath);
    rsbuild = await createRsbuild({
      cwd: paths.appPath,
      rsbuildConfig,
    });
  } catch (err) {
    console.log(chalk.red('Failed to load Rsbuild config.\n'));
    throw err;
  }

  try {
    // Override the port from the config by modifying the loaded config
    const { server } = await rsbuild.startDevServer({
      port,
    });

    ['SIGINT', 'SIGTERM'].forEach(function (sig) {
      process.on(sig, async function () {
        await server.close();
        process.exit();
      });
    });

    if (process.env.CI !== 'true') {
      // Gracefully exit when stdin ends
      process.stdin.on('end', async function () {
        await server.close();
        process.exit();
      });
    }

    console.log(chalk.green('Compiled successfully!'));
    console.log();
    console.log(`You can now view the app in the browser.`);
    console.log();

    if (urls.lanUrlForTerminal) {
      console.log(
        `  ${chalk.bold('Local:')}            ${urls.localUrlForTerminal}`
      );
      console.log(
        `  ${chalk.bold('On Your Network:')}  ${urls.lanUrlForTerminal}`
      );
    } else {
      console.log(`  ${urls.localUrlForTerminal}`);
    }

    console.log();
    console.log('Note that the development build is not optimized.');
    console.log(
      `To create a production build, use ` +
        `${chalk.cyan(`${useYarn ? 'yarn' : 'npm run'} build:frontend:rsbuild`)}.`
    );
    console.log();

    openBrowser(urls.localUrlForBrowser);
  } catch (err) {
    console.log(chalk.red('Failed to start development server.\n'));
    console.error(err);
    process.exit(1);
  }
}