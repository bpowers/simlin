// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The browser flavor's source resolution: with no source it adopts the
 * bundler-instantiated artifact (stubbed here, see rstest.config.mts); with a
 * caller-supplied source (bytes, module, URL, provider, configureWasm) it
 * instantiates THAT and never touches the bundled one. The shared runtime
 * behaviour behind both is covered by wasm-supplied.test.ts.
 */

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import * as browser from '../src/internal/wasm.browser';
import * as stub from './stubs/browser-wasm-stub';

const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');

function loadWasmBuffer(): ArrayBuffer {
  const bytes = fs.readFileSync(wasmPath);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function expectBundledAdopted(): void {
  expect(browser.isInitialized()).toBe(true);
  expect(browser.getExports().bundled_marker).toBe(stub.bundled_marker);
  expect(browser.getMemory()).toBe(stub.memory);
  expect(stub.initCalls).toEqual(['bundled']);
}

function expectRealEngine(): void {
  expect(browser.isInitialized()).toBe(true);
  expect(browser.getExports().bundled_marker).toBeUndefined();
  expect(typeof browser.getExports().simlin_project_open_xmile).toBe('function');
  expect(browser.getMemory()).not.toBe(stub.memory);
  expect(stub.initCalls).toEqual([]);
}

describe('wasm.browser flavor', () => {
  beforeEach(() => {
    browser.reset();
    stub.initCalls.length = 0;
  });
  afterEach(() => {
    browser.reset();
    rs.restoreAllMocks();
  });

  it('adopts the bundled artifact when no source is supplied', async () => {
    await browser.init();
    expectBundledAdopted();
  });

  it('ensureInitialized() with no source adopts the bundled artifact once', async () => {
    await Promise.all([browser.ensureInitialized(), browser.ensureInitialized()]);
    expectBundledAdopted();
  });

  it('honours caller-supplied bytes instead of the bundled artifact', async () => {
    await browser.init(loadWasmBuffer());
    expectRealEngine();
  });

  it('honours a precompiled module without recompiling', async () => {
    const module = await WebAssembly.compile(loadWasmBuffer());
    const compileSpy = rs.spyOn(WebAssembly, 'compile');
    await browser.init(module);
    expect(compileSpy).not.toHaveBeenCalled();
    expectRealEngine();
  });

  it('honours a provider function', async () => {
    const provider = rs.fn(async () => loadWasmBuffer());
    await browser.ensureInitialized(provider);
    expect(provider).toHaveBeenCalledTimes(1);
    expectRealEngine();
  });

  it('fetches a URL source', async () => {
    const bytes = loadWasmBuffer();
    rs.stubGlobal(
      'fetch',
      rs.fn(async () => new Response(bytes, { status: 200 })),
    );
    await browser.init('https://example.test/libsimlin-browser.wasm');
    expectRealEngine();
  });

  it('honours configureWasm({ source }) and throws on configureWasm after init', async () => {
    browser.configureWasm({ source: loadWasmBuffer() });
    await browser.init();
    expectRealEngine();
    expect(() => browser.configureWasm({ source: new ArrayBuffer(0) })).toThrow(/already initialized/);
  });

  it('a second init() with a source after adopting the bundle is a no-op', async () => {
    await browser.init();
    await browser.init(loadWasmBuffer());
    expectBundledAdopted();
  });
});
