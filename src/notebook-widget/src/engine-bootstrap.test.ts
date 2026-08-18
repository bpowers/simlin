// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import {
  ensureEngine,
  GLOBAL_KEY,
  INLINE_WASM_GLOBAL,
  takeInlineWasm,
  requestWasmModule,
  resetEngineBootstrapForTests,
  sharedWasmModule,
  WASM_IDENTITY,
  WASM_REPLY_TIMEOUT_MS,
  wasmCacheKey,
} from './engine-bootstrap';
import { readyCalls, resetEngineMock } from './test-utils/engine-mock';
import { FakeModel, defaultState } from './test-utils/fake-model';

// A tiny valid wasm module (magic + version, no sections) so WebAssembly.compile
// has something real to compile where the test wants the real thing.
const EMPTY_WASM = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);

async function emptyModule(): Promise<WebAssembly.Module> {
  return WebAssembly.compile(EMPTY_WASM.buffer.slice(0));
}

describe('requestWasmModule', () => {
  beforeEach(() => {
    rs.useFakeTimers();
  });
  afterEach(() => {
    rs.useRealTimers();
  });

  it('sends {type:"wasm"}, compiles the DataView reply, and unsubscribes', async () => {
    const model = new FakeModel(defaultState());
    const compile = rs.fn(async (bytes: ArrayBuffer) => {
      expect(new Uint8Array(bytes)).toEqual(EMPTY_WASM);
      return emptyModule();
    });
    const p = requestWasmModule(model, { compile });
    expect(model.sent).toEqual([{ type: 'wasm' }]);
    expect(model.listenerCount('msg:custom')).toBe(1);
    // An unrelated custom message first: ignored, still listening.
    model.trigger('msg:custom', { type: 'something-else' }, []);
    expect(model.listenerCount('msg:custom')).toBe(1);
    model.trigger('msg:custom', { type: 'wasm' }, [new DataView(EMPTY_WASM.buffer.slice(0))]);
    const module = await p;
    expect(module).toBeInstanceOf(WebAssembly.Module);
    expect(compile).toHaveBeenCalledTimes(1);
    expect(model.listenerCount('msg:custom')).toBe(0);
    // The timeout was cleared on success: nothing is left pending, and
    // advancing past it neither throws nor re-sends.
    expect(rs.getTimerCount()).toBe(0);
    await rs.advanceTimersByTimeAsync(WASM_REPLY_TIMEOUT_MS + 1);
    expect(model.sent).toEqual([{ type: 'wasm' }]);
  });

  it('rejects on a kernel error reply', async () => {
    const model = new FakeModel(defaultState());
    const p = requestWasmModule(model);
    model.trigger('msg:custom', { type: 'wasm', error: 'no asset' }, []);
    await expect(p).rejects.toThrow(/no asset/);
    expect(model.listenerCount('msg:custom')).toBe(0);
    expect(rs.getTimerCount()).toBe(0);
  });

  it('rejects after the timeout when the kernel never answers', async () => {
    const model = new FakeModel(defaultState());
    const p = requestWasmModule(model, { timeoutMs: 1000 });
    const assertion = expect(p).rejects.toThrow(/timed out after 1000ms/);
    await rs.advanceTimersByTimeAsync(1000);
    await assertion;
    expect(model.listenerCount('msg:custom')).toBe(0);
    // A late reply is ignored (no throw, no listener).
    model.trigger('msg:custom', { type: 'wasm' }, [new DataView(EMPTY_WASM.buffer.slice(0))]);
  });
});

