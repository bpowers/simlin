// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access (browser build)

// Import WASM as a module - with asyncWebAssembly enabled in the bundler,
// this import is handled automatically.
// The bundler loads the WASM, instantiates it, and returns the exports.
// @ts-expect-error TypeScript doesn't understand .wasm imports
import * as wasmModule from '../../core/libsimlin.wasm';

export type WasmSource = string | URL | ArrayBuffer | Uint8Array;
export type WasmSourceProvider = WasmSource | (() => WasmSource | Promise<WasmSource>);

export interface WasmConfig {
  source?: WasmSourceProvider;
}

let wasmExports: WebAssembly.Exports | null = null;
let wasmMemory: WebAssembly.Memory | null = null;
let initPromise: Promise<void> | null = null;

/**
 * Check if a string looks like a URL (http://, https://, or file://)
 * @internal Exported for testing
 */
export function isUrl(path: string): boolean {
  return path.startsWith('http://') || path.startsWith('https://') || path.startsWith('file://');
}

/**
 * Check if we're running in Node.js
 * @internal Exported for testing
 */
export function isNode(): boolean {
  return typeof process !== 'undefined' && process.versions?.node !== undefined;
}

/**
 * Load a file from the filesystem in Node.js.
 * @internal Exported for testing
 */
export async function loadFileNode(_pathOrUrl: string | URL): Promise<ArrayBuffer> {
  throw new Error('loadFileNode is not available in the browser build');
}

/**
 * Initialize the WASM module.
 * In browser builds with bundler support, this uses the pre-loaded WASM module.
 * The wasmPathOrBuffer parameter is ignored in browser builds since the bundler
 * handles WASM loading automatically.
 */
export async function init(_wasmPathOrBuffer?: WasmSourceProvider): Promise<void> {
  if (wasmExports !== null) {
    return; // Already initialized
  }

  // The bundler has already loaded and instantiated the WASM module.
  // wasmModule contains the exports directly.
  wasmExports = wasmModule as unknown as WebAssembly.Exports;

  // Get memory from exports if available
  if (wasmExports.memory instanceof WebAssembly.Memory) {
    wasmMemory = wasmExports.memory;
  }

  // Install the Rust panic hook so panic messages are captured in a
  // global buffer rather than silently lost to `unreachable` traps.
  const initFn = wasmExports.simlin_init as (() => void) | undefined;
  if (initFn) {
    initFn();
  }
}

/**
 * Get the raw WASM exports.
 * @throws Error if WASM is not initialized
 */
export function getExports(): WebAssembly.Exports {
  if (wasmExports === null) {
    throw new Error('WASM not initialized. Call Project.open() or ready() first.');
  }
  return wasmExports;
}

/**
 * Get the WASM memory instance.
 * @throws Error if WASM is not initialized
 */
export function getMemory(): WebAssembly.Memory {
  if (wasmMemory === null) {
    throw new Error('WASM not initialized. Call Project.open() or ready() first.');
  }
  return wasmMemory;
}

/**
 * Check if the WASM module is initialized.
 */
export function isInitialized(): boolean {
  return wasmExports !== null;
}

/**
 * Ensure the WASM module is initialized.
 * This is a convenience function that will initialize WASM with default settings
 * if it hasn't been initialized yet. Safe to call multiple times.
 *
 * @param wasmSource - Ignored in browser builds (bundler handles WASM loading).
 */
export async function ensureInitialized(wasmSource?: WasmSourceProvider): Promise<void> {
  if (wasmExports !== null) {
    return;
  }

  if (initPromise !== null) {
    await initPromise;
    return;
  }

  initPromise = init(wasmSource);
  try {
    await initPromise;
  } finally {
    initPromise = null;
  }
}

/**
 * Configure WASM source. In browser builds, this is a no-op since the bundler
 * handles WASM loading automatically.
 */
export function configureWasm(_config: WasmConfig = {}): void {
  // No-op in browser builds - bundler handles WASM loading
}

/**
 * Retrieve the last Rust panic message from the WASM global buffer.
 * Returns null if no panic has been recorded or WASM is not initialized.
 *
 * Call this after catching a `RuntimeError: unreachable` to get the
 * actual panic text (file, line, message) instead of just "unreachable".
 */
export function getPanicMessage(): string | null {
  if (wasmExports === null || wasmMemory === null) {
    return null;
  }
  const fn = wasmExports.simlin_get_panic_message as (() => number) | undefined;
  if (!fn) {
    return null;
  }
  const ptr = fn();
  if (ptr === 0) {
    return null;
  }
  // Read null-terminated UTF-8 string from WASM memory
  const view = new Uint8Array(wasmMemory.buffer);
  let end = ptr;
  const limit = Math.min(ptr + 8192, view.length);
  while (end < limit && view[end] !== 0) {
    end++;
  }
  return new TextDecoder().decode(view.slice(ptr, end));
}

/**
 * Clear the stored panic message.
 */
export function clearPanicMessage(): void {
  if (wasmExports === null) {
    return;
  }
  const fn = wasmExports.simlin_clear_panic_message as (() => void) | undefined;
  if (fn) {
    fn();
  }
}

/**
 * Reset the WASM state (for testing).
 */
export function reset(): void {
  wasmExports = null;
  wasmMemory = null;
  initPromise = null;
}
