'use strict';

const path = require('path');
const fs = require('fs');
const { URL } = require('url');

// Make sure any symlinks in the project folder are resolved:
const appDirectory = fs.realpathSync(process.cwd());
const resolveApp = relativePath => path.resolve(appDirectory, relativePath);

/**
 * Returns a URL or a path with a trailing slash.
 * In production can be URL, absolute path, or relative path.
 * In development always will be an absolute path.
 */
function getPublicUrlOrPath(isEnvDevelopment, homepage, envPublicUrl) {
  const stubDomain = 'https://localhost';

  if (envPublicUrl) {
    envPublicUrl = envPublicUrl.endsWith('/') ? envPublicUrl : envPublicUrl + '/';
    const validPublicUrl = new URL(envPublicUrl, stubDomain);
    return isEnvDevelopment
      ? envPublicUrl.startsWith('.') ? '/' : validPublicUrl.pathname
      : envPublicUrl;
  }

  if (homepage) {
    homepage = homepage.endsWith('/') ? homepage : homepage + '/';
    const validHomepagePathname = new URL(homepage, stubDomain).pathname;
    return isEnvDevelopment
      ? homepage.startsWith('.') ? '/' : validHomepagePathname
      : homepage.startsWith('.') ? homepage : validHomepagePathname;
  }

  return '/';
}

const publicUrlOrPath = getPublicUrlOrPath(
  process.env.NODE_ENV === 'development',
  require(resolveApp('package.json')).homepage,
  process.env.PUBLIC_URL
);

const buildPath = process.env.BUILD_PATH || 'build';

const moduleFileExtensions = [
  'web.mjs',
  'mjs',
  'web.js',
  'js',
  'web.ts',
  'ts',
  'web.tsx',
  'tsx',
  'json',
  'web.jsx',
  'jsx',
];

// Resolve file paths in the same order as the bundler
const resolveModule = (resolveFn, filePath) => {
  const extension = moduleFileExtensions.find(extension =>
    fs.existsSync(resolveFn(`${filePath}.${extension}`))
  );

  if (extension) {
    return resolveFn(`${filePath}.${extension}`);
  }

  return resolveFn(`${filePath}.js`);
};

module.exports = {
  dotenv: resolveApp('.env'),
  appPath: resolveApp('.'),
  appBuild: resolveApp(buildPath),
  componentBuild: resolveApp('build-component'),
  appPublic: resolveApp('../../public'),
  appHtml: resolveApp('public/index.html'),
  appIndexJs: resolveModule(resolveApp, 'index'),
  componentIndexJs: resolveModule(resolveApp, 'index-component'),
  appPackageJson: resolveApp('package.json'),
  appSrc: resolveApp('.'),
  appTsConfig: resolveApp('tsconfig.browser.json'),
  yarnLockFile: resolveApp('yarn.lock'),
  appNodeModules: resolveApp('node_modules'),
  publicUrlOrPath,
};

module.exports.moduleFileExtensions = moduleFileExtensions;
