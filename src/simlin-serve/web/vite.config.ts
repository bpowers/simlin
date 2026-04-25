// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

// `base: './'` produces relative asset URLs in `index.html` so the SPA can be
// served from any subpath (for example, when the Rust binary embeds the
// bundle under `/`, or when an HTTP proxy mounts it elsewhere).
//
// `vite-plugin-wasm` + `vite-plugin-top-level-await` are needed because
// `@simlin/engine`'s browser entry imports `libsimlin.wasm` directly via the
// "ESM integration proposal for Wasm" syntax. Without these plugins, Rollup
// fails the build the moment the engine module is touched.
export default defineConfig({
  base: './',
  plugins: [react(), wasm(), topLevelAwait()],
  worker: {
    format: 'es',
    plugins: () => [wasm(), topLevelAwait()],
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'esnext',
  },
});
