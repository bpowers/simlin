const { defineConfig } = require('@rsbuild/core');
const { pluginReact } = require('@rsbuild/plugin-react');
const { pluginTypeCheck } = require('@rsbuild/plugin-type-check');
const path = require('path');
const fs = require('fs');

// Get paths similar to the webpack config
const appDirectory = fs.realpathSync(process.cwd());
const resolveApp = (relativePath) => path.resolve(appDirectory, relativePath);

// Get environment variables
const NODE_ENV = process.env.NODE_ENV || 'development';
const isProduction = NODE_ENV === 'production';
const isDevelopment = NODE_ENV === 'development';
const shouldUseSourceMap = process.env.GENERATE_SOURCEMAP !== 'false';

// Public URL handling
const PUBLIC_URL = process.env.PUBLIC_URL || '';


const sharedConfig = defineConfig({
  plugins: [
    pluginReact({
      fastRefresh: isDevelopment && process.env.FAST_REFRESH !== 'false',
    }),
    pluginTypeCheck({
      typescript: {
        configFile: resolveApp('tsconfig.browser.json'),
        // Similar to ForkTsCheckerWebpackPlugin behavior
        memoryLimit: 2048,
        async: isDevelopment,
      },
    }),
    // Commenting out Babel plugin for now to avoid conflicts with built-in transforms
    // pluginBabel({
    //   babelLoaderOptions: {
    //     presets: [
    //       [
    //         require.resolve('babel-preset-react-app'),
    //         {
    //           runtime: 'automatic',
    //         },
    //       ],
    //     ],
    //     cacheDirectory: true,
    //     cacheCompression: false,
    //     compact: isProduction,
    //   },
    // }),
  ],
  source: {
    define: {
      'process.env.NODE_ENV': JSON.stringify(NODE_ENV),
      'process.env.PUBLIC_URL': JSON.stringify(PUBLIC_URL),
      // Add other environment variables as needed
    },
  },
  html: {
    template: resolveApp('public/index.html'),
    templateParameters: {
      PUBLIC_URL: PUBLIC_URL || '',
    },
    inject: 'body',
  },
  output: {
    publicPath: PUBLIC_URL ? `${PUBLIC_URL}/` : '/',
    filename: {
      js: isProduction ? '[name].[contenthash:8].js' : '[name].js',
      css: isProduction ? '[name].[contenthash:8].css' : '[name].css',
    },
    distPath: {
      js: 'static/js',
      css: 'static/css',
      wasm: 'static/wasm',
      image: 'static/media',
      font: 'static/media',
      media: 'static/media',
    },
    sourceMap: {
      js: shouldUseSourceMap ? (isProduction ? 'source-map' : 'cheap-module-source-map') : false,
      css: shouldUseSourceMap,
    },
  },
  server: {
    port: 3000,
    publicDir: {
      name: resolveApp('../../public'),
      watch: true,
    },
    htmlFallback: 'index',
    printUrls: ({ urls }) => {
      return urls.map(url => url.replace('/index', ''));
    },
    proxy: [
      {
        context: (pathname) => {
          // Only proxy specific API endpoints to backend
          // API endpoints that should be proxied
          if (pathname.startsWith('/api/') ||
              pathname.startsWith('/auth/') ||
              pathname.startsWith('/oauth/') ||
              pathname.startsWith('/logout') ||
              pathname.startsWith('/render/') ||
              pathname.startsWith('/session') ||
              pathname.startsWith('/download/')) {
            return true;
          }
          // Everything else (including user/project paths) should be handled by the SPA
          return false;
        },
        target: 'http://localhost:3030',
        changeOrigin: true,
        logLevel: 'debug',
      },
    ],
    historyApiFallback: true,
  },
  performance: {
    removeConsole: isProduction && !shouldUseSourceMap,
  },
  tools: {
    rspack: {
      experiments: {
        asyncWebAssembly: true,
      },
      module: {
        rules: [
          {
            test: /\.wasm$/,
            type: 'webassembly/async',
          },
        ],
      },
      resolve: {
        extensions: ['.web.mjs', '.mjs', '.web.js', '.js', '.web.ts', '.ts', '.web.tsx', '.tsx', '.json', '.web.jsx', '.jsx'],
        alias: {
          '@': resolveApp('.'),
          '@system-dynamics/core': resolveApp('../core'),
          '@system-dynamics/diagram': resolveApp('../diagram'),
          '@system-dynamics/engine2': resolveApp('../engine2'),
          '@system-dynamics/xmutil': resolveApp('../xmutil-js'),
        },
      },
    },
  },
});

module.exports = { sharedConfig };
