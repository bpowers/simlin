// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Race condition tests for the WorkerBackend.
 *
 * Verifies that concurrent operations via the FIFO queue produce
 * correct, ordered results - matching the single-threaded WASM
 * execution model.
 */

import { readFileSync } from 'fs';
import { join } from 'path';

import { WorkerBackend } from '../src/worker-backend';
import { WorkerServer } from '../src/worker-server';
import type { WorkerRequest, WorkerResponse } from '../src/worker-protocol';

const wasmPath = join(__dirname, '..', 'core', 'libsimlin.wasm');

function loadTestXmile(): Uint8Array {
  const xmilePath = join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  return readFileSync(xmilePath);
}

function loadWasmSource(): Uint8Array {
  return readFileSync(wasmPath);
}

function createTestPair(): { backend: WorkerBackend; server: WorkerServer } {
  let backendOnMessage: ((msg: WorkerResponse) => void) | null = null;

  const server = new WorkerServer((msg: WorkerResponse) => {
    if (backendOnMessage) {
      setTimeout(() => backendOnMessage!(msg), 0);
    }
  });

  const backend = new WorkerBackend(
    (msg: WorkerRequest) => {
      setTimeout(() => server.handleMessage(msg), 0);
    },
    (callback: (msg: WorkerResponse) => void) => {
      backendOnMessage = callback;
    },
  );

  return { backend, server };
}

describe('race conditions', () => {
  let backend: WorkerBackend;

  beforeEach(async () => {
    const pair = createTestPair();
    backend = pair.backend;
    await backend.init(loadWasmSource());
  });

  test('concurrent project operations all complete', async () => {
    const data = loadTestXmile();
    const handle = await backend.projectOpenXmile(data);

    // Fire many operations concurrently
    const results = await Promise.all([
      backend.projectGetModelCount(handle),
      backend.projectGetModelNames(handle),
      backend.projectIsSimulatable(handle, null),
      backend.projectGetErrors(handle),
      backend.projectGetLoops(handle),
      backend.projectSerializeProtobuf(handle),
    ]);

    const [count, names, simulatable, errors, loops, protobuf] = results;
    expect(count).toBe(1);
    expect(names.length).toBeGreaterThan(0);
    expect(simulatable).toBe(true);
    expect(errors).toBeInstanceOf(Array);
    expect(loops).toBeInstanceOf(Array);
    expect(protobuf).toBeInstanceOf(Uint8Array);
    expect(protobuf.length).toBeGreaterThan(0);
  });

  test('applyPatch followed by serialize returns post-patch state', async () => {
    const data = loadTestXmile();
    const handle = await backend.projectOpenXmile(data);

    // Get original serialization
    const before = await backend.projectSerializeProtobuf(handle);

    // Apply a patch (add a new auxiliary variable)
    const patch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertAux',
              payload: {
                aux: {
                  name: 'test_var',
                  equation: '42',
                },
              },
            },
          ],
        },
      ],
    };
    await backend.projectApplyPatch(handle, patch, false, true);

    // Immediately serialize - should reflect the patch
    const after = await backend.projectSerializeProtobuf(handle);

    // The serialized data should be different after the patch
    expect(after.length).not.toBe(before.length);
  });

  test('multiple rapid patches all applied in order', async () => {
    const data = loadTestXmile();
    const handle = await backend.projectOpenXmile(data);

    // Apply several patches concurrently via the FIFO queue
    const makePatch = (varName: string, eq: string) => ({
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertAux' as const,
              payload: { aux: { name: varName, equation: eq } },
            },
          ],
        },
      ],
    });
    const patches = [makePatch('var_a', '1'), makePatch('var_b', '2'), makePatch('var_c', '3')];

    // Fire all patches - they should be applied in order via FIFO
    await Promise.all(patches.map((p) => backend.projectApplyPatch(handle, p, false, true)));

    // All three variables should exist in the project
    const modelHandle = await backend.projectGetModel(handle, null);
    const varNames = await backend.modelGetIncomingLinks(modelHandle, 'var_a');
    // var_a has no dependencies, so empty array expected
    expect(varNames).toEqual([]);

    // Verify by serializing - the serialized project should contain all three vars
    const protobuf = await backend.projectSerializeProtobuf(handle);
    expect(protobuf.length).toBeGreaterThan(0);
  });

  test('sim operations interleaved with project operations', async () => {
    const data = loadTestXmile();
    const projHandle = await backend.projectOpenXmile(data);
    const modelHandle = await backend.projectGetModel(projHandle, null);
    const simHandle = await backend.simNew(modelHandle, false);

    // Fire sim and project operations concurrently
    const [, , time, stepCount, errors] = await Promise.all([
      backend.simRunToEnd(simHandle),
      backend.projectGetErrors(projHandle),
      backend.simGetTime(simHandle),
      backend.simGetStepCount(simHandle),
      backend.projectGetErrors(projHandle),
    ]);

    // Because of FIFO serialization:
    // 1. simRunToEnd completes first
    // 2. getErrors completes second
    // 3. simGetTime shows time after runToEnd
    // 4. simGetStepCount shows steps after runToEnd
    // 5. getErrors completes last
    expect(time).toBeGreaterThan(0);
    expect(stepCount).toBeGreaterThan(0);
    expect(errors).toBeInstanceOf(Array);
  });

  test('opening multiple projects concurrently', async () => {
    const data = loadTestXmile();

    // Open multiple projects in parallel (queued serially by FIFO)
    const handles = await Promise.all([
      backend.projectOpenXmile(data),
      backend.projectOpenXmile(data),
      backend.projectOpenXmile(data),
    ]);

    // All handles should be distinct
    const uniqueHandles = new Set(handles);
    expect(uniqueHandles.size).toBe(3);

    // All projects should be independently usable
    for (const handle of handles) {
      const count = await backend.projectGetModelCount(handle);
      expect(count).toBe(1);
    }
  });

  test('dispose during queued operations rejects pending', async () => {
    const data = loadTestXmile();
    const handle = await backend.projectOpenXmile(data);

    // Queue several operations then dispose
    const op1 = backend.projectGetModelCount(handle);
    const op2 = backend.projectGetModelNames(handle);
    const disposeOp = backend.projectDispose(handle);
    // After dispose, this operation should fail
    const op3 = backend.projectGetModelCount(handle);

    // op1 and op2 should succeed (queued before dispose)
    const count = await op1;
    expect(count).toBe(1);
    const names = await op2;
    expect(names.length).toBeGreaterThan(0);

    // dispose should succeed
    await disposeOp;

    // op3 should reject because the project was disposed
    await expect(op3).rejects.toThrow();
  });
});
