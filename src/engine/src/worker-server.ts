// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * WorkerServer: runs inside the Web Worker. Receives WorkerRequest messages,
 * dispatches them to a DirectBackend, and sends WorkerResponse messages back.
 *
 * Manages a handle map for WASM objects and enforces the worker state machine.
 */

import { DirectBackend } from './direct-backend';
import type { ProjectHandle, ModelHandle, SimHandle } from './backend';
import { SimlinJsonFormat } from './internal/types';
import { JsonProjectPatch } from './json-types';
import {
  WorkerRequest,
  WorkerResponse,
  WorkerState,
  serializeError,
  isValidRequest,
  WorkerProjectHandle,
  WorkerModelHandle,
  WorkerSimHandle,
} from './worker-protocol';

type PostMessageFn = (msg: WorkerResponse, transfer?: Transferable[]) => void;

export class WorkerServer {
  private backend: DirectBackend;
  private state: WorkerState = WorkerState.UNINITIALIZED;
  private postMessage: PostMessageFn;

  // Map worker handles to backend handles
  private nextHandle = 1;
  private projectHandles = new Map<WorkerProjectHandle, ProjectHandle>();
  private modelHandles = new Map<WorkerModelHandle, ModelHandle>();
  private simHandles = new Map<WorkerSimHandle, SimHandle>();

  // Track which model/sim handles belong to which project
  private projectChildren = new Map<WorkerProjectHandle, Set<WorkerModelHandle | WorkerSimHandle>>();

  constructor(postMessage: PostMessageFn) {
    this.backend = new DirectBackend();
    this.postMessage = postMessage;
  }

  /**
   * Current state of the worker, for testing.
   */
  get currentState(): WorkerState {
    return this.state;
  }

  /**
   * Number of child handles tracked for a project, for testing.
   * Returns undefined if the project handle doesn't exist.
   */
  getProjectChildCount(projectHandle: WorkerProjectHandle): number | undefined {
    return this.projectChildren.get(projectHandle)?.size;
  }

  /**
   * Handle an incoming message. This is the main entry point called from
   * the worker's onmessage handler.
   */
  handleMessage(msg: unknown): void {
    if (!isValidRequest(msg)) {
      // Unknown message format, ignore
      return;
    }

    const request = msg as WorkerRequest;
    const { requestId } = request;

    try {
      // Lifecycle requests are allowed in any state
      if (request.type === 'init' || request.type === 'isInitialized' || request.type === 'reset') {
        this.handleLifecycle(request);
        return;
      }

      if (request.type === 'configureWasm') {
        this.handleConfigureWasm(request);
        return;
      }

      // All other requests require READY state
      if (this.state !== WorkerState.READY) {
        this.sendError(requestId, new Error(`Worker not ready (state: ${this.state}). Call init first.`));
        return;
      }

      this.handleOperation(request);
    } catch (err) {
      this.sendError(requestId, err);
    }
  }

  private allocHandle(): number {
    return this.nextHandle++;
  }

  private getProjectHandle(workerHandle: WorkerProjectHandle): ProjectHandle {
    const handle = this.projectHandles.get(workerHandle);
    if (handle === undefined) {
      throw new Error(`Invalid or disposed project handle: ${workerHandle}`);
    }
    return handle;
  }

  private getModelHandle(workerHandle: WorkerModelHandle): ModelHandle {
    const handle = this.modelHandles.get(workerHandle);
    if (handle === undefined) {
      throw new Error(`Invalid or disposed model handle: ${workerHandle}`);
    }
    return handle;
  }

  private getSimHandle(workerHandle: WorkerSimHandle): SimHandle {
    const handle = this.simHandles.get(workerHandle);
    if (handle === undefined) {
      throw new Error(`Invalid or disposed sim handle: ${workerHandle}`);
    }
    return handle;
  }

  private registerProjectHandle(backendHandle: ProjectHandle): WorkerProjectHandle {
    const workerHandle = this.allocHandle() as WorkerProjectHandle;
    this.projectHandles.set(workerHandle, backendHandle);
    this.projectChildren.set(workerHandle, new Set());
    return workerHandle;
  }