describe('sharedWasmModule / ensureEngine', () => {
  beforeEach(() => {
    resetEngineBootstrapForTests();
    resetEngineMock();
  });
  afterEach(() => {
    resetEngineBootstrapForTests();
  });

  it('caches the compiled module on globalThis so a second requester never asks the kernel', async () => {
    const m1 = new FakeModel(defaultState());
    const m2 = new FakeModel(defaultState());
    const request = rs.fn(async (_model: unknown) => emptyModule());
    const a = sharedWasmModule(m1, request);
    const b = sharedWasmModule(m2, request);
    expect(b).toBe(a);
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith(m1);
    expect((globalThis as Record<string, unknown>)[GLOBAL_KEY]).toBe(a);
    await a;
  });

  it('the page-wide key carries the engine artifact identity, so a module compiled by a bundle built against another artifact is not reused', async () => {
    expect(GLOBAL_KEY).toBe(wasmCacheKey(WASM_IDENTITY));
    expect(wasmCacheKey('a')).not.toBe(wasmCacheKey('b'));
    // Outside a bundle there is no artifact: the identity is the placeholder
    // (rsbuild.config.ts bakes the real sha256 in; e2e/ checks the built key).
    expect(WASM_IDENTITY).toBe('unversioned');
    // Another build's compiled module on the page: not ours to reuse.
    const other = wasmCacheKey('deadbeef');
    (globalThis as Record<string, unknown>)[other] = Promise.resolve(await emptyModule());
    const m = new FakeModel(defaultState());
    const request = rs.fn(async (_model: unknown) => emptyModule());
    await sharedWasmModule(m, request);
    expect(request).toHaveBeenCalledTimes(1);
    delete (globalThis as Record<string, unknown>)[other];
  });

  it('drops a rejected shared promise so the next requester retries', async () => {
    const m = new FakeModel(defaultState());
    const failing = rs.fn(async (_model: unknown): Promise<WebAssembly.Module> => {
      throw new Error('kernel down');
    });
    await expect(sharedWasmModule(m, failing)).rejects.toThrow('kernel down');
    expect((globalThis as Record<string, unknown>)[GLOBAL_KEY]).toBeUndefined();
    const ok = rs.fn(async (_model: unknown) => emptyModule());
    await expect(sharedWasmModule(m, ok)).resolves.toBeInstanceOf(WebAssembly.Module);
  });

  it('ensureEngine hands the shared module to the engine once and memoizes', async () => {
    const model = new FakeModel(defaultState());
    // Pre-seed the page-wide cache; ensureEngine must not need the kernel.
    const module = await emptyModule();
    (globalThis as Record<string, unknown>)[GLOBAL_KEY] = Promise.resolve(module);
    const p1 = ensureEngine(model);
    const p2 = ensureEngine(model);
    expect(p2).toBe(p1);
    await p1;
    expect(readyCalls).toEqual([module]);
    expect(model.sent).toEqual([]);
  });

  it('ensureEngine requests through the model when nothing is cached', async () => {
    const model = new FakeModel(defaultState());
    const p = ensureEngine(model);
    expect(model.sent).toEqual([{ type: 'wasm' }]);
    model.trigger('msg:custom', { type: 'wasm' }, [new DataView(EMPTY_WASM.buffer.slice(0))]);
    await p;
    expect(readyCalls).toHaveLength(1);
    expect(readyCalls[0]).toBeInstanceOf(WebAssembly.Module);
  });

  it('a failed ensureEngine clears its memo so a later widget can retry', async () => {
    const model = new FakeModel(defaultState());
    const p = ensureEngine(model);
    model.trigger('msg:custom', { type: 'wasm', error: 'missing' }, []);
    await expect(p).rejects.toThrow(/missing/);
    const model2 = new FakeModel(defaultState());
    const p2 = ensureEngine(model2);
    expect(p2).not.toBe(p);
    expect(model2.sent).toEqual([{ type: 'wasm' }]);
    model2.trigger('msg:custom', { type: 'wasm' }, [new DataView(EMPTY_WASM.buffer.slice(0))]);
    await p2;
    expect(readyCalls).toHaveLength(1);
  });
});

describe('inline wasm (SIMLIN_WIDGET_ASSET=inline)', () => {
  // pysimlin's `inline` mode prepends `globalThis.__simlinWidgetInlineWasm =
  // "<base64>";` to the module text, so the global is set when this module
  // evaluates and the bytes are available without any comm round trip
  // (Colab may not deliver binary comm buffers).
  const base64 = Buffer.from(EMPTY_WASM).toString('base64');
  beforeEach(() => {
    resetEngineBootstrapForTests();
    resetEngineMock();
  });
  afterEach(() => {
    resetEngineBootstrapForTests();
    delete (globalThis as Record<string, unknown>)[INLINE_WASM_GLOBAL];
  });

  it('takeInlineWasm reads and clears the global; a missing or non-string global is null', () => {
    expect(takeInlineWasm()).toBeNull();
    (globalThis as Record<string, unknown>)[INLINE_WASM_GLOBAL] = 42;
    expect(takeInlineWasm()).toBeNull();
    (globalThis as Record<string, unknown>)[INLINE_WASM_GLOBAL] = base64;
    expect(takeInlineWasm()).toBe(base64);
    // Consumed: a second read finds nothing (the next instance's own module
    // text sets it again before that instance evaluates).
    expect((globalThis as Record<string, unknown>)[INLINE_WASM_GLOBAL]).toBeUndefined();
    expect(takeInlineWasm()).toBeNull();
  });

  it('ensureEngine compiles the inline bytes and never asks the kernel', async () => {
    resetEngineBootstrapForTests({ inlineWasm: base64 });
    const model = new FakeModel(defaultState());
    // The fake kernel would answer a wasm request; the point is that none is sent.
    await ensureEngine(model);
    expect(model.sent).toEqual([]);
    expect(readyCalls).toHaveLength(1);
    expect(readyCalls[0]).toBeInstanceOf(WebAssembly.Module);
    // Cached page-wide under the same identity key as a comm-delivered module.
    await expect((globalThis as Record<string, unknown>)[GLOBAL_KEY]).resolves.toBeInstanceOf(WebAssembly.Module);
    // A second instance (module-level memo cleared, page cache kept) reuses it.
    resetEngineBootstrapForTests({ keepPageCache: true });
    const model2 = new FakeModel(defaultState());
    await ensureEngine(model2);
    expect(model2.sent).toEqual([]);
  });

  it('unusable inline bytes fall back to the comm request (the kernel answers in every mode)', async () => {
    resetEngineBootstrapForTests({ inlineWasm: 'not base64 at all!!' });
    const model = new FakeModel(defaultState());
    const warn = rs.spyOn(console, 'warn').mockImplementation(() => {});
    const p = ensureEngine(model);
    // The inline attempt fails asynchronously, then the request goes out.
    for (let i = 0; i < 20 && model.sent.length === 0; i++) {
      await Promise.resolve();
    }
    expect(model.sent).toEqual([{ type: 'wasm' }]);
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
    model.trigger('msg:custom', { type: 'wasm' }, [new DataView(EMPTY_WASM.buffer.slice(0))]);
    await p;
    expect(readyCalls).toHaveLength(1);
  });
});
