// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Platform-neutral core shared by every `@simlin/engine/internal/wasm`
 * flavor (`wasm.node.ts`, `wasm.browser.ts`, `wasm.supplied.ts`).
 *
 * The flavors differ ONLY in where the wasm comes from when the caller does
 * not supply it: the Node flavor reads `core/libsimlin.wasm` from disk, the
 * browser flavor adopts the artifact the bundler instantiated, and the
 * supplied flavor has no default at all (the host must hand over bytes, a
 * URL, or a precompiled `WebAssembly.Module` at runtime -- the notebook
 * widget receives the wasm over the kernel comm, for example). Everything
 * else -- the singleton exports/memory, single-flight initialization,
 * source-override configuration, instantiation from bytes or a module, and
 * the panic-message accessors -- lives here exactly once. Keep it that way:
 * a flavor that grows its own copy of any of this drifts precisely where the
 * logic is non-trivial (the single-flight guard, the memory adoption rule).
 */

export type WasmSource = string | URL | ArrayBuffer | Uint8Array | WebAssembly.Module;
export type WasmSourceProvider = WasmSource | (() => WasmSource | Promise<WasmSource>);

export interface WasmConfig {
  source?: WasmSourceProvider;
}

let wasmExports: WebAssembly.Exports | null = null;
let wasmMemory: WebAssembly.Memory | null = null;
let initPromise: Promise<void> | null = null;
let wasmSourceOverride: WasmSourceProvider | null = null;

/**
 * Check if a string looks like a URL (http://, https://, or file://).
 * @internal Exported for testing
 */
export function isUrl(path: string): boolean {
  return path.startsWith('http://') || path.startsWith('https://') || path.startsWith('file://');
}

/** True for the two "raw wasm bytes" members of {@link WasmSource}. */
export function isWasmBytes(source: WasmSource): source is ArrayBuffer | Uint8Array {
  return source instanceof ArrayBuffer || source instanceof Uint8Array;
}

/**
 * Resolve the caller's source (or, failing that, the configured override) to
 * a concrete {@link WasmSource}, invoking a provider function if given.
 * Returns `undefined` when neither is set so each flavor can apply its own
 * default.
 */
export async function resolveSourceProvider(source?: WasmSourceProvider): Promise<WasmSource | undefined> {
  const provider = source ?? wasmSourceOverride ?? undefined;
  if (provider === undefined) {
    return undefined;
  }
  return typeof provider === 'function' ? await provider() : provider;
}

/**
 * Fetch a wasm binary over HTTP. Shared so every flavor reports a fetch
 * failure with the same URL-bearing message.
 */
export async function fetchWasm(url: string): Promise<ArrayBuffer> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load WASM from ${url}: ${response.status} ${response.statusText}`);
  }
  return response.arrayBuffer();
}

function toArrayBuffer(bytes: ArrayBuffer | Uint8Array): ArrayBuffer {
  if (bytes instanceof ArrayBuffer) {
    return bytes;
  }
  // Copy to a fresh ArrayBuffer: the view may be a window into a larger (or
  // shared) buffer, and WebAssembly.compile wants exactly the module bytes.
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

/**
 * Compile (if given bytes) and instantiate libsimlin, adopting the result as
 * the process-wide engine instance. A precompiled `WebAssembly.Module` skips
 * the compile step, which is what lets several independently loaded copies
 * of this package on one page share one compilation.
 */
export async function instantiate(source: ArrayBuffer | Uint8Array | WebAssembly.Module): Promise<void> {
  const module = source instanceof WebAssembly.Module ? source : await WebAssembly.compile(toArrayBuffer(source));

  // libsimlin manages its own memory, but an import is supplied so a build
  // that does import memory still links.
  const memory = new WebAssembly.Memory({ initial: 256, maximum: 16384 });
  const instance = await WebAssembly.instantiate(module, { env: { memory } });
  adoptInstance(instance.exports, memory);
}

/**
 * Instantiate from any concrete {@link WasmSource}: bytes and precompiled
 * modules go straight to {@link instantiate}; a URL (string or `URL`) is
 * fetched first. Filesystem paths are a Node-only concern handled by the Node
 * flavor before it gets here.
 */
export async function instantiateFromSource(source: WasmSource): Promise<void> {
  if (source instanceof WebAssembly.Module || isWasmBytes(source)) {
    await instantiate(source);
    return;
  }
  await instantiate(await fetchWasm(source instanceof URL ? source.toString() : source));
}

/**
 * Adopt an already-instantiated module's exports as the engine instance.
 * The bundler-instantiated browser artifact enters here directly; the
 * bytes/module path enters through {@link instantiate}.
 */
export function adoptInstance(exports: WebAssembly.Exports, fallbackMemory: WebAssembly.Memory | null = null): void {
  wasmExports = exports;
  // The module's own exported memory wins over the import we supplied.
  wasmMemory = exports.memory instanceof WebAssembly.Memory ? exports.memory : fallbackMemory;

  // Install the Rust panic hook so panic messages are captured in a global
  // buffer rather than silently lost to `unreachable` traps.
  const initFn = exports.simlin_init as (() => void) | undefined;
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

/** Check if the WASM module is initialized. */
export function isInitialized(): boolean {
  return wasmExports !== null;
}

/**
 * Run a flavor's `init` at most once at a time: concurrent callers share the
 * in-flight promise, and a call after success is a no-op. A failed init
 * clears the guard so the next caller retries (with a fresh source, say).
 */
export async function ensureInitializedWith(
  init: (source?: WasmSourceProvider) => Promise<void>,
  source?: WasmSourceProvider,
): Promise<void> {
  if (wasmExports !== null) {
    return;
  }
  if (initPromise !== null) {
    await initPromise;
    return;
  }
  initPromise = init(source);
  try {
    await initPromise;
  } finally {
    initPromise = null;
  }
}

/**
 * Configure a default WASM source consulted by `init()` when its caller
 * passes none. Must run before initialization starts.
 */
export function configureWasm(config: WasmConfig = {}): void {
  if (wasmExports !== null || initPromise !== null) {
    throw new Error('WASM already initialized');
  }
  wasmSourceOverride = config.source ?? null;
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

/** Clear the stored panic message. */
export function clearPanicMessage(): void {
  if (wasmExports === null) {
    return;
  }
  const fn = wasmExports.simlin_clear_panic_message as (() => void) | undefined;
  if (fn) {
    fn();
  }
}

/** Reset the WASM state (for testing). */
export function reset(): void {
  wasmExports = null;
  wasmMemory = null;
  initPromise = null;
  wasmSourceOverride = null;
}