  private registerModelHandle(backendHandle: ModelHandle, parentProject: WorkerProjectHandle): WorkerModelHandle {
    const workerHandle = this.allocHandle() as WorkerModelHandle;
    this.modelHandles.set(workerHandle, backendHandle);
    this.projectChildren.get(parentProject)?.add(workerHandle);
    this.modelToProject.set(workerHandle, parentProject);
    return workerHandle;
  }

  private registerSimHandle(backendHandle: SimHandle, parentProject: WorkerProjectHandle): WorkerSimHandle {
    const workerHandle = this.allocHandle() as WorkerSimHandle;
    this.simHandles.set(workerHandle, backendHandle);
    this.projectChildren.get(parentProject)?.add(workerHandle);
    this.simToProject.set(workerHandle, parentProject);
    return workerHandle;
  }

  // Track which project a model/sim belongs to, for cleanup on individual dispose
  private modelToProject = new Map<WorkerModelHandle, WorkerProjectHandle>();
  private simToProject = new Map<WorkerSimHandle, WorkerProjectHandle>();

  private handleLifecycle(request: WorkerRequest): void {
    const { requestId } = request;

    switch (request.type) {
      case 'init': {
        if (this.state === WorkerState.READY) {
          // Already initialized, idempotent
          this.sendSuccess(requestId, undefined);
          return;
        }
        this.state = WorkerState.INITIALIZING;
        const wasmSource = request.wasmSource
          ? new Uint8Array(request.wasmSource)
          : request.wasmUrl
            ? request.wasmUrl
            : undefined;
        this.backend
          .init(wasmSource)
          .then(() => {
            this.state = WorkerState.READY;
            this.sendSuccess(requestId, undefined);
          })
          .catch((err) => {
            this.state = WorkerState.FAILED;
            this.sendError(requestId, err);
          });
        return;
      }
      case 'isInitialized': {
        this.sendSuccess(requestId, this.state === WorkerState.READY);
        return;
      }
      case 'reset': {
        // backend.reset() invalidates all WASM pointers, so we just need
        // to clear our local handle maps without individually disposing each.
        this.simHandles.clear();
        this.modelHandles.clear();
        this.projectHandles.clear();
        this.projectChildren.clear();
        this.modelToProject.clear();
        this.simToProject.clear();
        this.nextHandle = 1;

        this.backend.reset();
        this.state = WorkerState.UNINITIALIZED;
        this.sendSuccess(requestId, undefined);
        return;
      }
    }
  }

  private handleConfigureWasm(request: Extract<WorkerRequest, { type: 'configureWasm' }>): void {
    const { requestId, config } = request;
    // Reconstruct a WasmConfig with source field from either buffer or URL.
    // The DirectBackend.configureWasm expects { source: WasmSourceProvider }.
    const source = config.source ? new Uint8Array(config.source) : config.url;
    this.backend.configureWasm(source !== undefined ? { source } : {});
    this.sendSuccess(requestId, undefined);
  }

