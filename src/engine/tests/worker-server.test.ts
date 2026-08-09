// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect, beforeAll, beforeEach } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import { WorkerServer } from '../src/worker-server';
import {
  WorkerRequest,
  WorkerResponse,
  serializeError,
  deserializeError,
  isValidRequest,
} from '../src/worker-protocol';
import { configureWasm, ready, resetWasm } from '../src/index';

// Helper to set up WASM for tests
async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

/**
 * Create a WorkerServer and helper for collecting responses.
 */
function createTestServer(): {
  server: WorkerServer;
  responses: WorkerResponse[];
  sendAndWait: (request: WorkerRequest) => Promise<WorkerResponse>;
} {
  const responses: WorkerResponse[] = [];
  const pendingResolvers = new Map<number, (resp: WorkerResponse) => void>();

  const server = new WorkerServer((msg: WorkerResponse) => {
    responses.push(msg);
    const resolver = pendingResolvers.get(msg.requestId);
    if (resolver) {
      pendingResolvers.delete(msg.requestId);
      resolver(msg);
    }
  });

  const sendAndWait = (request: WorkerRequest): Promise<WorkerResponse> => {
    return new Promise<WorkerResponse>((resolve) => {
      pendingResolvers.set(request.requestId, resolve);
      server.handleMessage(request);
      // For sync operations, the response is sent immediately
      if (pendingResolvers.has(request.requestId)) {
        // Still pending, it might be async (init)
        // The resolver will be called when the response comes
      }
    });
  };

  return { server, responses, sendAndWait };
}

let requestIdCounter = 1;
function nextRequestId(): number {
  return requestIdCounter++;
}

