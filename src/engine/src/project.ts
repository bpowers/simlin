// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Project class for managing system dynamics projects.
 *
 * A Project contains one or more Models and handles serialization,
 * error checking, and loop analysis at the project level.
 */

import { EngineBackend, ProjectHandle } from './backend';
import { getBackend } from '@simlin/engine/internal/backend-factory';
import { SimlinJsonFormat, ErrorDetail } from './internal/types';
import { WasmSourceProvider } from '@simlin/engine/internal/wasm';
import { Loop } from './types';
import { Model } from './model';
import { JsonProjectPatch } from './json-types';

type ProjectOpenOptions = {
  wasm?: WasmSourceProvider;
};

type ProjectOpenJsonOptions = ProjectOpenOptions & {
  format?: SimlinJsonFormat;
};

/**
 * A system dynamics project containing models.
 *
 * Projects manage the lifecycle of WASM resources and provide
 * access to models, serialization, and project-level analysis.
 */
export class Project {
  private _handle: ProjectHandle;
  private _backend: EngineBackend;
  private _disposed: boolean = false;
  private _models: Map<string, Model> = new Map();
  private _mainModel: Model | null = null;

  /** @internal */
  constructor(handle: ProjectHandle, backend: EngineBackend) {
    this._handle = handle;
    this._backend = backend;
  }

  /**
   * Create a project from XMILE data (string or bytes).
   * Automatically initializes WASM if needed.
   * @param xmile XMILE XML data as string or Uint8Array
   * @param options Optional WASM configuration
   * @returns Promise resolving to new Project instance
   * @throws SimlinError if the XMILE data is invalid
   */
  static async open(xmile: string | Uint8Array, options: ProjectOpenOptions = {}): Promise<Project> {
    const backend = getBackend();
    await backend.init(options.wasm);
    const data = typeof xmile === 'string' ? new TextEncoder().encode(xmile) : xmile;
    const handle = await backend.projectOpenXmile(data);
    return new Project(handle, backend);
  }

  /**
   * Create a project from protobuf data.
   * Automatically initializes WASM if needed.
   * @param data Protobuf-encoded project data
   * @param options Optional WASM configuration
   * @returns Promise resolving to new Project instance
   * @throws SimlinError if the protobuf data is invalid
   */
  static async openProtobuf(data: Uint8Array, options: ProjectOpenOptions = {}): Promise<Project> {
    const backend = getBackend();
    await backend.init(options.wasm);
    const handle = await backend.projectOpenProtobuf(data);
    return new Project(handle, backend);
  }

  /**
   * Create a project from JSON data (string or bytes).
   * Automatically initializes WASM if needed.
   * @param data JSON string or Uint8Array
   * @param options Optional format and WASM configuration
   * @returns Promise resolving to new Project instance
   * @throws SimlinError if the JSON data is invalid
   */
  static async openJson(data: string | Uint8Array, options: ProjectOpenJsonOptions = {}): Promise<Project> {
    const backend = getBackend();
    await backend.init(options.wasm);
    const format = options.format ?? SimlinJsonFormat.Native;
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    const handle = await backend.projectOpenJson(bytes, format);
    return new Project(handle, backend);
  }

  /**
   * Create a project from Vensim MDL data (string or bytes).
   * Automatically initializes WASM if needed.
   *
   * @param data MDL file data as string or Uint8Array
   * @param options Optional WASM configuration
   * @returns Promise resolving to new Project instance
   * @throws SimlinError if the MDL data is invalid
   */
  static async openVensim(data: string | Uint8Array, options: ProjectOpenOptions = {}): Promise<Project> {
    const backend = getBackend();
    await backend.init(options.wasm);
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    const handle = await backend.projectOpenVensim(bytes);
    return new Project(handle, backend);
  }

  /** @internal */
  get handle(): ProjectHandle {
    this.checkDisposed();
    return this._handle;
  }

  /** @internal */
  get backend(): EngineBackend {
    return this._backend;
  }

  /**
   * Check if the project has been disposed.
   */
  get isDisposed(): boolean {
    return this._disposed;
  }

  private checkDisposed(): void {
    if (this._disposed) {
      throw new Error('Project has been disposed');
    }
  }

  /**
   * Get the number of models in this project.
   */
  async modelCount(): Promise<number> {
    this.checkDisposed();
    return await this._backend.projectGetModelCount(this._handle);
  }

  /**
   * Get names of all models in this project.
   * @returns Array of model names
   */
  async getModelNames(): Promise<string[]> {
    this.checkDisposed();
    return await this._backend.projectGetModelNames(this._handle);
  }

  /**
   * Get the main (default) model from this project.
   * The main model is typically the first model or the one that is simulatable.
   * @returns The main Model instance
   */
  async mainModel(): Promise<Model> {
    this.checkDisposed();
    if (this._mainModel === null) {
      this._mainModel = await this.getModel(null);
    }
    return this._mainModel;
  }

