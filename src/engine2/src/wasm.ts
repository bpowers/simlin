// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access

let wasmInstance: WebAssembly.Instance | null = null;
let wasmMemory: WebAssembly.Memory | null = null;

/**
 * Initialize the WASM module.
 * Must be called before any other functions.
 */
export async function init(wasmPath?: string): Promise<void> {
  if (wasmInstance !== null) {
    return; // Already initialized
  }

  const path = wasmPath ?? './core/libsimlin.wasm';

  // Load WASM module
  const response = await fetch(path);
  const buffer = await response.arrayBuffer();
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
 * Reset the WASM state (for testing).
 */
export function reset(): void {
  wasmInstance = null;
  wasmMemory = null;
}
