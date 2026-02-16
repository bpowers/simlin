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

// Worker global scope interface for postMessage with Transferable support.
// We define this locally rather than adding the webworker lib (which conflicts
// with dom lib types).
interface WorkerGlobalScope {
  postMessage(message: unknown, transfer: Transferable[]): void;
  postMessage(message: unknown): void;
  onmessage: ((event: MessageEvent) => void) | null;
}

const workerSelf = self as unknown as WorkerGlobalScope;

// Buffer for messages that arrive before the dynamic import resolves.
let pendingMessages: unknown[] | null = [];

// Install the handler synchronously so no messages are dropped.
workerSelf.onmessage = (event: MessageEvent) => {
  if (pendingMessages !== null) {
    pendingMessages.push(event.data);
  }
};

import('./worker-server').then(({ WorkerServer }) => {
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
});
