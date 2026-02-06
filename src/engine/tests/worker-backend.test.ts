// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { readFileSync } from 'fs';
import { join } from 'path';

import { WorkerBackend } from '../src/worker-backend';
import { WorkerServer } from '../src/worker-server';
import type { WorkerRequest, WorkerResponse } from '../src/worker-protocol';
import type { ProjectHandle, ModelHandle, SimHandle } from '../src/backend';

const wasmPath = join(__dirname, '..', 'core', 'libsimlin.wasm');

function loadTestXmile(): Uint8Array {
  const xmilePath = join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  return readFileSync(xmilePath);
}

function loadWasmSource(): Uint8Array {
  return readFileSync(wasmPath);
}

interface TestPair {
  backend: WorkerBackend;
  server: WorkerServer;
  /** All transfer lists passed to postMessage, in order. */
  transfers: (Transferable[] | undefined)[];
}

/**
 * Create a WorkerBackend connected to a WorkerServer via direct function calls
 * (no actual Worker thread). This simulates the postMessage channel.
 */
function createTestPair(): TestPair {
  let backendOnMessage: ((msg: WorkerResponse) => void) | null = null;
  const transfers: (Transferable[] | undefined)[] = [];

  const server = new WorkerServer((msg: WorkerResponse) => {
    // Server -> Backend: simulate worker posting back
    if (backendOnMessage) {
      // Use setTimeout to simulate async message delivery
      setTimeout(() => backendOnMessage!(msg), 0);
    }
  });

  const backend = new WorkerBackend(
    // Backend -> Server: simulate main thread posting to worker
    (msg: WorkerRequest, transfer?: Transferable[]) => {
      transfers.push(transfer);
      // Deliver to server asynchronously to match real Worker behavior
      setTimeout(() => server.handleMessage(msg), 0);
    },
    // Register the callback for receiving messages from the server
    (callback: (msg: WorkerResponse) => void) => {
      backendOnMessage = callback;
    },
  );

  return { backend, server, transfers };
}

