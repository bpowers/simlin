// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access (browser build with a bundled artifact).
//
// The bundler (asyncWebAssembly) loads and instantiates the slim
// `libsimlin-browser.wasm` as part of importing this module, so a host that
// passes no source gets that artifact. A host that DOES pass a source --
// bytes, a precompiled `WebAssembly.Module`, or a URL -- gets exactly what it
// passed; the bundled artifact is then simply unused. The shared singleton
// and instantiation logic live in ./wasm-runtime.ts.
//
// The browser artifact is the slim build (no png_render: the resvg/text
// shaping stack is ~28% of the full binary and only Node-side PNG previews
// use it); Node loads the full libsimlin.wasm via wasm.node.ts instead.
// @ts-expect-error TypeScript doesn't understand .wasm imports
import * as wasmModule from '../../core/libsimlin-browser.wasm';

import {
  adoptInstance,
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
 * Initialize the WASM module.
 * @param wasmSource - Optional bytes, precompiled module, or URL. When omitted
 *                     the bundler-instantiated artifact is used.
 */
export async function init(wasmSource?: WasmSourceProvider): Promise<void> {
  if (isInitialized()) {
    return;
  }

  const resolved = await resolveSourceProvider(wasmSource);
  if (resolved === undefined) {
    // The bundler has already instantiated the module; wasmModule holds its
    // exports directly.
    adoptInstance(wasmModule as unknown as WebAssembly.Exports);
    return;
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