describe('WorkerServer', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  beforeEach(() => {
    requestIdCounter = 1;
  });

  // The state machine is observed through behavior rather than through a
  // getter that exists only for tests: UNINITIALIZED means operations answer
  // "not ready", READY means they are served.
  describe('state machine', () => {
    it('rejects operations before init', () => {
      const { server, responses } = createTestServer();

      server.handleMessage({
        type: 'projectGetModelCount',
        requestId: nextRequestId(),
        handle: 1,
      });

      const resp = responses[responses.length - 1];
      expect(resp.type).toBe('error');
      if (resp.type === 'error') {
        expect(resp.error.message).toContain('not ready');
      }
    });

    it('double-init is idempotent and leaves the server serving operations', async () => {
      const { sendAndWait } = createTestServer();

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);
      const source = wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength);

      const resp1 = await sendAndWait({ type: 'init', requestId: nextRequestId(), wasmSource: source });
      expect(resp1.type).toBe('success');

      // The second init carries no source: it must be a no-op on an already
      // initialized server rather than tearing the engine down.
      const resp2 = await sendAndWait({ type: 'init', requestId: nextRequestId() });
      expect(resp2.type).toBe('success');

      const opResp = await sendAndWait({ type: 'projectOpenXmile', requestId: nextRequestId(), data: loadTestXmile() });
      expect(opResp.type).toBe('success');
    });

    it('reset returns the server to the pre-init state', async () => {
      const { sendAndWait } = createTestServer();

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      await sendAndWait({
        type: 'init',
        requestId: nextRequestId(),
        wasmSource: wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength),
      });
      await sendAndWait({ type: 'reset', requestId: nextRequestId() });

      const resp = await sendAndWait({ type: 'projectOpenXmile', requestId: nextRequestId(), data: loadTestXmile() });
      expect(resp.type).toBe('error');
      if (resp.type === 'error') {
        expect(resp.error.message).toContain('not ready');
      }
    });
  });

  // Open/serialize/model-names/simulatable go through this same server one
  // layer up in worker-backend.test.ts (its WorkerBackend talks to a real
  // WorkerServer), so only the patch path -- which no backend test drives --
  // is exercised directly here.
  describe('project operations', () => {
    let sendAndWait: ReturnType<typeof createTestServer>['sendAndWait'];

    beforeEach(async () => {
      const test = createTestServer();
      sendAndWait = test.sendAndWait;

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);
      await sendAndWait({
        type: 'init',
        requestId: nextRequestId(),
        wasmSource: wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength),
      });
    });

    it('applies a patch', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const patch = {
        models: [
          {
            name: 'main',
            ops: [
              {
                type: 'upsertAux',
                payload: { aux: { name: 'test_var', equation: '42' } },
              },
            ],
          },
        ],
      };

      const patchResp = await sendAndWait({
        type: 'projectApplyPatch',
        requestId: nextRequestId(),
        handle: projectHandle,
        patchJson: JSON.stringify(patch),
        dryRun: false,
        allowErrors: true,
      });
      expect(patchResp.type).toBe('success');
    });
  });

  describe('model and sim operations', () => {
    let sendAndWait: ReturnType<typeof createTestServer>['sendAndWait'];
    let projectHandle: number;

    beforeEach(async () => {
      const test = createTestServer();
      sendAndWait = test.sendAndWait;

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);
      await sendAndWait({
        type: 'init',
        requestId: nextRequestId(),
        wasmSource: wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength),
      });

      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;
    });

    it('gets a model handle and retrieves links', async () => {
      const modelResp = await sendAndWait({
        type: 'projectGetModel',
        requestId: nextRequestId(),
        handle: projectHandle,
        name: null,
      });
      expect(modelResp.type).toBe('success');
      const modelHandle = (modelResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const linksResp = await sendAndWait({
        type: 'modelGetLinks',
        requestId: nextRequestId(),
        handle: modelHandle,
      });
      expect(linksResp.type).toBe('success');
      if (linksResp.type === 'success') {
        expect(Array.isArray(linksResp.result)).toBe(true);
      }
    });

    it('creates a sim, runs to end, and gets series data', async () => {
      const modelResp = await sendAndWait({
        type: 'projectGetModel',
        requestId: nextRequestId(),
        handle: projectHandle,
        name: null,
      });
      const modelHandle = (modelResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const simResp = await sendAndWait({
        type: 'simNew',
        requestId: nextRequestId(),
        modelHandle,
        enableLtm: false,
      });
      expect(simResp.type).toBe('success');
      const simHandle = (simResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      // Run to end
      const runResp = await sendAndWait({
        type: 'simRunToEnd',
        requestId: nextRequestId(),
        handle: simHandle,
      });
      expect(runResp.type).toBe('success');

      // Get var names
      const namesResp = await sendAndWait({
        type: 'simGetVarNames',
        requestId: nextRequestId(),
        handle: simHandle,
      });
      expect(namesResp.type).toBe('success');
      const varNames = (namesResp as Extract<WorkerResponse, { type: 'success' }>).result as string[];
      expect(varNames.length).toBeGreaterThan(0);

      // Get series for first variable
      const seriesResp = await sendAndWait({
        type: 'simGetSeries',
        requestId: nextRequestId(),
        handle: simHandle,
        name: varNames[0],
      });
      expect(seriesResp.type).toBe('success');
      const series = (seriesResp as Extract<WorkerResponse, { type: 'success' }>).result;
      expect(series).toBeInstanceOf(Float64Array);
      expect((series as Float64Array).length).toBeGreaterThan(0);
    });
  });

  describe('handle disposal', () => {
    let sendAndWait: ReturnType<typeof createTestServer>['sendAndWait'];

    beforeEach(async () => {
      const test = createTestServer();
      sendAndWait = test.sendAndWait;

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);
      await sendAndWait({
        type: 'init',
        requestId: nextRequestId(),
        wasmSource: wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength),
      });
    });

    it('disposed project handle returns error on use', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      // Dispose
      await sendAndWait({
        type: 'projectDispose',
        requestId: nextRequestId(),
        handle: projectHandle,
      });

      // Try to use
      const resp = await sendAndWait({
        type: 'projectGetModelNames',
        requestId: nextRequestId(),
        handle: projectHandle,
      });
      expect(resp.type).toBe('error');
      if (resp.type === 'error') {
        expect(resp.error.message).toContain('Invalid or disposed');
      }
    });

    it('double dispose is idempotent', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const resp1 = await sendAndWait({
        type: 'projectDispose',
        requestId: nextRequestId(),
        handle: projectHandle,
      });
      expect(resp1.type).toBe('success');

      const resp2 = await sendAndWait({
        type: 'projectDispose',
        requestId: nextRequestId(),
        handle: projectHandle,
      });
      expect(resp2.type).toBe('success');
    });

    it('project dispose invalidates child model handles', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      // Get model handle
      const modelResp = await sendAndWait({
        type: 'projectGetModel',
        requestId: nextRequestId(),
        handle: projectHandle,
        name: null,
      });
      const modelHandle = (modelResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      // Dispose project
      await sendAndWait({
        type: 'projectDispose',
        requestId: nextRequestId(),
        handle: projectHandle,
      });

      // Model handle should be invalid
      const linksResp = await sendAndWait({
        type: 'modelGetLinks',
        requestId: nextRequestId(),
        handle: modelHandle,
      });
      expect(linksResp.type).toBe('error');
      if (linksResp.type === 'error') {
        expect(linksResp.error.message).toContain('Invalid or disposed');
      }
    });

    it('project dispose invalidates child sim handles', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const modelResp = await sendAndWait({
        type: 'projectGetModel',
        requestId: nextRequestId(),
        handle: projectHandle,
        name: null,
      });
      const modelHandle = (modelResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const simResp = await sendAndWait({
        type: 'simNew',
        requestId: nextRequestId(),
        modelHandle,
        enableLtm: false,
      });
      const simHandle = (simResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      // Dispose project
      await sendAndWait({
        type: 'projectDispose',
        requestId: nextRequestId(),
        handle: projectHandle,
      });

      // Sim handle should be invalid
      const timeResp = await sendAndWait({
        type: 'simGetTime',
        requestId: nextRequestId(),
        handle: simHandle,
      });
      expect(timeResp.type).toBe('error');
    });
  });

  describe('error serialization', () => {
    it('serializes and deserializes basic Error', () => {
      const err = new Error('test error');
      const serialized = serializeError(err);
      expect(serialized.name).toBe('Error');
      expect(serialized.message).toBe('test error');

      const deserialized = deserializeError(serialized);
      expect(deserialized.message).toBe('test error');
      expect(deserialized.name).toBe('Error');
    });

    it('serializes Error with code and details', () => {
      const err = new Error('sim error') as Error & { code: number; details: unknown[] };
      err.name = 'SimlinError';
      err.code = 42;
      err.details = [{ code: 1, message: 'var error', variableName: 'x' }];

      const serialized = serializeError(err);
      expect(serialized.name).toBe('SimlinError');
      expect(serialized.code).toBe(42);
      expect(serialized.details).toHaveLength(1);

      const deserialized = deserializeError(serialized) as Error & { code?: number; details?: unknown[] };
      expect(deserialized.name).toBe('SimlinError');
      expect(deserialized.code).toBe(42);
      expect(deserialized.details).toHaveLength(1);
    });

    it('serializes non-Error values', () => {
      const serialized = serializeError('string error');
      expect(serialized.name).toBe('Error');
      expect(serialized.message).toBe('string error');
    });
  });

  describe('protocol validation', () => {
    it('isValidRequest accepts valid messages', () => {
      expect(isValidRequest({ type: 'init', requestId: 1 })).toBe(true);
      expect(
        isValidRequest({
          type: 'projectOpenXmile',
          requestId: 2,
          data: new Uint8Array(),
        }),
      ).toBe(true);
    });

    it('isValidRequest rejects invalid messages', () => {
      expect(isValidRequest(null)).toBe(false);
      expect(isValidRequest(undefined)).toBe(false);
      expect(isValidRequest(42)).toBe(false);
      expect(isValidRequest({})).toBe(false);
      expect(isValidRequest({ type: 'init' })).toBe(false); // no requestId
      expect(isValidRequest({ type: 'invalidType', requestId: 1 })).toBe(false);
    });

    it('server ignores completely invalid messages', () => {
      const { server, responses } = createTestServer();
      server.handleMessage(null);
      server.handleMessage(42);
      server.handleMessage('string');
      expect(responses.length).toBe(0);
    });
  });

  describe('safe buffer transfers', () => {
    let sendAndWait: ReturnType<typeof createTestServer>['sendAndWait'];
    let transfers: Transferable[][];

    beforeEach(async () => {
      transfers = [];
      const responses: WorkerResponse[] = [];
      const pendingResolvers = new Map<number, (resp: WorkerResponse) => void>();

      const server = new WorkerServer((msg: WorkerResponse, transfer?: Transferable[]) => {
        if (transfer) {
          transfers.push(transfer);
        }
        responses.push(msg);
        const resolver = pendingResolvers.get(msg.requestId);
        if (resolver) {
          pendingResolvers.delete(msg.requestId);
          resolver(msg);
        }
      });

      sendAndWait = (request: WorkerRequest): Promise<WorkerResponse> => {
        return new Promise<WorkerResponse>((resolve) => {
          pendingResolvers.set(request.requestId, resolve);
          server.handleMessage(request);
        });
      };

      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);
      await sendAndWait({
        type: 'init',
        requestId: nextRequestId(),
        wasmSource: wasmBuffer.buffer.slice(wasmBuffer.byteOffset, wasmBuffer.byteOffset + wasmBuffer.byteLength),
      });
    });

    it('serialized protobuf buffer is independent (not a view into WASM memory)', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const resp = await sendAndWait({
        type: 'projectSerializeProtobuf',
        requestId: nextRequestId(),
        handle: projectHandle,
      });
      expect(resp.type).toBe('success');
      const result = (resp as Extract<WorkerResponse, { type: 'success' }>).result as Uint8Array;

      // The result's buffer should own exactly the data (not a view into a larger buffer)
      expect(result.byteOffset).toBe(0);
      expect(result.buffer.byteLength).toBe(result.byteLength);

      // Transfer list should contain exactly this buffer
      expect(transfers.length).toBeGreaterThan(0);
      const lastTransfer = transfers[transfers.length - 1];
      expect(lastTransfer.length).toBe(1);
      expect(lastTransfer[0]).toBe(result.buffer);
    });

    it('sim series buffer is independent (not a view into WASM memory)', async () => {
      const xmile = loadTestXmile();
      const openResp = await sendAndWait({
        type: 'projectOpenXmile',
        requestId: nextRequestId(),
        data: xmile,
      });
      const projectHandle = (openResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const modelResp = await sendAndWait({
        type: 'projectGetModel',
        requestId: nextRequestId(),
        handle: projectHandle,
        name: null,
      });
      const modelHandle = (modelResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      const simResp = await sendAndWait({
        type: 'simNew',
        requestId: nextRequestId(),
        modelHandle,
        enableLtm: false,
      });
      const simHandle = (simResp as Extract<WorkerResponse, { type: 'success' }>).result as number;

      await sendAndWait({
        type: 'simRunToEnd',
        requestId: nextRequestId(),
        handle: simHandle,
      });

      transfers = [];
      const seriesResp = await sendAndWait({
        type: 'simGetSeries',
        requestId: nextRequestId(),
        handle: simHandle,
        name: 'teacup_temperature',
      });
      expect(seriesResp.type).toBe('success');
      const series = (seriesResp as Extract<WorkerResponse, { type: 'success' }>).result as Float64Array;

      expect(series.byteOffset).toBe(0);
      expect(series.buffer.byteLength).toBe(series.byteLength);

      expect(transfers.length).toBe(1);
      expect(transfers[0][0]).toBe(series.buffer);
    });
  });
});