describe('WorkerBackend', () => {
  describe('lifecycle', () => {
    test('init -> isInitialized returns true', async () => {
      const { backend } = createTestPair();
      expect(backend.isInitialized()).toBe(false);
      await backend.init(loadWasmSource());
      expect(backend.isInitialized()).toBe(true);
    });

    test('double init is idempotent', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());
      await backend.init(loadWasmSource());
      expect(backend.isInitialized()).toBe(true);
    });

    test('reset returns to uninitialized', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());
      expect(backend.isInitialized()).toBe(true);
      await backend.reset();
      expect(backend.isInitialized()).toBe(false);
    });

    test('configureWasm after init throws', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());
      expect(() => backend.configureWasm({ source: loadWasmSource() })).toThrow(/already initialized/i);
    });

    test('configureWasm during init throws', async () => {
      const { backend } = createTestPair();
      // Start init but don't await it yet
      const initPromise = backend.init(loadWasmSource());
      // configureWasm should reject because init is in progress
      expect(() => backend.configureWasm({ source: loadWasmSource() })).toThrow(/already initialized/i);
      await initPromise;
    });

    test('init with string path forwards to worker', async () => {
      const { backend } = createTestPair();
      await backend.init(wasmPath);
      expect(backend.isInitialized()).toBe(true);

      // Verify it actually works by opening a project
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    });

    test('init with URL object forwards to worker', async () => {
      const { backend } = createTestPair();
      const url = new URL(`file://${wasmPath}`);
      await backend.init(url);
      expect(backend.isInitialized()).toBe(true);

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    });

    test('configureWasm with string path is forwarded during init', async () => {
      const { backend } = createTestPair();
      // Reset to clear global WASM state from prior tests, since
      // configureWasm requires WASM to not yet be initialized.
      await backend.reset();
      backend.configureWasm({ source: wasmPath });
      await backend.init();
      expect(backend.isInitialized()).toBe(true);

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    });

    test('init with provider function returning string path', async () => {
      const { backend } = createTestPair();
      await backend.init(() => wasmPath);
      expect(backend.isInitialized()).toBe(true);

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    });
  });

  describe('WASM buffer transfer', () => {
    test('init with Uint8Array transfers the buffer', async () => {
      const { backend, transfers } = createTestPair();
      const wasmBuffer = loadWasmSource();
      await backend.init(wasmBuffer);

      // The init message should have a transfer list with the buffer
      const initTransfer = transfers.find((t) => t !== undefined && t.length > 0);
      expect(initTransfer).toBeDefined();
      expect(initTransfer![0]).toBeInstanceOf(ArrayBuffer);
    });

    test('init with partial Uint8Array view slices instead of transferring', async () => {
      const { backend, transfers } = createTestPair();
      const wasmBuffer = loadWasmSource();
      // Create a view that is offset into a larger buffer
      const largerBuffer = new ArrayBuffer(wasmBuffer.byteLength + 16);
      new Uint8Array(largerBuffer).set(wasmBuffer, 8);
      const partialView = new Uint8Array(largerBuffer, 8, wasmBuffer.byteLength);

      await backend.init(partialView);

      // The init message should still have a transfer list (with the sliced buffer)
      const initTransfer = transfers.find((t) => t !== undefined && t.length > 0);
      expect(initTransfer).toBeDefined();
      // The transferred buffer should be the sliced copy, not the original larger buffer
      const transferredBuffer = initTransfer![0] as ArrayBuffer;
      expect(transferredBuffer.byteLength).toBe(wasmBuffer.byteLength);
      // The original larger buffer should NOT be neutered/detached
      expect(largerBuffer.byteLength).toBe(wasmBuffer.byteLength + 16);
    });

    test('init with string path does not transfer', async () => {
      const { backend, transfers } = createTestPair();
      await backend.init(wasmPath);

      // No buffer to transfer for string paths
      const initTransfer = transfers.find((t) => t !== undefined && t.length > 0);
      expect(initTransfer).toBeUndefined();
    });

    test('configureWasm with buffer transfers during init', async () => {
      const { backend, transfers } = createTestPair();
      await backend.reset();
      backend.configureWasm({ source: loadWasmSource() });
      await backend.init();

      // configureWasm message should have transferred the buffer
      const bufferTransfers = transfers.filter((t) => t !== undefined && t.length > 0);
      expect(bufferTransfers.length).toBeGreaterThanOrEqual(1);
    });
  });

  describe('project operations', () => {
    let backend: WorkerBackend;

    beforeEach(async () => {
      const pair = createTestPair();
      backend = pair.backend;
      await backend.init(loadWasmSource());
    });

    test('open XMILE project and get model count', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      expect(handle).toBeDefined();
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    });

    test('open protobuf project and serialize roundtrip', async () => {
      const xmileData = loadTestXmile();
      const handle1 = await backend.projectOpenXmile(xmileData);
      const pbData = await backend.projectSerializeProtobuf(handle1);
      expect(pbData).toBeInstanceOf(Uint8Array);
      expect(pbData.length).toBeGreaterThan(0);

      // Round-trip through protobuf
      const handle2 = await backend.projectOpenProtobuf(pbData);
      const count = await backend.projectGetModelCount(handle2);
      expect(count).toBe(1);
    });

    test('get model names', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const names = await backend.projectGetModelNames(handle);
      expect(names).toBeInstanceOf(Array);
      expect(names.length).toBeGreaterThan(0);
    });

    test('get model handle', async () => {
      const data = loadTestXmile();
      const projHandle = await backend.projectOpenXmile(data);
      const modelHandle = await backend.projectGetModel(projHandle, null);
      expect(modelHandle).toBeDefined();
    });

    test('isSimulatable returns true for valid model', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const result = await backend.projectIsSimulatable(handle, null);
      expect(result).toBe(true);
    });

    test('getErrors returns array', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const errors = await backend.projectGetErrors(handle);
      expect(errors).toBeInstanceOf(Array);
    });

    test('getLoops returns array', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      const loops = await backend.projectGetLoops(handle);
      expect(loops).toBeInstanceOf(Array);
    });

    test('dispose is idempotent', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      await backend.projectDispose(handle);
      // Second dispose should not throw
      await backend.projectDispose(handle);
    });

    test('operations on disposed handle throw', async () => {
      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);
      await backend.projectDispose(handle);
      await expect(backend.projectGetModelCount(handle)).rejects.toThrow();
    });
  });

  describe('model operations', () => {
    let backend: WorkerBackend;
    let projHandle: ProjectHandle;
    let modelHandle: ModelHandle;

    beforeEach(async () => {
      const pair = createTestPair();
      backend = pair.backend;
      await backend.init(loadWasmSource());
      const data = loadTestXmile();
      projHandle = await backend.projectOpenXmile(data);
      modelHandle = await backend.projectGetModel(projHandle, null);
    });

    test('getLinks returns array', async () => {
      const links = await backend.modelGetLinks(modelHandle);
      expect(links).toBeInstanceOf(Array);
      expect(links.length).toBeGreaterThan(0);
    });

    test('getIncomingLinks returns array', async () => {
      const links = await backend.modelGetIncomingLinks(modelHandle, 'teacup_temperature');
      expect(links).toBeInstanceOf(Array);
    });

    test('getLatexEquation returns string or null', async () => {
      const result = await backend.modelGetLatexEquation(modelHandle, 'teacup_temperature');
      // May be null or string depending on the model
      expect(result === null || typeof result === 'string').toBe(true);
    });

    test('dispose model is idempotent', async () => {
      await backend.modelDispose(modelHandle);
      await backend.modelDispose(modelHandle);
    });
  });

  describe('sim operations', () => {
    let backend: WorkerBackend;
    let projHandle: ProjectHandle;
    let modelHandle: ModelHandle;

    beforeEach(async () => {
      const pair = createTestPair();
      backend = pair.backend;
      await backend.init(loadWasmSource());
      const data = loadTestXmile();
      projHandle = await backend.projectOpenXmile(data);
      modelHandle = await backend.projectGetModel(projHandle, null);
    });

    test('create sim and run to end', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      expect(simHandle).toBeDefined();
      await backend.simRunToEnd(simHandle);
      const stepCount = await backend.simGetStepCount(simHandle);
      expect(stepCount).toBeGreaterThan(0);
    });

    test('get and set value', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      const time = await backend.simGetTime(simHandle);
      expect(typeof time).toBe('number');

      // Set a value and verify
      await backend.simSetValue(simHandle, 'teacup_temperature', 100);
      const value = await backend.simGetValue(simHandle, 'teacup_temperature');
      expect(value).toBe(100);
    });

    test('get series after run', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      await backend.simRunToEnd(simHandle);
      const series = await backend.simGetSeries(simHandle, 'teacup_temperature');
      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBeGreaterThan(0);
    });

    test('getVarNames returns array', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      const names = await backend.simGetVarNames(simHandle);
      expect(names).toBeInstanceOf(Array);
      expect(names.length).toBeGreaterThan(0);
    });

    test('getLinks returns array', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      await backend.simRunToEnd(simHandle);
      const links = await backend.simGetLinks(simHandle);
      expect(links).toBeInstanceOf(Array);
    });

    test('sim reset restores initial state', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      await backend.simRunToEnd(simHandle);
      const timeAfterRun = await backend.simGetTime(simHandle);
      expect(timeAfterRun).toBeGreaterThan(0);

      await backend.simReset(simHandle);
      const timeAfterReset = await backend.simGetTime(simHandle);
      expect(timeAfterReset).toBe(0);
    });

    test('dispose sim is idempotent', async () => {
      const simHandle = await backend.simNew(modelHandle, false);
      await backend.simDispose(simHandle);
      await backend.simDispose(simHandle);
    });
  });

  describe('strict serialization', () => {
    test('concurrent operations are serialized', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);

      // Fire multiple operations concurrently - they should all complete
      const [count, names, simulatable, errors] = await Promise.all([
        backend.projectGetModelCount(handle),
        backend.projectGetModelNames(handle),
        backend.projectIsSimulatable(handle, null),
        backend.projectGetErrors(handle),
      ]);

      expect(count).toBe(1);
      expect(names.length).toBeGreaterThan(0);
      expect(simulatable).toBe(true);
      expect(errors).toBeInstanceOf(Array);
    });
  });

  describe('error propagation', () => {
    test('operations before init are rejected', async () => {
      const { backend } = createTestPair();
      // Don't init
      await expect(backend.projectOpenXmile(new Uint8Array([]))).rejects.toThrow(/not ready/i);
    });
  });

  describe('terminate', () => {
    test('terminate rejects pending requests', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);

      // Fire an operation and terminate before it resolves
      const pendingOp = backend.projectGetModelCount(handle);
      backend.terminate();

      await expect(pendingOp).rejects.toThrow(/terminated/i);
    });

    test('terminate rejects queued requests', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());

      const data = loadTestXmile();
      const handle = await backend.projectOpenXmile(data);

      // Queue up multiple operations to ensure some are queued (not yet sent)
      const op1 = backend.projectGetModelCount(handle);
      const op2 = backend.projectGetModelNames(handle);
      const op3 = backend.projectGetErrors(handle);
      backend.terminate();

      await expect(op1).rejects.toThrow(/terminated/i);
      await expect(op2).rejects.toThrow(/terminated/i);
      await expect(op3).rejects.toThrow(/terminated/i);
    });

    test('operations after terminate are rejected', async () => {
      const { backend } = createTestPair();
      await backend.init(loadWasmSource());
      backend.terminate();

      await expect(backend.projectOpenXmile(loadTestXmile())).rejects.toThrow(/terminated/i);
    });
  });
});
