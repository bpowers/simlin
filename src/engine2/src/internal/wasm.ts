// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access

let wasmInstance: WebAssembly.Instance | null = null;
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
 * This function uses a dynamic import pattern that avoids bundler analysis.
 * Note: This approach uses new Function() to hide the import from bundlers,
 * which doesn't work in Jest's sandbox. Tests that call this function directly
 * should read the file with fs.readFileSync and pass the buffer to init() instead.
 * @internal Exported for testing
 */
export async function loadFileNode(path: string): Promise<ArrayBuffer> {
  // Use a technique that prevents bundlers from analyzing this import.
  // In Node.js, this will work. In browsers, this function should never be called.
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const dynamicImport = new Function('specifier', 'return import(specifier)');
  const fs = await dynamicImport('node:fs/promises');
  const nodeBuffer = await fs.readFile(path);
  return nodeBuffer.buffer.slice(nodeBuffer.byteOffset, nodeBuffer.byteOffset + nodeBuffer.byteLength);
}

/**
 * Initialize the WASM module.
 * Must be called before any other functions.
 * @param wasmPathOrBuffer - Either a path/URL to the WASM file, or an ArrayBuffer/Uint8Array containing the WASM binary.
 *                           In browsers, paths are fetched as URLs. In Node.js, filesystem paths are read directly.
 */
export async function init(wasmPathOrBuffer?: string | ArrayBuffer | Uint8Array): Promise<void> {
  if (wasmInstance !== null) {
    return; // Already initialized
  }

  let buffer: ArrayBuffer;

  if (wasmPathOrBuffer instanceof ArrayBuffer) {
    buffer = wasmPathOrBuffer;
  } else if (wasmPathOrBuffer instanceof Uint8Array) {
    // Copy to a new ArrayBuffer to handle SharedArrayBuffer case
    const copy = new Uint8Array(wasmPathOrBuffer.length);
    copy.set(wasmPathOrBuffer);
    buffer = copy.buffer;
  } else {
    const path = wasmPathOrBuffer ?? './core/libsimlin.wasm';

    // In Node.js, filesystem paths need to be read directly (fetch only works with URLs)
    if (isNode() && !isUrl(path)) {
      buffer = await loadFileNode(path);
    } else {
      const response = await fetch(path);
      buffer = await response.arrayBuffer();
    }
  }

  const module = await WebAssembly.compile(buffer);

  // Create memory - libsimlin manages its own memory
  wasmMemory = new WebAssembly.Memory({ initial: 256, maximum: 16384 });

  // Instantiate with imports
  wasmInstance = await WebAssembly.instantiate(module, {
    env: {
      memory: wasmMemory,
    },
  });

  // The WASM module may export its own memory
  const exports = wasmInstance.exports;
  if (exports.memory instanceof WebAssembly.Memory) {
    wasmMemory = exports.memory;
  }
}

/**
 * Get the raw WASM exports.
 * @throws Error if WASM is not initialized
 */
export function getExports(): WebAssembly.Exports {
  if (wasmInstance === null) {
    throw new Error('WASM not initialized. Call init() first.');
  }
  return wasmInstance.exports;
}

/**
 * Get the WASM memory instance.
 * @throws Error if WASM is not initialized
 */
export function getMemory(): WebAssembly.Memory {
  if (wasmMemory === null) {
    throw new Error('WASM not initialized. Call init() first.');
  }
  return wasmMemory;
}

/**
 * Check if the WASM module is initialized.
 */
export function isInitialized(): boolean {
  return wasmInstance !== null;
}

/**
 * Ensure the WASM module is initialized.
 * This is a convenience function that will initialize WASM with default settings
 * if it hasn't been initialized yet. Safe to call multiple times.
 *
 * @param wasmPath - Optional path to the WASM file. Defaults to './core/libsimlin.wasm'
 *                   which works for both Node.js (relative to engine2 package) and
 *                   browsers (if the WASM is served at that path).
 */
export async function ensureInitialized(wasmPath?: string): Promise<void> {
  if (wasmInstance !== null) {
    return;
  }

  if (initPromise !== null) {
    await initPromise;
    return;
  }

  initPromise = init(wasmPath);
  try {
    await initPromise;
  } finally {
    initPromise = null;
  }
}

/**
 * Reset the WASM state (for testing).
 */
export function reset(): void {
  wasmInstance = null;
  wasmMemory = null;
  initPromise = null;
}
