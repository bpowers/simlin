// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Engine bootstrap: get the libsimlin wasm from the kernel exactly once per
 * page and initialize this module's engine from it.
 *
 * Two levels of caching, because anywidget gives every widget INSTANCE its own
 * copy of this module (it imports `_esm` through a fresh `blob:` URL per model,
 * see anywidget's load.ts/runtime.ts): module-level state -- including
 * `@simlin/engine`'s wasm singleton -- is per instance, so the only thing that
 * can be shared across instances is something on `globalThis`. We share the
 * COMPILED module (`WebAssembly.Module`), which is the expensive part; each
 * instance then instantiates the engine from it (cheap).
 *
 *   globalThis[GLOBAL_KEY]  Promise<WebAssembly.Module>  page-wide, compiled once
 *   enginePromise           Promise<void>                per module instance
 *
 * The bytes travel over the widget comm: the first instance sends
 * `{type:'wasm'}` and the kernel answers with a custom message carrying the
 * artifact as a binary buffer. Both caches drop a rejected promise so a later
 * render (possibly under a different, healthy widget/kernel) can retry.
 *
 * The page-wide key carries the identity of the engine artifact this bundle
 * was built against ({@link WASM_IDENTITY}), so instances from two different
 * builds on one page -- two notebooks with two kernels running different
 * pysimlin versions, or a redisplay after an upgrade beside an old output --
 * each compile their own engine instead of the second silently running the
 * first one's wasm against a JS side built for another ABI. The identity is
 * the sha256 of libsimlin-browser.wasm at build time (rsbuild.config.ts bakes
 * it in; the staging script ships that same file beside widget.js), which is
 * why an instance can key the cache without ever seeing the bytes: a cache
 * hit means "a bundle built against this exact artifact already compiled it".
 * A widget cannot verify what the kernel actually sent against this identity;
 * the kernel does not carry the wasm's hash in its reply (it is in the wheel's
 * ASSETS.json), so a mismatched wheel -- widget.js from one build, wasm from
 * another -- is a packaging error the build/staging checks own, not this key.
 */

import { ready } from '@simlin/engine';

import type { AnyModel } from './anywidget-model';
import { parseWasmReply, WASM_REQUEST } from './widget-core';

/**
 * The engine artifact identity baked into this bundle (see the module doc);
 * `unversioned` outside a bundle (unit tests), where no artifact exists.
 */
export const WASM_IDENTITY: string =
  typeof SIMLIN_WIDGET_WASM_SHA256 === 'string' && SIMLIN_WIDGET_WASM_SHA256 !== ''
    ? SIMLIN_WIDGET_WASM_SHA256
    : 'unversioned';

/** The globalThis property caching the compiled module for `identity`. */
export function wasmCacheKey(identity: string): string {
  return `__simlinWidgetWasmModule:${identity}`;
}

export const GLOBAL_KEY = wasmCacheKey(WASM_IDENTITY);

/**
 * How long to wait for the kernel's wasm reply. anywidget queues comm
 * messages until the widget module has loaded (`_handle_comm_msg` awaits
 * `runtime.ready`), so a slow reply is normal; a kernel that never answers
 * (dead kernel, an old pysimlin without the handler) should surface as an
 * error in the cell rather than an eternal spinner.
 *
 * Only the ONE widget instance that issued the request runs this timer. Every
 * other instance on the page awaits the shared promise under GLOBAL_KEY (see
 * sharedWasmModule) with no timer of its own: it fails when, and only when,
 * the requester's attempt fails -- and that failure evicts the shared entry,
 * so the next render on any instance issues a fresh request.
 */
export const WASM_REPLY_TIMEOUT_MS = 60_000;

type GlobalCache = Record<string, Promise<WebAssembly.Module> | undefined>;

let enginePromise: Promise<void> | null = null;

/**
 * Ask the kernel for the wasm bytes over `model` and compile them.
 * Exported for tests; production goes through {@link ensureEngine}.
 */
export function requestWasmModule(
  model: AnyModel,
  opts: { timeoutMs?: number; compile?: (bytes: ArrayBuffer) => Promise<WebAssembly.Module> } = {},
): Promise<WebAssembly.Module> {
  const timeoutMs = opts.timeoutMs ?? WASM_REPLY_TIMEOUT_MS;
  const compile = opts.compile ?? ((bytes: ArrayBuffer) => WebAssembly.compile(bytes));
  return new Promise<WebAssembly.Module>((resolve, reject) => {
    let settled = false;
    const finish = (): void => {
      settled = true;
      clearTimeout(timer);
      model.off('msg:custom', onCustom);
    };
    const onCustom = (...args: unknown[]): void => {
      if (settled) {
        return;
      }
      const reply = parseWasmReply(args[0], args[1] as ReadonlyArray<unknown> | undefined);
      if (reply.kind === 'ignore') {
        return;
      }
      finish();
      if (reply.kind === 'error') {
        reject(new Error(`kernel could not supply the Simlin engine: ${reply.message}`));
        return;
      }
      compile(reply.bytes).then(resolve, reject);
    };
    const timer = setTimeout(() => {
      if (settled) {
        return;
      }
      finish();
      reject(new Error(`timed out after ${timeoutMs}ms waiting for the kernel to send the Simlin engine`));
    }, timeoutMs);
    model.on('msg:custom', onCustom);
    model.send(WASM_REQUEST);
  });
}

/**
 * The page-wide compiled module, requesting it through `model` if no widget
 * on this page has yet.
 */
export function sharedWasmModule(
  model: AnyModel,
  request: (model: AnyModel) => Promise<WebAssembly.Module> = requestWasmModule,
): Promise<WebAssembly.Module> {
  const cache = globalThis as unknown as GlobalCache;
  const existing = cache[GLOBAL_KEY];
  if (existing !== undefined) {
    return existing;
  }
  const created = request(model).catch((err: unknown) => {
    if (cache[GLOBAL_KEY] === created) {
      delete cache[GLOBAL_KEY];
    }
    throw err;
  });
  cache[GLOBAL_KEY] = created;
  return created;
}

/**
 * Initialize this module instance's engine (once) from the shared compiled
 * module. Idempotent and single-flight; a failure clears the memo so the next
 * call retries.
 */
export function ensureEngine(model: AnyModel): Promise<void> {
  if (enginePromise !== null) {
    return enginePromise;
  }
  const started = sharedWasmModule(model)
    .then((module) => ready(module))
    .catch((err: unknown) => {
      if (enginePromise === started) {
        enginePromise = null;
      }
      throw err;
    });
  enginePromise = started;
  return started;
}

/** Test seam: forget both caches. */
export function resetEngineBootstrapForTests(): void {
  enginePromise = null;
  delete (globalThis as unknown as GlobalCache)[GLOBAL_KEY];
}
