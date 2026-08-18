// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access (host-supplied artifact, no bundled default).
//
// For bundles that must be a single self-contained JavaScript file with no
// relative asset fetches -- the notebook widget, whose module is loaded from
// a blob: URL and receives the wasm bytes over the kernel comm. Nothing here
// references the .wasm artifact, so a bundler that selects this flavor emits
// no wasm asset and no fetch of one; the host MUST hand the wasm to
// `ready()` / `Project.open*({ wasm })` / `configureWasm({ source })` as
// bytes, a precompiled `WebAssembly.Module`, or a URL, and `init()` with
// nothing to go on fails loudly rather than guessing a path.
//
// Select it with a bundler alias for the exact specifier
// `@simlin/engine/internal/wasm` (see src/notebook-widget/rsbuild.config.ts).
// The shared singleton and instantiation logic live in ./wasm-runtime.ts.

import {
  ensureInitializedWith,
  instantiateFromSource,
  isInitialized,
  resolveSourceProvider,
  type WasmSourceProvider,
} from './wasm-runtime';

export {
  clearPanicMessage,
  configureWasm,
  getExports,
  getMemory,
  getPanicMessage,
  isInitialized,
  isUrl,
  reset,
} from './wasm-runtime';
export type { WasmConfig, WasmSource, WasmSourceProvider } from './wasm-runtime';

/**
 * Initialize the WASM module from a host-supplied source.
 * @param wasmSource - Bytes, a precompiled module, or a URL (or a provider
 *                     returning one). Required unless `configureWasm` set one.
 * @throws Error when no source is available: this build bundles no artifact.
 */
export async function init(wasmSource?: WasmSourceProvider): Promise<void> {
  if (isInitialized()) {
    return;
  }

  const resolved = await resolveSourceProvider(wasmSource);
  if (resolved === undefined) {
    throw new Error(
      '@simlin/engine: no WASM source supplied and this build bundles no libsimlin artifact; ' +
        'pass the wasm bytes, a WebAssembly.Module, or a URL to ready() or configureWasm().',
    );
  }
  await instantiateFromSource(resolved);
}

/**
 * Ensure the WASM module is initialized. Safe to call multiple times;
 * concurrent callers share one initialization.
 */
export function ensureInitialized(wasmSource?: WasmSourceProvider): Promise<void> {
  return ensureInitializedWith(init, wasmSource);
}
