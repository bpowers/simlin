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
 * `SIMLIN_WIDGET_ASSET=inline` (pysimlin `_widget_core.inline_esm`) prepends
 * `globalThis.__simlinWidgetInlineWasm = "<base64 wasm>";` to the module
 * text, so the artifact is already on the page when this module evaluates
 * (Colab may not deliver binary comm buffers). It is read ONCE, at module
 * evaluation -- the same synchronous script that set it, so a later instance
 * from another build overwriting the global cannot hand this instance the
 * wrong bytes -- and cleared, and it fills the page-wide cache instead of a
 * comm request; anything unusable in it falls back to the comm request, which
 * the kernel answers in every asset mode.
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

/** The global pysimlin's `inline` asset mode defines (`_widget_core.INLINE_WASM_GLOBAL`). */
export const INLINE_WASM_GLOBAL = '__simlinWidgetInlineWasm';

/**
 * Read and clear the inline-wasm global. Null when absent or not a
 * non-empty string; the string is base64 of the artifact (a plain string,
 * not a data: URL -- the contract in `inline_esm`).
 */
export function takeInlineWasm(): string | null {
  const g = globalThis as Record<string, unknown>;
  const value = g[INLINE_WASM_GLOBAL];
  if (typeof value !== 'string' || value === '') {
    return null;
  }
  delete g[INLINE_WASM_GLOBAL];
  return value;
}

/** Base64 text -> the bytes, as a standalone ArrayBuffer for WebAssembly.compile. */
export function decodeBase64(text: string): ArrayBuffer {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

// Captured at module evaluation (see the module doc); consumed by the first
// sharedWasmModule call on this instance, then dropped so the ~9 MB string is
// not retained.
let inlineWasmBase64: string | null = takeInlineWasm();

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

/**
 * What the cell says when the wasm never arrives. The one case a user is
 * likely to hit is a static export: `jupyter nbconvert --to html --execute`
 * stores the widget state by default and the exported page then shows the
 * widget itself, whose kernel request nobody answers, instead of the SVG the
 * output also carries -- so the message names that case and its fix first.
 * The widget cannot tell a missing kernel from a slow one: anywidget's model
 * proxy exposes neither the comm nor its liveness (`widget_manager` is on
 * the proxy but flagged as an over-wide surface to be narrowed, and its
 * shape differs per host), and `send` on a comm-less model is a silent
 * no-op. So the export case is not detected early; it is explained when the
 * timeout fires.
 */
export function wasmTimeoutMessage(timeoutMs: number): string {
  const seconds = Math.round(timeoutMs / 1000);
  return (
    `the kernel did not send the Simlin engine within ${seconds} s. ` +
    'If this page is a static export (nbconvert, a saved HTML page), there is no kernel to send it: ' +
    're-export with --ExecutePreprocessor.store_widget_state=False so the diagram is embedded as an image instead. ' +
    'If a kernel is running, it may be busy, or its pysimlin install may be missing the widget assets.'
  );
}

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
      reject(new Error(wasmTimeoutMessage(timeoutMs)));
    }, timeoutMs);
    model.on('msg:custom', onCustom);
    model.send(WASM_REQUEST);
  });
}

/**
 * Compile the inline artifact this module was loaded with, or reject when
 * there is none or it is unusable (bad base64, not a wasm module).
 */
async function compileInlineWasm(
  compile: (bytes: ArrayBuffer) => Promise<WebAssembly.Module>,
): Promise<WebAssembly.Module> {
  const base64 = inlineWasmBase64;
  inlineWasmBase64 = null;
  if (base64 === null) {
    throw new Error('no inline wasm');
  }
  return compile(decodeBase64(base64));
}

/**
 * The page-wide compiled module: the inline artifact if this module was
 * loaded with one, else requested through `model` -- unless a widget on this
 * page (built against the same artifact) has already compiled it.
 */
export function sharedWasmModule(
  model: AnyModel,
  request: (model: AnyModel) => Promise<WebAssembly.Module> = requestWasmModule,
  compile: (bytes: ArrayBuffer) => Promise<WebAssembly.Module> = (bytes) => WebAssembly.compile(bytes),
): Promise<WebAssembly.Module> {
  const cache = globalThis as unknown as GlobalCache;
  const existing = cache[GLOBAL_KEY];
  if (existing !== undefined) {
    // Another instance already compiled it; drop this instance's copy of the
    // inline payload rather than keep ~9 MB of base64 alive for nothing.
    inlineWasmBase64 = null;
    return existing;
  }
  const source =
    inlineWasmBase64 !== null
      ? compileInlineWasm(compile).catch((err: unknown) => {
          // The kernel answers wasm requests in every asset mode, so a
          // broken inline payload costs a round trip, not the widget.
          console.warn('simlin widget: inline wasm unusable, requesting it from the kernel instead', err);
          return request(model);
        })
      : request(model);
  const created = source.catch((err: unknown) => {
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

/** Test seam: whether this module still holds an inline payload. */
export function inlineWasmHeldForTests(): boolean {
  return inlineWasmBase64 !== null;
}

/**
 * Test seam: forget the per-module memo and (unless `keepPageCache`) the
 * page-wide cache, and set the inline artifact this module "was loaded with".
 */
export function resetEngineBootstrapForTests(opts: { inlineWasm?: string | null; keepPageCache?: boolean } = {}): void {
  enginePromise = null;
  inlineWasmBase64 = opts.inlineWasm ?? null;
  if (!opts.keepPageCache) {
    delete (globalThis as unknown as GlobalCache)[GLOBAL_KEY];
  }
}
