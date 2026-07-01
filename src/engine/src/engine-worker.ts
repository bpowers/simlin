// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Web Worker entry point for the engine.
 *
 * Instantiates a WorkerServer and wires it to the Worker's message handler.
 * This file is excluded from the Node.js build (CommonJS tsconfig) and
 * included only in the browser build.
 *
 * IMPORTANT: The message handler must be installed synchronously at module
 * top level.  WorkerServer transitively imports WASM, which rspack treats
 * as an async module.  If we used a static `import { WorkerServer }`, rspack
 * would wrap this entire entry point in an async module wrapper, meaning
 * `self.onmessage` wouldn't be set until the WASM binary finished loading.
 * The main thread sends its first message (init) immediately after creating
 * the Worker, so that message would be silently dropped, deadlocking the
 * backend.  By using a dynamic `import()` instead, the entry module stays
 * synchronous: messages that arrive before the server is ready are buffered
 * and replayed once the dynamic import resolves.
 */

import type { WorkerResponse } from './worker-protocol';
import { ENGINE_PUBLIC_PATH_GLOBAL } from './worker-trampoline';

// Rewritten by rspack/webpack to the runtime publicPath slot. Under bundlers
// without webpack module variables (vite) the identifier stays undeclared,
// which is still safe: `typeof` never throws on unresolvable references, it
// yields 'undefined' and the guarded assignment below is skipped.
declare let __webpack_public_path__: string;

// Worker global scope interface for postMessage with Transferable support.
// We define this locally rather than adding the webworker lib (which conflicts
// with dom lib types).
interface WorkerGlobalScope {
  postMessage(message: unknown, transfer: Transferable[]): void;
  postMessage(message: unknown): void;
  onmessage: ((event: MessageEvent) => void) | null;
}

const workerSelf = self as unknown as WorkerGlobalScope;

// Cross-origin embed support (issue #688): when the blob trampoline in
// backend-factory.browser.ts boots this worker, the bundler's
// `publicPath: 'auto'` runtime derives the asset base from `self.location`
// -- the blob URL, i.e. the *embedding* page's origin -- so the WASM fetch
// below would target the wrong host. The trampoline records the correct
// asset root under a well-known global before loading this chunk; apply it
// here, which runs after the bundler runtime computed its (wrong) value and
// before the dynamic import below triggers the WASM load. Directly-loaded
// workers never see the global, so their behavior is unchanged.
const publicPathOverride = (self as unknown as Record<string, unknown>)[ENGINE_PUBLIC_PATH_GLOBAL];
if (typeof publicPathOverride === 'string' && typeof __webpack_public_path__ === 'string') {
  __webpack_public_path__ = publicPathOverride;
}

// Buffer for messages that arrive before the dynamic import resolves.
let pendingMessages: unknown[] | null = [];

// Install the handler synchronously so no messages are dropped.
workerSelf.onmessage = (event: MessageEvent) => {
  if (pendingMessages !== null) {
    pendingMessages.push(event.data);
  }
};

import('./worker-server')
  .then(({ WorkerServer }) => {
    const server = new WorkerServer((msg: WorkerResponse, transfer?: Transferable[]) => {
      if (transfer && transfer.length > 0) {
        workerSelf.postMessage(msg, transfer);
      } else {
        workerSelf.postMessage(msg);
      }
    });

    // Replay any messages that arrived while we were loading.
    const buffered = pendingMessages!;
    pendingMessages = null;
    for (const msg of buffered) {
      server.handleMessage(msg);
    }

    // All future messages go directly to the server.
    workerSelf.onmessage = (event: MessageEvent) => {
      server.handleMessage(event.data);
    };
  })
  .catch((err: unknown) => {
    // If the dynamic import fails (e.g. WASM binary can't be loaded),
    // reject all buffered messages so the main thread's queue doesn't hang.
    const buffered = pendingMessages ?? [];
    pendingMessages = null;
    const errorMsg = err instanceof Error ? err.message : String(err);
    for (const msg of buffered) {
      const req = msg as { requestId?: number };
      if (typeof req.requestId === 'number') {
        workerSelf.postMessage({
          type: 'error',
          requestId: req.requestId,
          error: { name: 'Error', message: `Worker initialization failed: ${errorMsg}` },
        });
      }
    }
    // Future messages also get an error response.
    workerSelf.onmessage = (event: MessageEvent) => {
      const req = event.data as { requestId?: number };
      if (typeof req.requestId === 'number') {
        workerSelf.postMessage({
          type: 'error',
          requestId: req.requestId,
          error: { name: 'Error', message: `Worker initialization failed: ${errorMsg}` },
        });
      }
    };
  });