  private handleOperation(request: WorkerRequest): void {
    const { requestId } = request;

    switch (request.type) {
      // Project open operations
      case 'projectOpenXmile': {
        const backendHandle = this.backend.projectOpenXmile(request.data);
        const workerHandle = this.registerProjectHandle(backendHandle);
        this.sendSuccess(requestId, workerHandle);
        return;
      }
      case 'projectOpenProtobuf': {
        const backendHandle = this.backend.projectOpenProtobuf(request.data);
        const workerHandle = this.registerProjectHandle(backendHandle);
        this.sendSuccess(requestId, workerHandle);
        return;
      }
      case 'projectOpenJson': {
        const backendHandle = this.backend.projectOpenJson(request.data, request.format as SimlinJsonFormat);
        const workerHandle = this.registerProjectHandle(backendHandle);
        this.sendSuccess(requestId, workerHandle);
        return;
      }
      case 'projectOpenVensim': {
        const backendHandle = this.backend.projectOpenVensim(request.data);
        const workerHandle = this.registerProjectHandle(backendHandle);
        this.sendSuccess(requestId, workerHandle);
        return;
      }

      // Project operations
      case 'projectDispose': {
        this.disposeProject(request.handle);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'projectGetModelCount': {
        const handle = this.getProjectHandle(request.handle);
        this.sendSuccess(requestId, this.backend.projectGetModelCount(handle));
        return;
      }
      case 'projectGetModelNames': {
        const handle = this.getProjectHandle(request.handle);
        this.sendSuccess(requestId, this.backend.projectGetModelNames(handle));
        return;
      }
      case 'projectGetModel': {
        const handle = this.getProjectHandle(request.handle);
        const backendModelHandle = this.backend.projectGetModel(handle, request.name);
        const workerModelHandle = this.registerModelHandle(backendModelHandle, request.handle);
        this.sendSuccess(requestId, workerModelHandle);
        return;
      }
      case 'projectIsSimulatable': {
        const handle = this.getProjectHandle(request.handle);
        this.sendSuccess(requestId, this.backend.projectIsSimulatable(handle, request.modelName));
        return;
      }
      case 'projectSerializeProtobuf': {
        const handle = this.getProjectHandle(request.handle);
        const result = this.backend.projectSerializeProtobuf(handle);
        // Transfer the buffer for zero-copy
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'projectSerializeJson': {
        const handle = this.getProjectHandle(request.handle);
        const result = this.backend.projectSerializeJson(handle, request.format as SimlinJsonFormat);
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'projectSerializeXmile': {
        const handle = this.getProjectHandle(request.handle);
        const result = this.backend.projectSerializeXmile(handle);
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'projectRenderSvg': {
        const handle = this.getProjectHandle(request.handle);
        const result = this.backend.projectRenderSvg(handle, request.modelName);
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'projectGetErrors': {
        const handle = this.getProjectHandle(request.handle);
        this.sendSuccess(requestId, this.backend.projectGetErrors(handle));
        return;
      }
      case 'projectApplyPatch': {
        const handle = this.getProjectHandle(request.handle);
        const patch = JSON.parse(request.patchJson) as JsonProjectPatch;
        const result = this.backend.projectApplyPatch(handle, patch, request.dryRun, request.allowErrors);
        this.sendSuccess(requestId, result);
        return;
      }

      // Model operations
      case 'modelGetName': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetName(handle));
        return;
      }
      case 'modelDispose': {
        this.disposeModel(request.handle);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'modelGetIncomingLinks': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetIncomingLinks(handle, request.varName));
        return;
      }
      case 'modelGetLinks': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetLinks(handle));
        return;
      }
      case 'modelGetLoops': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetLoops(handle));
        return;
      }
      case 'modelGetLatexEquation': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetLatexEquation(handle, request.ident));
        return;
      }
      case 'modelGetVarJson': {
        const handle = this.getModelHandle(request.handle);
        const result = this.backend.modelGetVarJson(handle, request.varName);
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'modelGetVarNames': {
        const handle = this.getModelHandle(request.handle);
        this.sendSuccess(requestId, this.backend.modelGetVarNames(handle, request.typeMask, request.filter));
        return;
      }
      case 'modelGetSimSpecsJson': {
        const handle = this.getModelHandle(request.handle);
        const result = this.backend.modelGetSimSpecsJson(handle);
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }

      // Sim operations
      case 'simNew': {
        const modelHandle = this.getModelHandle(request.modelHandle);
        const backendSimHandle = this.backend.simNew(modelHandle, request.enableLtm);
        const parentProject = this.modelToProject.get(request.modelHandle);
        if (parentProject === undefined) {
          throw new Error(`Model handle ${request.modelHandle} not associated with a project`);
        }
        const workerSimHandle = this.registerSimHandle(backendSimHandle, parentProject);
        this.sendSuccess(requestId, workerSimHandle);
        return;
      }
      case 'simDispose': {
        this.disposeSim(request.handle);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'simRunTo': {
        const handle = this.getSimHandle(request.handle);
        this.backend.simRunTo(handle, request.time);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'simRunToEnd': {
        const handle = this.getSimHandle(request.handle);
        this.backend.simRunToEnd(handle);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'simReset': {
        const handle = this.getSimHandle(request.handle);
        this.backend.simReset(handle);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'simGetTime': {
        const handle = this.getSimHandle(request.handle);
        this.sendSuccess(requestId, this.backend.simGetTime(handle));
        return;
      }
      case 'simGetStepCount': {
        const handle = this.getSimHandle(request.handle);
        this.sendSuccess(requestId, this.backend.simGetStepCount(handle));
        return;
      }
      case 'simGetValue': {
        const handle = this.getSimHandle(request.handle);
        this.sendSuccess(requestId, this.backend.simGetValue(handle, request.name));
        return;
      }
      case 'simSetValue': {
        const handle = this.getSimHandle(request.handle);
        this.backend.simSetValue(handle, request.name, request.value);
        this.sendSuccess(requestId, undefined);
        return;
      }
      case 'simGetSeries': {
        const handle = this.getSimHandle(request.handle);
        const result = this.backend.simGetSeries(handle, request.name);
        // Transfer Float64Array buffer for zero-copy
        this.sendSuccessWithTransfer(requestId, result, [result.buffer as ArrayBuffer]);
        return;
      }
      case 'simGetVarNames': {
        const handle = this.getSimHandle(request.handle);
        this.sendSuccess(requestId, this.backend.simGetVarNames(handle));
        return;
      }
      case 'simGetLinks': {
        const handle = this.getSimHandle(request.handle);
        this.sendSuccess(requestId, this.backend.simGetLinks(handle));
        return;
      }

      // Lifecycle types are handled before handleOperation is called
      case 'init':
      case 'isInitialized':
      case 'reset':
      case 'configureWasm':
        throw new Error(`Lifecycle request '${request.type}' should not reach handleOperation`);

      default: {
        // TypeScript exhaustiveness check
        const _exhaustive: never = request;
        throw new Error(`Unknown request type: ${(_exhaustive as { type: string }).type}`);
      }
    }
  }

  private disposeProject(workerHandle: WorkerProjectHandle): void {
    // Dispose all child handles first
    const children = this.projectChildren.get(workerHandle);
    if (children) {
      for (const childHandle of children) {
        if (this.modelHandles.has(childHandle as WorkerModelHandle)) {
          this.disposeModel(childHandle as WorkerModelHandle);
        } else if (this.simHandles.has(childHandle as WorkerSimHandle)) {
          this.disposeSim(childHandle as WorkerSimHandle);
        }
      }
    }
    this.projectChildren.delete(workerHandle);

    const backendHandle = this.projectHandles.get(workerHandle);
    if (backendHandle !== undefined) {
      this.backend.projectDispose(backendHandle);
      this.projectHandles.delete(workerHandle);
    }
    // Idempotent: no error if already disposed
  }

  private disposeModel(workerHandle: WorkerModelHandle): void {
    const backendHandle = this.modelHandles.get(workerHandle);
    if (backendHandle !== undefined) {
      this.backend.modelDispose(backendHandle);
      this.modelHandles.delete(workerHandle);
    }
    const parentProject = this.modelToProject.get(workerHandle);
    if (parentProject !== undefined) {
      this.projectChildren.get(parentProject)?.delete(workerHandle);
    }
    this.modelToProject.delete(workerHandle);
  }

  private disposeSim(workerHandle: WorkerSimHandle): void {
    const backendHandle = this.simHandles.get(workerHandle);
    if (backendHandle !== undefined) {
      this.backend.simDispose(backendHandle);
      this.simHandles.delete(workerHandle);
    }
    const parentProject = this.simToProject.get(workerHandle);
    if (parentProject !== undefined) {
      this.projectChildren.get(parentProject)?.delete(workerHandle);
    }
    this.simToProject.delete(workerHandle);
  }

  private sendSuccess(requestId: number, result: unknown): void {
    this.postMessage({ type: 'success', requestId, result });
  }

  private sendSuccessWithTransfer(requestId: number, result: unknown, transfer: Transferable[]): void {
    this.postMessage({ type: 'success', requestId, result }, transfer);
  }

  private sendError(requestId: number, err: unknown): void {
    this.postMessage({ type: 'error', requestId, error: serializeError(err) });
  }
}
