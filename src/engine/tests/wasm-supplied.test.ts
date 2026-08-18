// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The host-supplied WASM flavor (`internal/wasm.supplied.ts`) and the runtime
 * core it shares with the other flavors. This flavor bundles no artifact, so
 * every way a host can hand the engine its wasm is exercised here: raw bytes
 * (ArrayBuffer and an offset Uint8Array view), a precompiled
 * WebAssembly.Module, a URL (via global fetch), a provider function, and a
 * `configureWasm` override -- plus the loud failure when nothing is supplied.
 */

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import * as supplied from '../src/internal/wasm.supplied';
import { adoptInstance, ensureInitializedWith } from '../src/internal/wasm-runtime';

const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');

function loadWasmBytes(): Uint8Array {
  return fs.readFileSync(wasmPath);
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

let compiledModule: WebAssembly.Module | null = null;
async function compiled(): Promise<WebAssembly.Module> {
  if (compiledModule === null) {
    compiledModule = await WebAssembly.compile(toArrayBuffer(loadWasmBytes()));
  }
  return compiledModule;
}

function expectEngineUsable(): void {
  expect(supplied.isInitialized()).toBe(true);
  const exports = supplied.getExports();
  expect(typeof exports.simlin_project_open_xmile).toBe('function');
  expect(supplied.getMemory()).toBeInstanceOf(WebAssembly.Memory);
  // simlin_init ran: the panic buffer is reachable and empty.
  expect(supplied.getPanicMessage()).toBeNull();
}

describe('wasm.supplied flavor', () => {
  beforeEach(() => {
    supplied.reset();
  });
  afterEach(() => {
    supplied.reset();
    rs.restoreAllMocks();
  });

  it('starts uninitialized and getters throw', () => {
    expect(supplied.isInitialized()).toBe(false);
    expect(() => supplied.getExports()).toThrow(/not initialized/);
    expect(() => supplied.getMemory()).toThrow(/not initialized/);
    expect(supplied.getPanicMessage()).toBeNull();
    // clearPanicMessage is a no-op before init rather than a throw.
    supplied.clearPanicMessage();
  });

  it('rejects init() with no source instead of guessing a path', async () => {
    await expect(supplied.init()).rejects.toThrow(/no WASM source supplied/);
    await expect(supplied.ensureInitialized()).rejects.toThrow(/no WASM source supplied/);
    expect(supplied.isInitialized()).toBe(false);
  });

  it('initializes from an ArrayBuffer', async () => {
    await supplied.init(toArrayBuffer(loadWasmBytes()));
    expectEngineUsable();
  });

  it('initializes from a Uint8Array view that does not start at offset 0', async () => {
    const bytes = loadWasmBytes();
    const padded = new Uint8Array(bytes.byteLength + 16);
    padded.set(bytes, 16);
    const view = new Uint8Array(padded.buffer, 16, bytes.byteLength);
    await supplied.init(view);
    expectEngineUsable();
  });

  it('initializes from a precompiled WebAssembly.Module without recompiling', async () => {
    const module = await compiled();
    const compileSpy = rs.spyOn(WebAssembly, 'compile');
    await supplied.init(module);
    expect(compileSpy).not.toHaveBeenCalled();
    expectEngineUsable();
  });

  it('initializes from a provider function', async () => {
    const provider = rs.fn(async () => await compiled());
    await supplied.init(provider);
    expect(provider).toHaveBeenCalledTimes(1);
    expectEngineUsable();
  });

  it('honours configureWasm({ source }) when init() gets no argument', async () => {
    supplied.configureWasm({ source: toArrayBuffer(loadWasmBytes()) });
    await supplied.init();
    expectEngineUsable();
  });

  it('configureWasm throws once initialized', async () => {
    await supplied.init(await compiled());
    expect(() => supplied.configureWasm({ source: new ArrayBuffer(0) })).toThrow(/already initialized/);
  });

  it('fetches a URL source through global fetch', async () => {
    const bytes = toArrayBuffer(loadWasmBytes());
    const fetchMock = rs.fn(async (input: string | URL | Request) => {
      expect(String(input)).toBe('https://example.test/libsimlin.wasm');
      return new Response(bytes, { status: 200 });
    });
    rs.stubGlobal('fetch', fetchMock);
    await supplied.init(new URL('https://example.test/libsimlin.wasm'));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expectEngineUsable();
  });

  it('reports a failed fetch with the URL', async () => {
    rs.stubGlobal(
      'fetch',
      rs.fn(async () => new Response(null, { status: 404, statusText: 'Not Found' })),
    );
    await expect(supplied.init('https://example.test/missing.wasm')).rejects.toThrow(
      /Failed to load WASM from https:\/\/example.test\/missing.wasm: 404/,
    );
    expect(supplied.isInitialized()).toBe(false);
  });

  it('init() after success is a no-op that keeps the first instance', async () => {
    await supplied.init(await compiled());
    const first = supplied.getExports();
    await supplied.init(toArrayBuffer(loadWasmBytes()));
    expect(supplied.getExports()).toBe(first);
  });

  it('ensureInitialized single-flights concurrent callers', async () => {
    const instantiateSpy = rs.spyOn(WebAssembly, 'instantiate');
    const module = await compiled();
    await Promise.all([supplied.ensureInitialized(module), supplied.ensureInitialized(module)]);
    expect(instantiateSpy).toHaveBeenCalledTimes(1);
    expectEngineUsable();
  });

  it('a failed ensureInitialized clears the guard so the next caller retries', async () => {
    await expect(supplied.ensureInitialized()).rejects.toThrow();
    await supplied.ensureInitialized(await compiled());
    expectEngineUsable();
  });

  it('reset() drops the instance and the configured source', async () => {
    supplied.configureWasm({ source: await compiled() });
    await supplied.init();
    supplied.reset();
    expect(supplied.isInitialized()).toBe(false);
    await expect(supplied.init()).rejects.toThrow(/no WASM source supplied/);
  });

  it('flavors share one singleton: the runtime sees the flavor init', async () => {
    // Every flavor re-exports the runtime's accessors, so a second flavor
    // loaded in the same realm observes the same engine instance.
    const runtime = await import('../src/internal/wasm-runtime');
    await supplied.init(await compiled());
    expect(runtime.isInitialized()).toBe(true);
    expect(runtime.getExports()).toBe(supplied.getExports());
  });
});

describe('wasm-runtime core', () => {
  beforeEach(() => {
    supplied.reset();
  });
  afterEach(() => {
    supplied.reset();
  });

  it('adoptInstance prefers the exported memory over the fallback', async () => {
    const module = await compiled();
    const fallback = new WebAssembly.Memory({ initial: 1 });
    const instance = await WebAssembly.instantiate(module, { env: { memory: fallback } });
    adoptInstance(instance.exports, fallback);
    expect(supplied.getMemory()).toBe(instance.exports.memory);
    expect(supplied.getMemory()).not.toBe(fallback);
  });

  it('adoptInstance falls back to the supplied memory when the module exports none', () => {
    const fallback = new WebAssembly.Memory({ initial: 1 });
    adoptInstance({}, fallback);
    expect(supplied.getMemory()).toBe(fallback);
    // No simlin_init / panic exports: the accessors degrade to null, not a throw.
    expect(supplied.getPanicMessage()).toBeNull();
    supplied.clearPanicMessage();
  });

  it('ensureInitializedWith passes the source through to the loader once', async () => {
    const loader = rs.fn(async (source?: unknown) => {
      expect(source).toBe('marker');
      adoptInstance({});
    });
    await Promise.all([ensureInitializedWith(loader, 'marker'), ensureInitializedWith(loader, 'marker')]);
    await ensureInitializedWith(loader, 'marker');
    expect(loader).toHaveBeenCalledTimes(1);
  });
});
