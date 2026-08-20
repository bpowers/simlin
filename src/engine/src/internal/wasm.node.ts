// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// WASM module loading and access (Node build).
//
// Defaults to reading the full `core/libsimlin.wasm` from disk; also accepts
// filesystem paths, `file://`/http(s) URLs, raw bytes, or a precompiled
// module. The shared singleton and instantiation logic live in
// ./wasm-runtime.ts; this file only resolves the source.

import {
  ensureInitializedWith,
  instantiate,
  instantiateFromSource,
  isInitialized,
  isUrl,
  resolveSourceProvider,
  type WasmSource,
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
  return path.join(__dirname, '..', '..', 'core', 'libsimlin.wasm');
}

function getLocationHref(): string | undefined {
  if (typeof globalThis === 'undefined' || !('location' in globalThis)) {
    return undefined;
  }
  return (globalThis as { location?: Location }).location?.href;
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

/**
 * Load a file from the filesystem in Node.js.
 * @internal Exported for testing
 */
export async function loadFileNode(pathOrUrl: string | URL): Promise<ArrayBuffer> {
  const fs = await import('node:fs/promises');
  const nodeBuffer = await fs.readFile(pathOrUrl);
  // fs.readFile always returns a Buffer backed by ArrayBuffer, not SharedArrayBuffer
  const buffer = nodeBuffer.buffer as ArrayBuffer;
  return buffer.slice(nodeBuffer.byteOffset, nodeBuffer.byteOffset + nodeBuffer.byteLength);
}

/**
 * Initialize the WASM module.
 * Must be called before any other functions.
 * @param wasmPathOrBuffer - A path/URL to the WASM file, the WASM binary as an
 *                           ArrayBuffer/Uint8Array, or a precompiled
 *                           `WebAssembly.Module`. In browsers, paths are
 *                           fetched as URLs. In Node.js, filesystem paths are
 *                           read directly. Defaults to `core/libsimlin.wasm`.
 */
export async function init(wasmPathOrBuffer?: WasmSourceProvider): Promise<void> {
  if (isInitialized()) {
    return;
  }

  const resolved: WasmSource =
    (await resolveSourceProvider(wasmPathOrBuffer)) ??
    (isNode() ? await getDefaultNodeWasmPath() : getDefaultBrowserWasmUrl());

  if (typeof resolved === 'string' || resolved instanceof URL) {
    const pathOrUrl = resolved instanceof URL ? resolved.toString() : resolved;
    if (isNode() && (isFileUrl(pathOrUrl) || !isUrl(pathOrUrl))) {
      const fileTarget = isFileUrl(pathOrUrl) ? new URL(pathOrUrl) : pathOrUrl;
      await instantiate(await loadFileNode(fileTarget));
      return;
    }
  }
  await instantiateFromSource(resolved);
}

/**
 * Ensure the WASM module is initialized.
 * This is a convenience function that will initialize WASM with default settings
 * if it hasn't been initialized yet. Safe to call multiple times.
 *
 * @param wasmSource - Optional WASM source or provider. Defaults to auto-detected
 *                     runtime settings for Node.js and browsers.
 */
export function ensureInitialized(wasmSource?: WasmSourceProvider): Promise<void> {
  return ensureInitializedWith(init, wasmSource);
}
