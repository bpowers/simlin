// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Web Worker entry point for the engine.
 *
 * Instantiates a WorkerServer and wires it to the Worker's message handler.
 * This file is excluded from the Node.js build (CommonJS tsconfig) and
 * included only in the browser build.
 */

import { WorkerServer } from './worker-server';
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

const server = new WorkerServer((msg: WorkerResponse, transfer?: Transferable[]) => {
  if (transfer && transfer.length > 0) {
    workerSelf.postMessage(msg, transfer);
  } else {
    workerSelf.postMessage(msg);
  }
});

workerSelf.onmessage = (event: MessageEvent) => {
  server.handleMessage(event.data);
};
