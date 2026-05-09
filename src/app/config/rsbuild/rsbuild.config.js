const { defineConfig, mergeRsbuildConfig } = require('@rsbuild/core');
const { sharedConfig } = require('./shared.config');
const path = require('path');
const fs = require('fs');

const appDirectory = fs.realpathSync(process.cwd());
const resolveApp = (relativePath) => path.resolve(appDirectory, relativePath);

const isProduction = process.env.NODE_ENV === 'production';
const shouldInlineRuntimeChunk = process.env.INLINE_RUNTIME_CHUNK !== 'false';

module.exports = mergeRsbuildConfig(
  sharedConfig,
  defineConfig({
    source: {
      entry: {
        index: resolveApp('index.tsx'),
      },
    },
    output: {
      distPath: {
        root: resolveApp('build'),
      },
    },
    html: {
      inject: 'body',
      // CSP is set by Express helmet in src/server/app.ts on every
      // dynamic route (notably /:username/:projectName, the SPA's
      // primary entry). A meta tag here would intersect per-directive
      // with the helmet header on those routes -- and the two are
      // hard to keep in sync, so the previous meta tag silently
      // blocked Firebase auth iframes, blob: scripts, and
      // apis.google.com once helmet's stricter directives diverged.
      // The few pages served as App Engine static files (/, /new,
      // /legal*, /privacy in app.yaml) load the same SPA bundle from
      // /static/js/* with no inline scripts, so they are not a
      // material XSS vector even without a CSP.
    },
    performance: {
      chunkSplit: {
        strategy: 'all-in-one',
        // This creates a single JS bundle with everything
        // Better for apps where all code is needed on initial load
      },
      bundleAnalyze: process.env.ANALYZE === 'true' ? {} : undefined,
    },
    tools: {
      rspack: (config, { mergeConfig }) => {
        return mergeConfig(config, {
          optimization: {
            runtimeChunk: shouldInlineRuntimeChunk ? false : 'single',
            splitChunks: config.optimization?.splitChunks,
          },
          plugins: [
            // Add any additional plugins needed for main app
            // Note: Many webpack plugins are not needed in Rsbuild as they're built-in
          ],
        });
      },
    },
  })
);