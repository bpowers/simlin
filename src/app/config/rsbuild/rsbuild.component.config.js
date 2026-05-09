const { defineConfig, mergeRsbuildConfig } = require('@rsbuild/core');
const { sharedConfig } = require('./shared.config');
const path = require('path');
const fs = require('fs');
const { createRequire } = require('module');

// `@rspack/core` is bundled with rsbuild but not declared as a direct
// devDependency of @simlin/app, so a plain `require('@rspack/core')` fails
// under pnpm's strict resolution.  Resolve it through rsbuild's own
// node_modules so the version always matches what rsbuild uses internally.
const rspack = createRequire(require.resolve('@rsbuild/core'))('@rspack/core');

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
        // Fixed filename (no content hash -- see filenameHash below) so the
        // embed URL is stable: <script src="/static/js/sd-component.js">.
        // The static/js/ (and static/css/) directory prefix comes from
        // distPath.js / distPath.css in shared.config.js; do NOT repeat it
        // here or the output nests as static/js/static/js/sd-component.js
        // and every external embed 404s.
        js: '[name].js',
        css: '[name].css',
      },
      // Disable content hash for web component
      filenameHash: false,
      // Make Rspack/Webpack compute the publicPath at runtime from the
      // script's own URL (`document.currentScript.src`), not from
      // `document.baseURI`.  Without this the worker chunk URL
      // (`new Worker(new URL(publicPath + chunkUrl, baseURI))`) resolves
      // against the embedding page's origin -- so a third-party site that
      // includes `<script src="https://app.simlin.com/static/js/sd-component.js">`
      // would try to load the worker from its own origin and 404.
      // With `'auto'`, `publicPath` becomes `https://app.simlin.com/static/js/`
      // (computed from the script src), and the worker URL resolves correctly.
      assetPrefix: 'auto',
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
            // CRITICAL for the cross-origin embed contract.
            //
            // Embedders load just `<script src="https://app.simlin.com/static/js/sd-component.js">`
            // from a third-party origin. They do NOT mirror our `static/js/async/*` directory.
            // `splitChunks: false` only disables the SplitChunksPlugin (vendor/common
            // grouping); it does NOT collapse async chunks created by dynamic `import()`.
            // Two such chunks exist in the engine package: the worker entry created via
            // `new Worker(new URL('./engine-worker.js', import.meta.url))` and, inside that
            // worker, `import('./worker-server')` (intentionally dynamic so the worker can
            // install `self.onmessage` synchronously before WASM finishes loading -- see
            // src/engine/src/engine-worker.ts).  Without merging, embeds 404 on first
            // worker init.
            //
            // `LimitChunkCountPlugin({ maxChunks: 1 })` forces every chunk in each
            // compilation (the main bundle AND the worker compilation) to merge into a
            // single output file -- i.e. one `sd-component.js` and one worker bundle.
            // The worker bundle URL is computed at runtime from `import.meta.url`, so it
            // resolves relative to the script's actual origin and works cross-origin.
            //
            // The pre-rsbuild webpack config relied on this same plugin; the rsbuild
            // migration silently dropped it.  Do not remove without restoring an
            // equivalent guarantee (and updating verify-deploy-build.sh).
            new rspack.optimize.LimitChunkCountPlugin({ maxChunks: 1 }),
          ],
        });
      },
    },
  })
);