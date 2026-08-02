// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defineConfig } from '@rsbuild/core';
import { pluginReact } from '@rsbuild/plugin-react';

// `assetPrefix: './'` (Vite's `base: './'`) produces relative asset URLs in
// `index.html` so the SPA can be served from any subpath (for example, when
// the Rust binary embeds the bundle under `/`, or when an HTTP proxy mounts
// it elsewhere).
//
// The flat `distPath` layout below is load-bearing for that relative prefix:
// Rspack builds runtime asset URLs as `publicPath + <output-relative path>`,
// resolved against whatever the referencing context's base URL is -- the
// document for the main thread, but the *worker script's own URL* inside the
// engine's Web Worker, and the CSS file for `url()` references. A relative
// publicPath is therefore only self-consistent when every JS chunk (and the
// assets they reference) sits at the output root, exactly like the flat
// `assets/` directory Vite emitted. Nesting chunks under `static/js/` would
// make the worker resolve the WASM blob to `static/js/static/wasm/...`.
//
// `asyncWebAssembly` replaces `vite-plugin-wasm` + `vite-plugin-top-level-await`:
// `@simlin/engine`'s browser entry imports `libsimlin-browser.wasm` via the
// "ESM integration proposal for Wasm" syntax, which Rspack supports natively
// as async modules (same setup as src/app's rsbuild config).
export default defineConfig({
  plugins: [pluginReact()],
  source: {
    entry: {
      index: './src/main.tsx',
    },
  },
  html: {
    template: './index.html',
  },
  output: {
    assetPrefix: './',
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
    },
  },
});
