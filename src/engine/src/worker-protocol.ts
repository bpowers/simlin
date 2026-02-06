// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Typed message protocol for communication between the main thread
 * and the engine Web Worker.
 *
 * All messages use discriminated unions on the `type` field.
 * Each request has a `requestId` for correlating responses.
 * Handles are opaque integers that reference WASM objects in the worker.
 */

import type { ErrorDetail } from './internal/types';

// Branded handle types for messages (mirrors backend.ts, but without class brands)
export type WorkerProjectHandle = number;
export type WorkerModelHandle = number;
export type WorkerSimHandle = number;

/**
 * Serializable error detail for transmission over postMessage.
 * Matches ErrorDetail but uses plain objects only.
 */
export interface SerializedError {
  name: string;
  message: string;
  code?: number;
  details?: ErrorDetail[];
}

// ---- Request Messages ----

export type WorkerRequest =
  // Lifecycle
  | { type: 'init'; requestId: number; wasmSource?: ArrayBuffer; wasmUrl?: string }
  | { type: 'isInitialized'; requestId: number }
  | { type: 'reset'; requestId: number }
  | { type: 'configureWasm'; requestId: number; config: { source?: ArrayBuffer; url?: string } }
  // Project open
  | { type: 'projectOpenXmile'; requestId: number; data: Uint8Array }
  | { type: 'projectOpenProtobuf'; requestId: number; data: Uint8Array }
  | { type: 'projectOpenJson'; requestId: number; data: Uint8Array; format: number }
  | { type: 'projectOpenVensim'; requestId: number; data: Uint8Array }
  // Project operations
  | { type: 'projectDispose'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectGetModelCount'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectGetModelNames'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectGetModel'; requestId: number; handle: WorkerProjectHandle; name: string | null }
  | { type: 'projectIsSimulatable'; requestId: number; handle: WorkerProjectHandle; modelName: string | null }
  | { type: 'projectSerializeProtobuf'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectSerializeJson'; requestId: number; handle: WorkerProjectHandle; format: number }
  | { type: 'projectSerializeXmile'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectRenderSvg'; requestId: number; handle: WorkerProjectHandle; modelName: string }
  | { type: 'projectGetLoops'; requestId: number; handle: WorkerProjectHandle }
  | { type: 'projectGetErrors'; requestId: number; handle: WorkerProjectHandle }
  | {
      type: 'projectApplyPatch';
      requestId: number;
      handle: WorkerProjectHandle;
      patchJson: string;
      dryRun: boolean;
      allowErrors: boolean;
    }
  // Model operations
  | { type: 'modelDispose'; requestId: number; handle: WorkerModelHandle }
  | { type: 'modelGetIncomingLinks'; requestId: number; handle: WorkerModelHandle; varName: string }
  | { type: 'modelGetLinks'; requestId: number; handle: WorkerModelHandle }
  | { type: 'modelGetLatexEquation'; requestId: number; handle: WorkerModelHandle; ident: string }
  // Sim operations
  | { type: 'simNew'; requestId: number; modelHandle: WorkerModelHandle; enableLtm: boolean }
  | { type: 'simDispose'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simRunTo'; requestId: number; handle: WorkerSimHandle; time: number }
  | { type: 'simRunToEnd'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simReset'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simGetTime'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simGetStepCount'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simGetValue'; requestId: number; handle: WorkerSimHandle; name: string }
  | { type: 'simSetValue'; requestId: number; handle: WorkerSimHandle; name: string; value: number }
  | { type: 'simGetSeries'; requestId: number; handle: WorkerSimHandle; name: string }
  | { type: 'simGetVarNames'; requestId: number; handle: WorkerSimHandle }
  | { type: 'simGetLinks'; requestId: number; handle: WorkerSimHandle };

// ---- Response Messages ----

export type WorkerResponse =
  | { type: 'success'; requestId: number; result: unknown; transfer?: ArrayBuffer[] }
  | { type: 'error'; requestId: number; error: SerializedError };

// ---- Worker State Machine ----

export enum WorkerState {
  UNINITIALIZED = 'UNINITIALIZED',
  INITIALIZING = 'INITIALIZING',
  READY = 'READY',
  FAILED = 'FAILED',
}

/**
 * Serialize an error into a plain object suitable for postMessage.
 */
export function serializeError(err: unknown): SerializedError {
  if (err instanceof Error) {
    const serialized: SerializedError = {
      name: err.name,
      message: err.message,
    };
    // Handle SimlinError-like objects with code and details
    const errAny = err as unknown as Record<string, unknown>;
    if (typeof errAny['code'] === 'number') {
      serialized.code = errAny['code'] as number;
    }
    if (Array.isArray(errAny['details'])) {
      serialized.details = errAny['details'] as ErrorDetail[];
    }
    return serialized;
  }
  return {
    name: 'Error',
    message: String(err),
  };
}

/**
 * Deserialize a SerializedError back into an Error object.
 */
export function deserializeError(serialized: SerializedError): Error {
  const err = new Error(serialized.message) as Error & {
    code?: number;
    details?: ErrorDetail[];
  };
  err.name = serialized.name;
  if (serialized.code !== undefined) {
    err.code = serialized.code;
  }
  if (serialized.details !== undefined) {
    err.details = serialized.details;
  }
  return err;
}

// ---- Type Guards ----

/**
 * All valid request type strings.
 */
export const VALID_REQUEST_TYPES: ReadonlySet<string> = new Set([
  'init',
  'isInitialized',
  'reset',
  'configureWasm',
  'projectOpenXmile',
  'projectOpenProtobuf',
  'projectOpenJson',
  'projectOpenVensim',
  'projectDispose',
  'projectGetModelCount',
  'projectGetModelNames',
  'projectGetModel',
  'projectIsSimulatable',
  'projectSerializeProtobuf',
  'projectSerializeJson',
  'projectSerializeXmile',
  'projectRenderSvg',
  'projectGetLoops',
  'projectGetErrors',
  'projectApplyPatch',
  'modelDispose',
  'modelGetIncomingLinks',
  'modelGetLinks',
  'modelGetLatexEquation',
  'simNew',
  'simDispose',
  'simRunTo',
  'simRunToEnd',
  'simReset',
  'simGetTime',
  'simGetStepCount',
  'simGetValue',
  'simSetValue',
  'simGetSeries',
  'simGetVarNames',
  'simGetLinks',
]);

export function isValidRequest(msg: unknown): msg is WorkerRequest {
  if (typeof msg !== 'object' || msg === null) return false;
  const obj = msg as Record<string, unknown>;
  return (
    typeof obj['type'] === 'string' && VALID_REQUEST_TYPES.has(obj['type']) && typeof obj['requestId'] === 'number'
  );
}
