// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access

export type WasmSource = string | URL | ArrayBuffer | Uint8Array;
export type WasmSourceProvider = WasmSource | (() => WasmSource | Promise<WasmSource>);

export interface WasmConfig {
  source?: WasmSourceProvider;
}

let wasmInstance: WebAssembly.Instance | null = null;
let wasmMemory: WebAssembly.Memory | null = null;
let initPromise: Promise<void> | null = null;
let wasmSourceOverride: WasmSourceProvider | null = null;

/**
 * Check if a string looks like a URL (http://, https://, or file://)
 * @internal Exported for testing
 */
export function isUrl(path: string): boolean {
  return path.startsWith('http://') || path.startsWith('https://') || path.startsWith('file://');
}

function isFileUrl(path: string): boolean {
  return path.startsWith('file://');
}

/**
 * Check if we're running in Node.js
 * @internal Exported for testing
 */
export function isNode(): boolean {
  return typeof process !== 'undefined' && process.versions?.node !== undefined;
}

async function getDefaultNodeWasmPath(): Promise<string> {
  const path = await import('node:path');
  return path.join(__dirname, '..', 'core', 'libsimlin.wasm');
}

function getDefaultBrowserWasmUrl(): string {
  if (typeof document !== 'undefined') {
    const currentScript = document.currentScript;
    const scriptUrl = currentScript && 'src' in currentScript ? (currentScript as HTMLScriptElement).src : undefined;
    const base = document.baseURI ?? scriptUrl ?? getLocationHref() ?? '';
    if (base) {
      return new URL('core/libsimlin.wasm', base).toString();
    }
  }
  const locationHref = getLocationHref();
  if (locationHref) {
    return new URL('core/libsimlin.wasm', locationHref).toString();
  }
  return './core/libsimlin.wasm';
}

function getLocationHref(): string | undefined {
  if (typeof globalThis === 'undefined' || !('location' in globalThis)) {
    return undefined;
  }
  return (globalThis as { location?: Location }).location?.href;
}

async function resolveWasmSource(source?: WasmSourceProvider): Promise<WasmSource> {
  const provider = source ?? wasmSourceOverride;
  if (provider !== undefined && provider !== null) {
    return typeof provider === 'function' ? await provider() : provider;
  }
  return isNode() ? await getDefaultNodeWasmPath() : getDefaultBrowserWasmUrl();
}

/**
 * Load a file from the filesystem in Node.js.
 * @internal Exported for testing
 */
export async function loadFileNode(pathOrUrl: string | URL): Promise<ArrayBuffer> {
  const fs = await import('node:fs/promises');
  const nodeBuffer = await fs.readFile(pathOrUrl);
  return nodeBuffer.buffer.slice(nodeBuffer.byteOffset, nodeBuffer.byteOffset + nodeBuffer.byteLength);
}

/**
 * Initialize the WASM module.
 * Must be called before any other functions.
 * @param wasmPathOrBuffer - Either a path/URL to the WASM file, or an ArrayBuffer/Uint8Array containing the WASM binary.
 *                           In browsers, paths are fetched as URLs. In Node.js, filesystem paths are read directly.
 */
export async function init(wasmPathOrBuffer?: WasmSourceProvider): Promise<void> {
  if (wasmInstance !== null) {
    return; // Already initialized
  }

  const resolvedSource = await resolveWasmSource(wasmPathOrBuffer);
  let buffer: ArrayBuffer;

  if (resolvedSource instanceof ArrayBuffer) {
    buffer = resolvedSource;
  } else if (resolvedSource instanceof Uint8Array) {
    // Copy to a new ArrayBuffer to handle SharedArrayBuffer case
    const copy = new Uint8Array(resolvedSource.length);
    copy.set(resolvedSource);
    buffer = copy.buffer;
  } else {
    const pathOrUrl = resolvedSource instanceof URL ? resolvedSource.toString() : resolvedSource;

    if (isNode() && (isFileUrl(pathOrUrl) || !isUrl(pathOrUrl))) {
      const fileTarget = isFileUrl(pathOrUrl) ? new URL(pathOrUrl) : pathOrUrl;
      buffer = await loadFileNode(fileTarget);
    } else {
      const response = await fetch(pathOrUrl);
      if (!response.ok) {
        throw new Error(`Failed to load WASM from ${pathOrUrl}: ${response.status} ${response.statusText}`);
      }
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
    throw new Error('WASM not initialized. Call Project.open() or ready() first.');
  }
  return wasmInstance.exports;
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
  return wasmInstance !== null;
}

/**
 * Ensure the WASM module is initialized.
 * This is a convenience function that will initialize WASM with default settings
 * if it hasn't been initialized yet. Safe to call multiple times.
 *
 * @param wasmSource - Optional WASM source or provider. Defaults to auto-detected
 *                     runtime settings for Node.js and browsers.
 */
export async function ensureInitialized(wasmSource?: WasmSourceProvider): Promise<void> {
  if (wasmInstance !== null) {
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

export function configureWasm(config: WasmConfig = {}): void {
  if (wasmInstance !== null || initPromise !== null) {
    throw new Error('WASM already initialized');
  }
  wasmSourceOverride = config.source ?? null;
}

/**
 * Reset the WASM state (for testing).
 */
export function reset(): void {
  wasmInstance = null;
  wasmMemory = null;
  initPromise = null;
  wasmSourceOverride = null;
}
