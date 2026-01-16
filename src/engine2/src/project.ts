// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Project class for managing system dynamics projects.
 *
 * A Project contains one or more Models and handles serialization,
 * error checking, and loop analysis at the project level.
 */

import {
  simlin_project_open,
  simlin_project_json_open,
  simlin_project_unref,
  simlin_project_get_model_count,
  simlin_project_get_model_names,
  simlin_project_get_model,
  simlin_project_serialize,
  simlin_project_serialize_json,
  simlin_project_is_simulatable,
  simlin_project_get_errors,
  simlin_project_apply_patch_json,
} from './internal/project';
import { simlin_import_xmile, simlin_export_xmile } from './internal/import-export';
import { simlin_analyze_get_loops, readLoops, simlin_free_loops } from './internal/analysis';
import { SimlinProjectPtr, SimlinJsonFormat, ErrorDetail } from './internal/types';
import { readAllErrorDetails, simlin_error_free } from './internal/error';
import { registerFinalizer, unregisterFinalizer } from './internal/dispose';
import { ensureInitialized, WasmSourceProvider } from './internal/wasm';
import { Loop, LoopPolarity } from './types';
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
  private _ptr: SimlinProjectPtr;
  private _disposed: boolean = false;
  private _models: Map<string, Model> = new Map();
  private _mainModel: Model | null = null;

  private constructor(ptr: SimlinProjectPtr) {
    if (ptr === 0) {
      throw new Error('Cannot create Project from null pointer');
    }
    this._ptr = ptr;
    registerFinalizer(this, ptr, simlin_project_unref);
  }

  /**
   * Create a project from XMILE data.
   * @param data XMILE XML data as Uint8Array
   * @returns New Project instance
   * @throws SimlinError if the XMILE data is invalid
   */
  private static fromXmile(data: Uint8Array): Project {
    const ptr = simlin_import_xmile(data);
    return new Project(ptr);
  }

  /**
   * Create a project from protobuf data.
   * @param data Protobuf-encoded project data
   * @returns New Project instance
   * @throws SimlinError if the protobuf data is invalid
   */
  private static fromProtobuf(data: Uint8Array): Project {
    const ptr = simlin_project_open(data);
    return new Project(ptr);
  }

  /**
   * Create a project from JSON data.
   * @param data JSON string or Uint8Array
   * @param format JSON format (Native or SDAI)
   * @returns New Project instance
   * @throws SimlinError if the JSON data is invalid
   */
  private static fromJson(data: string | Uint8Array, format: SimlinJsonFormat = SimlinJsonFormat.Native): Project {
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    const ptr = simlin_project_json_open(bytes, format);
    return new Project(ptr);
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
    await ensureInitialized(options.wasm);
    const data = typeof xmile === 'string' ? new TextEncoder().encode(xmile) : xmile;
    return Project.fromXmile(data);
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
    await ensureInitialized(options.wasm);
    return Project.fromProtobuf(data);
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
    await ensureInitialized(options.wasm);
    const format = options.format ?? SimlinJsonFormat.Native;
    return Project.fromJson(data, format);
  }

  /**
   * Get the internal WASM pointer. For internal use only.
   */
  get ptr(): SimlinProjectPtr {
    this.checkDisposed();
    return this._ptr;
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
  get modelCount(): number {
    this.checkDisposed();
    return simlin_project_get_model_count(this._ptr);
  }

  /**
   * Get names of all models in this project.
   * @returns Array of model names
   */
  getModelNames(): string[] {
    this.checkDisposed();
    return simlin_project_get_model_names(this._ptr);
  }

  /**
   * Get the main (default) model from this project.
   * The main model is typically the first model or the one that is simulatable.
   * @returns The main Model instance
   */
  get mainModel(): Model {
    this.checkDisposed();
    if (this._mainModel === null) {
      this._mainModel = this.getModel(null);
    }
    return this._mainModel;
  }

  /**
   * Get a model by name.
   * @param name Model name, or null for the default/main model
   * @returns The Model instance
   * @throws SimlinError if model not found
   */
  getModel(name: string | null): Model {
    this.checkDisposed();

    // Check cache first
    const cacheKey = name ?? '';
    const cached = this._models.get(cacheKey);
    if (cached) {
      return cached;
    }

    const modelPtr = simlin_project_get_model(this._ptr, name);
    const model = new Model(modelPtr, this, name);
    this._models.set(cacheKey, model);
    return model;
  }

  /**
   * Get all models in this project.
   * @returns Array of Model instances
   */
  get models(): readonly Model[] {
    this.checkDisposed();
    return this.getModelNames().map((name) => this.getModel(name));
  }

  /**
   * Check if this project (or a specific model) is simulatable.
   * @param modelName Optional model name to check, or null for main model
   * @returns true if simulatable
   */
  isSimulatable(modelName: string | null = null): boolean {
    this.checkDisposed();
    return simlin_project_is_simulatable(this._ptr, modelName);
  }

  /**
   * Serialize this project to protobuf format.
   * @returns Protobuf-encoded data
   */
  serializeProtobuf(): Uint8Array {
    this.checkDisposed();
    return simlin_project_serialize(this._ptr);
  }

  /**
   * Serialize this project to JSON format.
   * @param format JSON format (Native or SDAI)
   * @returns JSON string
   */
  serializeJson(format: SimlinJsonFormat = SimlinJsonFormat.Native): string {
    this.checkDisposed();
    const bytes = simlin_project_serialize_json(this._ptr, format);
    return new TextDecoder().decode(bytes);
  }

  /**
   * Export this project to XMILE format.
   * @returns XMILE XML data
   */
  toXmile(): Uint8Array {
    this.checkDisposed();
    return simlin_export_xmile(this._ptr);
  }

  /**
   * Export this project to XMILE format as a string.
   * @returns XMILE XML string
   */
  toXmileString(): string {
    return new TextDecoder().decode(this.toXmile());
  }

  /**
   * Get all feedback loops in this project.
   * @returns Array of Loop objects
   */
  getLoops(): Loop[] {
    this.checkDisposed();
    const loopsPtr = simlin_analyze_get_loops(this._ptr);
    if (loopsPtr === 0) {
      return [];
    }
    const rawLoops = readLoops(loopsPtr);
    simlin_free_loops(loopsPtr);
    return rawLoops.map((loop) => ({
      id: loop.id,
      variables: loop.variables,
      polarity: loop.polarity as unknown as LoopPolarity,
    }));
  }

  /**
   * Get all errors in this project.
   * @returns Array of ErrorDetail objects
   */
  getErrors(): ErrorDetail[] {
    this.checkDisposed();
    const errPtr = simlin_project_get_errors(this._ptr);
    if (errPtr === 0) {
      return [];
    }
    const details = readAllErrorDetails(errPtr);
    simlin_error_free(errPtr);
    return details;
  }

  /**
   * Apply a JSON patch to this project.
   * @param patch The patch to apply
   * @param options Patch options
   * @returns Array of collected errors (if allowErrors is true)
   * @throws SimlinError if patch fails and allowErrors is false
   */
  applyPatch(patch: JsonProjectPatch, options: { dryRun?: boolean; allowErrors?: boolean } = {}): ErrorDetail[] {
    this.checkDisposed();
    const { dryRun = false, allowErrors = false } = options;

    const patchJson = JSON.stringify(patch);
    const patchBytes = new TextEncoder().encode(patchJson);

    const collectedPtr = simlin_project_apply_patch_json(
      this._ptr,
      patchBytes,
      SimlinJsonFormat.Native,
      dryRun,
      allowErrors,
    );

    // Invalidate all model caches since the project state changed
    if (!dryRun) {
      for (const model of this._models.values()) {
        model.invalidateCaches();
      }
    }

    if (collectedPtr === 0) {
      return [];
    }

    const details = readAllErrorDetails(collectedPtr);
    simlin_error_free(collectedPtr);
    return details;
  }

  /**
   * Dispose this project and free WASM resources.
   * After disposal, the project cannot be used.
   */
  dispose(): void {
    if (this._disposed) {
      return;
    }

    unregisterFinalizer(this);

    // Dispose all cached models first (includes main model if accessed)
    for (const model of this._models.values()) {
      model.dispose();
    }
    this._models.clear();
    this._mainModel = null;

    // Free the WASM pointer
    simlin_project_unref(this._ptr);
    this._ptr = 0;
    this._disposed = true;
  }

  /**
   * Symbol.dispose support for using statement.
   */
  [Symbol.dispose](): void {
    this.dispose();
  }
}
