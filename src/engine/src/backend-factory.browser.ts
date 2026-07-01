// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Browser backend factory.
 *
 * Creates a WorkerBackend that spawns a Web Worker for WASM execution,
 * keeping the main thread free for UI interaction. The Worker is created
 * lazily on first access and reused for all subsequent operations.
 *
 * When the resolved worker chunk URL is cross-origin -- the embeddable web
 * component hotlinked from a third-party page (issue #688) -- the worker is
 * created through a same-origin blob trampoline instead, because the Worker
 * constructor enforces the same-origin policy regardless of CORS. See
 * worker-trampoline.ts for the mechanism.
 *
 * This is selected at build time via tsconfig path mapping for browser builds.
 */

import { EngineBackend } from './backend';
import { WorkerBackend } from './worker-backend';
import { spawnWithTrampoline } from './worker-trampoline';
import type { WorkerRequest, WorkerResponse } from './worker-protocol';

// Bundlers that implement webpack's module variables (rspack, webpack)
// rewrite this free identifier to their runtime publicPath; with
// assetPrefix 'auto' that value is derived from the embedding <script src>,
// i.e. it carries the origin our assets actually live on. Under bundlers
// that don't implement it (vite), the typeof guard below safely yields
// 'undefined'.
declare let __webpack_public_path__: string;

let sharedBackend: EngineBackend | null = null;
let sharedWorker: Worker | null = null;
// blob: URL backing a trampolined worker; revoked once the worker proves it
// loaded (first message) or failed (error event) so it doesn't leak.
let sharedWorkerBlobUrl: string | null = null;

function releaseWorkerBlobUrl(): void {
  if (sharedWorkerBlobUrl !== null) {
    URL.revokeObjectURL(sharedWorkerBlobUrl);
    sharedWorkerBlobUrl = null;
  }
}

function spawnBundledWorker(): Worker {
  // IMPORTANT: this expression must stay literally in the form
  // `new Worker(new URL('...', import.meta.url), ...)`. rspack, webpack and
  // vite detect the worker (and bundle ./engine-worker.js into a worker
  // chunk) only when the `new URL(...)` is inline inside the
  // `new Worker(...)` call; hoisting the URL into a variable silently
  // degrades the reference to a raw, unbundled asset (verified against
  // rspack 1.7). Cross-origin handling happens in spawnWithTrampoline, which
  // intercepts this construction to observe the resolved chunk URL.
  return new Worker(new URL('./engine-worker.js', import.meta.url), {
    type: 'module',
  });
}

function bundlerPublicPath(): string | undefined {
  return typeof __webpack_public_path__ === 'string' ? __webpack_public_path__ : undefined;
}

function pageOrigin(): string | undefined {
  return typeof self !== 'undefined' && self.location ? self.location.origin : undefined;
}

function createWorkerBackend(): WorkerBackend {
  const spawned = spawnWithTrampoline(globalThis as { Worker?: unknown }, spawnBundledWorker, {
    pageOrigin: pageOrigin(),
    publicPath: bundlerPublicPath(),
  });
  const worker = spawned.worker;
  sharedWorker = worker;
  sharedWorkerBlobUrl = spawned.blobUrl;

  const backend = new WorkerBackend(
    (msg: WorkerRequest, transfer?: Transferable[]) => {
      if (transfer && transfer.length > 0) {
        worker.postMessage(msg, transfer);
      } else {
        worker.postMessage(msg);
      }
    },
    (callback: (msg: WorkerResponse) => void) => {
      worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
        // The first message proves the (possibly trampolined) worker script
        // finished loading, so the backing blob URL is no longer needed.
        // No-op on every later message and on the direct path.
        releaseWorkerBlobUrl();
        callback(event.data);
      };
    },
  );

  worker.onerror = (event: ErrorEvent) => {
    event.preventDefault();
    releaseWorkerBlobUrl();
    const error = new Error(`Worker error: ${event.message}`);
    backend.handleWorkerError(error);
    sharedBackend = null;
    sharedWorker = null;
    worker.terminate();
  };

  return backend;
}

export function getBackend(): EngineBackend {
  if (!sharedBackend) {
    sharedBackend = createWorkerBackend();
  }
  return sharedBackend;
}

export function resetBackend(): void {
  if (sharedBackend) {
    // Reject all pending/queued requests before terminating the worker
    // to prevent promise leaks.
    (sharedBackend as WorkerBackend).terminate();
    sharedBackend = null;
  }
  if (sharedWorker) {
    sharedWorker.terminate();
    sharedWorker = null;
  }
  releaseWorkerBlobUrl();
}
