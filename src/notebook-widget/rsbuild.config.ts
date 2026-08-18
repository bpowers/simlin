// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Builds `dist/widget.js`: ONE self-contained ES module that anywidget can
 * load from a `blob:` URL. That hosting model dictates every choice below:
 *
 * - `import.meta.url` is a blob: URL and there is no asset directory next to
 *   the module, so nothing may be fetched relative to the bundle: no separate
 *   worker chunk, no wasm asset, no CSS file, no font files. CSS is injected
 *   into the document from JS (`injectStyles`), fonts are data URIs, and the
 *   engine runs on the main thread (`backend-factory.direct`) with a wasm
 *   flavor that bundles no artifact (`wasm.supplied`) -- the kernel sends the
 *   bytes over the widget comm at runtime (see src/engine-bootstrap.ts).
 * - anywidget imports the module and reads `default.{initialize, render}`, so
 *   the output must be a real ES module with a default export
 *   (`output.module` + rspack `library.type: 'module'`), and everything must
 *   be in that one file (`splitChunks: false` + LimitChunkCountPlugin(1)).
 */

import { defineConfig, rspack } from '@rsbuild/core';
import { pluginReact } from '@rsbuild/plugin-react';

import { scopeCssPlugin } from './build/scope-css';
import { WIDGET_ROOT_CLASS } from './src/widget-root-class';

// Which engine backend to bundle. 'direct' (default) runs libsimlin on the
// main thread; 'worker' keeps @simlin/engine's Web Worker backend and is here
// only so the two can be measured against each other (its worker chunk is a
// SECOND file, which anywidget cannot load -- see CLAUDE.md for the numbers
// and the decision).
const backend = process.env.SIMLIN_WIDGET_BACKEND === 'worker' ? 'worker' : 'direct';

export default defineConfig({
  plugins: [pluginReact()],
  source: {
    entry: {
      widget: './src/index.tsx',
    },
  },
  resolve: {
    alias: {
      // The two platform-specific engine modules. `$` = exact match, so only
      // the specifiers themselves are redirected. The targets are ordinary
      // package subpaths (the engine's "./*" export -> lib.browser/*.js).
      '@simlin/engine/internal/wasm$': '@simlin/engine/internal/wasm.supplied',
      ...(backend === 'direct'
        ? { '@simlin/engine/internal/backend-factory$': '@simlin/engine/backend-factory.direct' }
        : {}),
    },
  },
  output: {
    target: 'web',
    // ES module output (import/export, chunkFormat 'module').
    module: true,
    distPath: {
      root: 'dist',
      js: '',
      jsAsync: '',
      css: '',
      cssAsync: '',
      wasm: '',
      font: '',
      image: '',
      svg: '',
      media: '',
    },
    filename: {
      js: '[name].js',
    },
    filenameHash: false,
    // CSS becomes <style> tags appended by the bundle at import time instead
    // of a sibling .css file the host would have to know about.
    injectStyles: true,
    // Fonts are inlined; the generator below decides WHICH (woff2 only).
    dataUriLimit: { font: Number.MAX_SAFE_INTEGER },
    // A sibling LICENSE.txt is a second file too; keep the notices inline.
    legalComments: 'inline',
    sourceMap: { js: false, css: false },
  },
  // No HTML shell: the module is loaded by anywidget, not a page of ours.
  tools: {
    htmlPlugin: false,
    // css-loader's ES-module output turns every url() into
    // `new URL(asset, __webpack_require__.b)`, and rspack's ESM runtime defines
    // that base as `new URL('./', import.meta.url)` at module init -- which
    // THROWS for a blob: URL (blob URLs are not hierarchical), before a single
    // line of the widget runs. Every url() here is a data: URI that needs no
    // base, so the CommonJS-shaped css-loader output (plain strings, no URL
    // construction) is the correct form, not merely a workaround.
    cssLoader: { esModule: false },
    // Global stylesheets (theme.css tokens, katex) are confined to the widget
    // root so the notebook page is untouched; see build/scope-css.ts.
    postcss: (_opts, { addPlugins }) => {
      addPlugins(scopeCssPlugin(WIDGET_ROOT_CLASS));
    },
    bundlerChain: (chain, { CHAIN_ID }) => {
      // katex.min.css declares every face three times (woff2, woff, ttf) and
      // browsers take the first supported source, which is always woff2 in
      // any browser that runs this widget. Inlining all three would triple
      // the font payload (~1.2 MB raw before base64) for bytes that are never
      // read, so woff2 becomes a real data URI and the fallbacks become an
      // empty one -- the declaration stays valid, the browser never gets past
      // the woff2 entry. Rspack's dataUrlCondition is size-only, so the
      // per-extension decision has to live in the generator.
      chain.module
        .rule(CHAIN_ID.RULE.FONT)
        .oneOf(`${CHAIN_ID.RULE.FONT}-asset`)
        .set('generator', {
          dataUrl: (content: Buffer, { filename }: { filename: string }) =>
            filename.endsWith('.woff2') ? `data:font/woff2;base64,${content.toString('base64')}` : 'data:,',
        });
    },
    rspack: (config, { mergeConfig }) =>
      mergeConfig(config, {
        output: {
          library: { type: 'module' },
        },
        optimization: {
          runtimeChunk: false,
          splitChunks: false,
        },
        plugins: [
          // Async chunks from dynamic import() are not covered by
          // splitChunks: false; force every chunk of the main compilation into
          // the single output file.
          new rspack.optimize.LimitChunkCountPlugin({ maxChunks: 1 }),
        ],
      }),
  },
});
