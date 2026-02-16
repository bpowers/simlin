// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * WorkerBackend: forwards all engine operations to a Web Worker via postMessage.
 *
 * All operations return Promises that resolve when the Worker responds.
 * Operations are strictly serialized via a FIFO queue to match WASM's
 * single-threaded execution model.
 *
 * Most input buffers are structured-cloned to preserve caller data.
 * WASM source buffers are transferred (zero-copy) during init since
 * they are large and used only once. Output buffers from the worker
 * are transferred for zero-copy.
 */

import type { EngineBackend, ProjectHandle, ModelHandle, SimHandle } from './backend';
import type { ErrorDetail, SimlinJsonFormat } from './internal/types';
import type { Loop, Link } from './types';
import type { JsonProjectPatch } from './json-types';
import type { WasmConfig, WasmSourceProvider } from '@simlin/engine/internal/wasm';
import type { WorkerRequest, WorkerResponse } from './worker-protocol';
import { deserializeError } from './worker-protocol';

type PostFn = (msg: WorkerRequest, transfer?: Transferable[]) => void;
type OnMessageFn = (callback: (msg: WorkerResponse) => void) => void;

interface PendingRequest<T = unknown> {
  resolve: (value: T) => void;
  reject: (error: Error) => void;
}

/**
 * A FIFO queue entry: a function that sends the request and returns
 * a promise for the result, with a reject hook for termination.
 */
interface QueueEntry {
  execute: () => void;
  reject: (error: Error) => void;
}

export class WorkerBackend implements EngineBackend {
  private _post: PostFn;
  private _nextRequestId = 1;
  private _pending = new Map<number, PendingRequest>();
  private _initialized = false;
  private _initializing = false;
  private _terminated = false;
  private _storedWasmConfig: WasmConfig | null = null;

  // FIFO queue for strict serialization
  private _queue: QueueEntry[] = [];
  private _processing = false;

  constructor(post: PostFn, onMessage: OnMessageFn) {
    this._post = post;
    onMessage((msg: WorkerResponse) => this.handleResponse(msg));
  }

  private handleResponse(msg: WorkerResponse): void {
    const pending = this._pending.get(msg.requestId);
    if (!pending) {
      return;
    }
    this._pending.delete(msg.requestId);

    if (msg.type === 'success') {
      pending.resolve(msg.result);
    } else {
      pending.reject(deserializeError(msg.error));
    }
  }

  /**
   * Send a request and return a promise for the result.
   * The request is enqueued and executed in FIFO order.
   */
  private sendRequest<T>(
    buildMessage: (requestId: number) => WorkerRequest,
    transfer?: Transferable[],
  ): Promise<T> {
    if (this._terminated) {
      return Promise.reject(new Error('WorkerBackend terminated'));
    }
    return new Promise<T>((resolve, reject) => {
      this._queue.push({
        execute: () => {
          const requestId = this._nextRequestId++;
          this._pending.set(requestId, {
            resolve: (value: unknown) => {
              resolve(value as T);
              this.processNext();
            },
            reject: (error: Error) => {
              reject(error);
              this.processNext();
            },
          });
          const msg = buildMessage(requestId);
          this._post(msg, transfer);
        },
        reject,
      });

      // Start processing if not already
      if (!this._processing) {
        this.processNext();
      }
    });
  }

  private processNext(): void {
    const entry = this._queue.shift();
    if (!entry) {
      this._processing = false;
      return;
    }
    this._processing = true;
    entry.execute();
  }

  // ---- Lifecycle ----