  /**
   * Get a model by name.
   * @param name Model name, or null for the default/main model
   * @returns The Model instance
   * @throws SimlinError if model not found
   */
  async getModel(name: string | null): Promise<Model> {
    this.checkDisposed();

    // Check cache first
    const cacheKey = name ?? '';
    const cached = this._models.get(cacheKey);
    if (cached) {
      return cached;
    }

    const modelHandle = await this._backend.projectGetModel(this._handle, name);
    // The Rust FFI resolves canonical name variants (e.g. "my_model" -> "My Model").
    // Query the resolved display name so that edit() patches and check() error
    // filtering use the correct name, not the caller-supplied alias.
    const resolvedName = await this._backend.modelGetName(modelHandle);
    const model = new Model(modelHandle, this, resolvedName);
    this._models.set(cacheKey, model);
    return model;
  }

  /**
   * Get all models in this project.
   * @returns Array of Model instances
   */
  async models(): Promise<readonly Model[]> {
    this.checkDisposed();
    const names = await this.getModelNames();
    const models: Model[] = [];
    for (const name of names) {
      models.push(await this.getModel(name));
    }
    return models;
  }

  /**
   * Check if this project (or a specific model) is simulatable.
   * @param modelName Optional model name to check, or null for main model
   * @returns true if simulatable
   */
  async isSimulatable(modelName: string | null = null): Promise<boolean> {
    this.checkDisposed();
    return await this._backend.projectIsSimulatable(this._handle, modelName);
  }

  /**
   * Serialize this project to protobuf format.
   * @returns Protobuf-encoded data
   */
  async serializeProtobuf(): Promise<Uint8Array> {
    this.checkDisposed();
    return await this._backend.projectSerializeProtobuf(this._handle);
  }

  /**
   * Serialize this project to JSON format.
   * @param format JSON format (Native or SDAI)
   * @returns JSON string
   */
  async serializeJson(format: SimlinJsonFormat = SimlinJsonFormat.Native): Promise<string> {
    this.checkDisposed();
    const bytes = await this._backend.projectSerializeJson(this._handle, format);
    return new TextDecoder().decode(bytes);
  }

  /**
   * Export this project to XMILE format.
   * @returns XMILE XML data
   */
  async toXmile(): Promise<Uint8Array> {
    this.checkDisposed();
    return await this._backend.projectSerializeXmile(this._handle);
  }

  /**
   * Export this project to XMILE format as a string.
   * @returns XMILE XML string
   */
  async toXmileString(): Promise<string> {
    return new TextDecoder().decode(await this.toXmile());
  }

  /**
   * Render a model's stock-and-flow diagram as SVG.
   * @param modelName Model name
   * @returns SVG data as UTF-8 bytes
   */
  async renderSvg(modelName: string): Promise<Uint8Array> {
    this.checkDisposed();
    return await this._backend.projectRenderSvg(this._handle, modelName);
  }

  /**
   * Render a model's stock-and-flow diagram as an SVG string.
   * @param modelName Model name
   * @returns SVG string
   */
  async renderSvgString(modelName: string): Promise<string> {
    return new TextDecoder().decode(await this.renderSvg(modelName));
  }

  /**
   * Get all feedback loops in this project.
   * @returns Array of Loop objects
   */
  async getLoops(): Promise<Loop[]> {
    this.checkDisposed();
    return await this._backend.projectGetLoops(this._handle);
  }

  /**
   * Get all errors in this project.
   * @returns Array of ErrorDetail objects
   */
  async getErrors(): Promise<ErrorDetail[]> {
    this.checkDisposed();
    return await this._backend.projectGetErrors(this._handle);
  }

  /**
   * Apply a JSON patch to this project.
   * @param patch The patch to apply
   * @param options Patch options
   * @returns Array of collected errors (if allowErrors is true)
   * @throws SimlinError if patch fails and allowErrors is false
   */
  async applyPatch(
    patch: JsonProjectPatch,
    options: { dryRun?: boolean; allowErrors?: boolean } = {},
  ): Promise<ErrorDetail[]> {
    this.checkDisposed();
    const { dryRun = false, allowErrors = false } = options;

    const errors = await this._backend.projectApplyPatch(this._handle, patch, dryRun, allowErrors);

    // Invalidate all model caches since the project state changed
    if (!dryRun) {
      for (const model of this._models.values()) {
        model.invalidateCaches();
      }
    }

    return errors;
  }

  /**
   * Dispose this project and free WASM resources.
   * After disposal, the project cannot be used.
   */
  async dispose(): Promise<void> {
    if (this._disposed) {
      return;
    }

    // Dispose all cached models first (includes main model if accessed)
    for (const model of this._models.values()) {
      await model.dispose();
    }
    this._models.clear();
    this._mainModel = null;

    await this._backend.projectDispose(this._handle);
    this._disposed = true;
  }

  /**
   * Symbol.dispose support for using statement.
   * Fires dispose but cannot await; for WorkerBackend the message
   * is enqueued and will complete asynchronously.
   */
  [Symbol.dispose](): void {
    if (this._disposed) {
      return;
    }

    for (const model of this._models.values()) {
      model[Symbol.dispose]();
    }
    this._models.clear();
    this._mainModel = null;

    const result = this._backend.projectDispose(this._handle);
    if (result instanceof Promise) {
      result.catch((e) => console.warn('Project dispose failed:', e));
    }
    this._disposed = true;
  }
}
