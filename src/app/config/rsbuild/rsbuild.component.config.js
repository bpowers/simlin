const { defineConfig, mergeRsbuildConfig } = require('@rsbuild/core');
const { sharedConfig } = require('./shared.config');
const path = require('path');
const fs = require('fs');

const appDirectory = fs.realpathSync(process.cwd());
const resolveApp = (relativePath) => path.resolve(appDirectory, relativePath);

const isProduction = process.env.NODE_ENV === 'production';

module.exports = mergeRsbuildConfig(
  sharedConfig,
  defineConfig({
    source: {
      entry: {
        'sd-component': resolveApp('index-component.tsx'),
      },
    },
    output: {
      distPath: {
        root: resolveApp('build-component'),
      },
      filename: {
        // Fixed filename for web component to ensure predictable embedding
        js: 'static/js/[name].js',
        css: 'static/css/[name].css',
      },
      // Disable content hash for web component
      filenameHash: false,
    },
    html: {
      // Web component doesn't need HTML output
      template: undefined,
    },
    performance: {
      // Force single chunk for web component
      chunkSplit: {
        strategy: 'all-in-one',
      },
    },
    tools: {
      rspack: (config, { mergeConfig }) => {
        return mergeConfig(config, {
          optimization: {
            // Ensure single bundle for web component
            runtimeChunk: false,
            splitChunks: false,
          },
          output: {
            // Ensure the web component can determine its own base URL
            ...config.output,
            library: {
              type: 'umd',
              name: 'SDComponent',
            },
          },
          plugins: [
            // Add webpack.optimize.LimitChunkCountPlugin equivalent
            // In Rspack/Rsbuild, this is handled by setting splitChunks: false
          ],
        });
      },
    },
  })
);