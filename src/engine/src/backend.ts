// Copyright 2026 The Simlin Authors. All rights reserved.
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

/**
 * Return type helper: allows implementations to return either T or Promise<T>.
 * DirectBackend returns T synchronously; WorkerBackend returns Promise<T>.
 */
export type MaybePromise<T> = T | Promise<T>;

export interface EngineBackend {
  // Lifecycle
  init(wasmSource?: WasmSourceProvider): Promise<void>;
  isInitialized(): boolean;
  reset(): MaybePromise<void>;
  configureWasm(config: WasmConfig): void;

  // Project open operations
  projectOpenXmile(data: Uint8Array): MaybePromise<ProjectHandle>;
  projectOpenProtobuf(data: Uint8Array): MaybePromise<ProjectHandle>;
  projectOpenJson(data: Uint8Array, format: SimlinJsonFormat): MaybePromise<ProjectHandle>;
  projectOpenVensim(data: Uint8Array): MaybePromise<ProjectHandle>;

  // Project operations
  projectDispose(handle: ProjectHandle): MaybePromise<void>;
  projectGetModelCount(handle: ProjectHandle): MaybePromise<number>;
  projectGetModelNames(handle: ProjectHandle): MaybePromise<string[]>;
  projectGetModel(handle: ProjectHandle, name: string | null): MaybePromise<ModelHandle>;
  projectIsSimulatable(handle: ProjectHandle, modelName: string | null): MaybePromise<boolean>;
  projectSerializeProtobuf(handle: ProjectHandle): MaybePromise<Uint8Array>;
  projectSerializeJson(handle: ProjectHandle, format: SimlinJsonFormat): MaybePromise<Uint8Array>;
  projectSerializeXmile(handle: ProjectHandle): MaybePromise<Uint8Array>;
  projectRenderSvg(handle: ProjectHandle, modelName: string): MaybePromise<Uint8Array>;
  projectGetErrors(handle: ProjectHandle): MaybePromise<ErrorDetail[]>;
  projectApplyPatch(
    handle: ProjectHandle,
    patch: JsonProjectPatch,
    dryRun: boolean,
    allowErrors: boolean,
  ): MaybePromise<ErrorDetail[]>;

  // Model operations
  modelGetName(handle: ModelHandle): MaybePromise<string>;
  modelDispose(handle: ModelHandle): MaybePromise<void>;
  modelGetIncomingLinks(handle: ModelHandle, varName: string): MaybePromise<string[]>;
  modelGetLinks(handle: ModelHandle): MaybePromise<Link[]>;
  modelGetLoops(handle: ModelHandle): MaybePromise<Loop[]>;
  modelGetLatexEquation(handle: ModelHandle, ident: string): MaybePromise<string | null>;
  modelGetVarJson(handle: ModelHandle, varName: string): MaybePromise<Uint8Array>;
  modelGetVarNames(handle: ModelHandle, typeMask?: number, filter?: string | null): MaybePromise<string[]>;
  modelGetSimSpecsJson(handle: ModelHandle): MaybePromise<Uint8Array>;

  // Sim operations
  simNew(modelHandle: ModelHandle, enableLtm: boolean): MaybePromise<SimHandle>;
  simDispose(handle: SimHandle): MaybePromise<void>;
  simRunTo(handle: SimHandle, time: number): MaybePromise<void>;
  simRunToEnd(handle: SimHandle): MaybePromise<void>;
  simReset(handle: SimHandle): MaybePromise<void>;
  simGetTime(handle: SimHandle): MaybePromise<number>;
  simGetStepCount(handle: SimHandle): MaybePromise<number>;
  simGetValue(handle: SimHandle, name: string): MaybePromise<number>;
  simSetValue(handle: SimHandle, name: string, value: number): MaybePromise<void>;
  simGetSeries(handle: SimHandle, name: string): MaybePromise<Float64Array>;
  simGetVarNames(handle: SimHandle): MaybePromise<string[]>;
  simGetLinks(handle: SimHandle): MaybePromise<Link[]>;
}
