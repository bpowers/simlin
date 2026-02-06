// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * EngineBackend interface: an abstraction over WASM engine operations.
 *
 * DirectBackend calls WASM directly (Node.js / same-thread).
 * WorkerBackend forwards calls to a Web Worker (browser).
 *
 * All WASM pointers are replaced by opaque handles (non-zero integers).
 * The backend owns the pointer-to-handle mapping.
 */

import { ErrorDetail, SimlinJsonFormat } from './internal/types';
import { Loop, Link } from './types';
import { JsonProjectPatch } from './json-types';
import { WasmConfig, WasmSourceProvider } from '@simlin/engine/internal/wasm';

// Branded handle types for type safety at the TypeScript level
export type Handle = number;
export type ProjectHandle = Handle & { __brand: 'project' };
export type ModelHandle = Handle & { __brand: 'model' };
export type SimHandle = Handle & { __brand: 'sim' };

export interface SimRunResult {
  varNames: string[];
  results: Map<string, Float64Array>;
  stepCount: number;
  loops: Loop[];
  links: Link[];
  overrides: Record<string, number>;
}

export interface EngineBackend {
  // Lifecycle
  init(wasmSource?: WasmSourceProvider): Promise<void>;
  isInitialized(): boolean;
  reset(): void;
  configureWasm(config: WasmConfig): void;

  // Project open operations
  projectOpenXmile(data: Uint8Array): ProjectHandle;
  projectOpenProtobuf(data: Uint8Array): ProjectHandle;
  projectOpenJson(data: Uint8Array, format: SimlinJsonFormat): ProjectHandle;
  projectOpenVensim(data: Uint8Array): ProjectHandle;

  // Project operations
  projectDispose(handle: ProjectHandle): void;
  projectGetModelCount(handle: ProjectHandle): number;
  projectGetModelNames(handle: ProjectHandle): string[];
  projectGetModel(handle: ProjectHandle, name: string | null): ModelHandle;
  projectIsSimulatable(handle: ProjectHandle, modelName: string | null): boolean;
  projectSerializeProtobuf(handle: ProjectHandle): Uint8Array;
  projectSerializeJson(handle: ProjectHandle, format: SimlinJsonFormat): Uint8Array;
  projectSerializeXmile(handle: ProjectHandle): Uint8Array;
  projectRenderSvg(handle: ProjectHandle, modelName: string): Uint8Array;
  projectGetLoops(handle: ProjectHandle): Loop[];
  projectGetErrors(handle: ProjectHandle): ErrorDetail[];
  projectApplyPatch(
    handle: ProjectHandle,
    patch: JsonProjectPatch,
    dryRun: boolean,
    allowErrors: boolean,
  ): ErrorDetail[];

  // Model operations
  modelDispose(handle: ModelHandle): void;
  modelGetIncomingLinks(handle: ModelHandle, varName: string): string[];
  modelGetLinks(handle: ModelHandle): Link[];
  modelGetLatexEquation(handle: ModelHandle, ident: string): string | null;

  // Sim operations
  simNew(modelHandle: ModelHandle, enableLtm: boolean): SimHandle;
  simDispose(handle: SimHandle): void;
  simRunTo(handle: SimHandle, time: number): void;
  simRunToEnd(handle: SimHandle): void;
  simReset(handle: SimHandle): void;
  simGetTime(handle: SimHandle): number;
  simGetStepCount(handle: SimHandle): number;
  simGetValue(handle: SimHandle, name: string): number;
  simSetValue(handle: SimHandle, name: string, value: number): void;
  simGetSeries(handle: SimHandle, name: string): Float64Array;
  simGetVarNames(handle: SimHandle): string[];
  simGetLinks(handle: SimHandle): Link[];
}
