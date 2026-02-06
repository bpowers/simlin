// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Browser backend factory.
 *
 * Creates a WorkerBackend that spawns a Web Worker for WASM execution,
 * keeping the main thread free for UI interaction. The Worker is created
 * lazily on first access and reused for all subsequent operations.
 *
 * This is selected at build time via tsconfig path mapping for browser builds.
 */

import { EngineBackend } from './backend';
import { WorkerBackend } from './worker-backend';
import type { WorkerRequest, WorkerResponse } from './worker-protocol';

let sharedBackend: EngineBackend | null = null;
let sharedWorker: Worker | null = null;

function createWorkerBackend(): WorkerBackend {
  const worker = new Worker(new URL('./engine-worker.ts', import.meta.url), {
    type: 'module',
  });
  sharedWorker = worker;

  return new WorkerBackend(
    (msg: WorkerRequest) => {
      worker.postMessage(msg);
    },
    (callback: (msg: WorkerResponse) => void) => {
      worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
        callback(event.data);
      };
    },
  );
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
}