  private async resolveWasmSource(
    source?: WasmSourceProvider,
  ): Promise<{ buffer?: ArrayBuffer; url?: string } | undefined> {
    if (source === undefined) {
      return undefined;
    }
    if (typeof source === 'function') {
      const resolved = await source();
      return this.resolveWasmSource(resolved);
    }
    if (source instanceof Uint8Array) {
      // Extract the underlying ArrayBuffer region. For WASM sources this
      // avoids an extra multi-MB copy -- the buffer will be transferred
      // to the worker via postMessage transfer list instead of being
      // structured-cloned.
      if (source.buffer instanceof ArrayBuffer && source.byteOffset === 0 && source.byteLength === source.buffer.byteLength) {
        return { buffer: source.buffer };
      }
      return { buffer: source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength) as ArrayBuffer };
    }
    if (source instanceof ArrayBuffer) {
      return { buffer: source };
    }
    if (source instanceof URL) {
      return { url: source.toString() };
    }
    // string path or URL
    return { url: source };
  }

  async init(wasmSource?: WasmSourceProvider): Promise<void> {
    if (this._initialized || this._initializing) {
      return;
    }

    this._initializing = true;
    try {
      // Send stored wasm config to worker if any
      if (this._storedWasmConfig) {
        const resolved = await this.resolveWasmSource(this._storedWasmConfig.source);
        if (resolved) {
          const transfer = resolved.buffer ? [resolved.buffer] : undefined;
          await this.sendRequest<void>(
            (requestId) => ({
              type: 'configureWasm',
              requestId,
              config: { source: resolved.buffer, url: resolved.url },
            }),
            transfer,
          );
        }
        this._storedWasmConfig = null;
      }

      const resolved = await this.resolveWasmSource(wasmSource);
      const transfer = resolved?.buffer ? [resolved.buffer] : undefined;
      await this.sendRequest<void>(
        (requestId) => ({
          type: 'init',
          requestId,
          wasmSource: resolved?.buffer,
          wasmUrl: resolved?.url,
        }),
        transfer,
      );
      this._initialized = true;
    } finally {
      this._initializing = false;
    }
  }

  isInitialized(): boolean {
    return this._initialized;
  }

  async reset(): Promise<void> {
    await this.sendRequest<void>((requestId) => ({
      type: 'reset',
      requestId,
    }));
    this._initialized = false;
  }

  /**
   * Terminate this backend, rejecting all pending and queued requests.
   * After termination, all new requests will be immediately rejected.
   * Call this before terminating the underlying Worker to prevent
   * promise leaks.
   */
  terminate(): void {
    this._terminated = true;
    this._initialized = false;
    this._initializing = false;
    this._processing = false;

    const error = new Error('WorkerBackend terminated');

    // Reject all pending requests (sent to worker, awaiting response)
    for (const [, pending] of this._pending) {
      pending.reject(error);
    }
    this._pending.clear();

    // Reject all queued requests (not yet sent to worker)
    for (const entry of this._queue) {
      entry.reject(error);
    }
    this._queue = [];
  }

  configureWasm(config: WasmConfig): void {
    if (this._initialized || this._initializing) {
      throw new Error('WASM already initialized');
    }
    // Store config locally; it will be sent to the worker during init().
    // We can't resolve async sources here (sync method), so we defer.
    this._storedWasmConfig = config;
  }

  // ---- Project open operations ----

  projectOpenXmile(data: Uint8Array): Promise<ProjectHandle> {
    return this.sendRequest<ProjectHandle>((requestId) => ({
      type: 'projectOpenXmile',
      requestId,
      data,
    }));
  }

  projectOpenProtobuf(data: Uint8Array): Promise<ProjectHandle> {
    return this.sendRequest<ProjectHandle>((requestId) => ({
      type: 'projectOpenProtobuf',
      requestId,
      data,
    }));
  }

  projectOpenJson(data: Uint8Array, format: SimlinJsonFormat): Promise<ProjectHandle> {
    return this.sendRequest<ProjectHandle>((requestId) => ({
      type: 'projectOpenJson',
      requestId,
      data,
      format,
    }));
  }

  projectOpenVensim(data: Uint8Array): Promise<ProjectHandle> {
    return this.sendRequest<ProjectHandle>((requestId) => ({
      type: 'projectOpenVensim',
      requestId,
      data,
    }));
  }

  // ---- Project operations ----

  projectDispose(handle: ProjectHandle): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'projectDispose',
      requestId,
      handle,
    }));
  }

  projectGetModelCount(handle: ProjectHandle): Promise<number> {
    return this.sendRequest<number>((requestId) => ({
      type: 'projectGetModelCount',
      requestId,
      handle,
    }));
  }

  projectGetModelNames(handle: ProjectHandle): Promise<string[]> {
    return this.sendRequest<string[]>((requestId) => ({
      type: 'projectGetModelNames',
      requestId,
      handle,
    }));
  }

  projectGetModel(handle: ProjectHandle, name: string | null): Promise<ModelHandle> {
    return this.sendRequest<ModelHandle>((requestId) => ({
      type: 'projectGetModel',
      requestId,
      handle,
      name,
    }));
  }

  projectIsSimulatable(handle: ProjectHandle, modelName: string | null): Promise<boolean> {
    return this.sendRequest<boolean>((requestId) => ({
      type: 'projectIsSimulatable',
      requestId,
      handle,
      modelName,
    }));
  }

  projectSerializeProtobuf(handle: ProjectHandle): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'projectSerializeProtobuf',
      requestId,
      handle,
    }));
  }

  projectSerializeJson(handle: ProjectHandle, format: SimlinJsonFormat): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'projectSerializeJson',
      requestId,
      handle,
      format,
    }));
  }

  projectSerializeXmile(handle: ProjectHandle): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'projectSerializeXmile',
      requestId,
      handle,
    }));
  }

  projectRenderSvg(handle: ProjectHandle, modelName: string): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'projectRenderSvg',
      requestId,
      handle,
      modelName,
    }));
  }

  projectGetLoops(handle: ProjectHandle): Promise<Loop[]> {
    return this.sendRequest<Loop[]>((requestId) => ({
      type: 'projectGetLoops',
      requestId,
      handle,
    }));
  }

  projectGetErrors(handle: ProjectHandle): Promise<ErrorDetail[]> {
    return this.sendRequest<ErrorDetail[]>((requestId) => ({
      type: 'projectGetErrors',
      requestId,
      handle,
    }));
  }

  projectApplyPatch(
    handle: ProjectHandle,
    patch: JsonProjectPatch,
    dryRun: boolean,
    allowErrors: boolean,
  ): Promise<ErrorDetail[]> {
    return this.sendRequest<ErrorDetail[]>((requestId) => ({
      type: 'projectApplyPatch',
      requestId,
      handle,
      patchJson: JSON.stringify(patch),
      dryRun,
      allowErrors,
    }));
  }

  // ---- Model operations ----

  modelGetName(handle: ModelHandle): Promise<string> {
    return this.sendRequest<string>((requestId) => ({
      type: 'modelGetName',
      requestId,
      handle,
    }));
  }

  modelDispose(handle: ModelHandle): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'modelDispose',
      requestId,
      handle,
    }));
  }

  modelGetIncomingLinks(handle: ModelHandle, varName: string): Promise<string[]> {
    return this.sendRequest<string[]>((requestId) => ({
      type: 'modelGetIncomingLinks',
      requestId,
      handle,
      varName,
    }));
  }

  modelGetLinks(handle: ModelHandle): Promise<Link[]> {
    return this.sendRequest<Link[]>((requestId) => ({
      type: 'modelGetLinks',
      requestId,
      handle,
    }));
  }

  modelGetLatexEquation(handle: ModelHandle, ident: string): Promise<string | null> {
    return this.sendRequest<string | null>((requestId) => ({
      type: 'modelGetLatexEquation',
      requestId,
      handle,
      ident,
    }));
  }

  modelGetVarJson(handle: ModelHandle, varName: string): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'modelGetVarJson',
      requestId,
      handle,
      varName,
    }));
  }

  modelGetVarNames(handle: ModelHandle, typeMask: number = 0, filter: string | null = null): Promise<string[]> {
    return this.sendRequest<string[]>((requestId) => ({
      type: 'modelGetVarNames',
      requestId,
      handle,
      typeMask,
      filter,
    }));
  }

  modelGetSimSpecsJson(handle: ModelHandle): Promise<Uint8Array> {
    return this.sendRequest<Uint8Array>((requestId) => ({
      type: 'modelGetSimSpecsJson',
      requestId,
      handle,
    }));
  }

  // ---- Sim operations ----

  simNew(modelHandle: ModelHandle, enableLtm: boolean): Promise<SimHandle> {
    return this.sendRequest<SimHandle>((requestId) => ({
      type: 'simNew',
      requestId,
      modelHandle,
      enableLtm,
    }));
  }

  simDispose(handle: SimHandle): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'simDispose',
      requestId,
      handle,
    }));
  }

  simRunTo(handle: SimHandle, time: number): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'simRunTo',
      requestId,
      handle,
      time,
    }));
  }

  simRunToEnd(handle: SimHandle): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'simRunToEnd',
      requestId,
      handle,
    }));
  }

  simReset(handle: SimHandle): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'simReset',
      requestId,
      handle,
    }));
  }

  simGetTime(handle: SimHandle): Promise<number> {
    return this.sendRequest<number>((requestId) => ({
      type: 'simGetTime',
      requestId,
      handle,
    }));
  }

  simGetStepCount(handle: SimHandle): Promise<number> {
    return this.sendRequest<number>((requestId) => ({
      type: 'simGetStepCount',
      requestId,
      handle,
    }));
  }

  simGetValue(handle: SimHandle, name: string): Promise<number> {
    return this.sendRequest<number>((requestId) => ({
      type: 'simGetValue',
      requestId,
      handle,
      name,
    }));
  }

  simSetValue(handle: SimHandle, name: string, value: number): Promise<void> {
    return this.sendRequest<void>((requestId) => ({
      type: 'simSetValue',
      requestId,
      handle,
      name,
      value,
    }));
  }

  simGetSeries(handle: SimHandle, name: string): Promise<Float64Array> {
    return this.sendRequest<Float64Array>((requestId) => ({
      type: 'simGetSeries',
      requestId,
      handle,
      name,
    }));
  }

  simGetVarNames(handle: SimHandle): Promise<string[]> {
    return this.sendRequest<string[]>((requestId) => ({
      type: 'simGetVarNames',
      requestId,
      handle,
    }));
  }

  simGetLinks(handle: SimHandle): Promise<Link[]> {
    return this.sendRequest<Link[]>((requestId) => ({
      type: 'simGetLinks',
      requestId,
      handle,
    }));
  }
}
