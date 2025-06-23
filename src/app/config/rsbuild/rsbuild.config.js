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
      // Content Security Policy will be added via plugin if needed
      meta: isProduction ? {
        'Content-Security-Policy': {
          'http-equiv': 'Content-Security-Policy',
          content: process.env.CSP_CONTENT || "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self' https://api.simlin.com wss://api.simlin.com https://firestore.googleapis.com https://securetoken.googleapis.com https://identitytoolkit.googleapis.com https://www.googleapis.com; worker-src 'self' blob:;",
        },
      } : undefined,
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