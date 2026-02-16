// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * High-level TypeScript API for Simlin.
 *
 * This module provides an idiomatic TypeScript interface for working with
 * system dynamics models. It works in both browser and Node.js environments.
 *
 * @example
 * // Load a model and run simulation
 * import { Project } from '@simlin/engine';
 *
 * const project = await Project.open(xmileData);
 * const model = await project.mainModel();
 *
 * // Run simulation with default parameters
 * const run = await model.run();
 * console.log(run.results.get('population'));
 *
 * // Run with overrides
 * const runWithOverrides = await model.run({ birth_rate: 0.05 });
 *
 * // Edit the model
 * await model.edit((vars, patch) => {
 *   patch.upsertAux({ name: 'new_var', equation: '42' });
 * });
 *
 * // Serialize the project back out
 * const updatedXmile = await project.toXmile();
 */

// High-level API classes
export { Project } from './project';
export { Model, SIMLIN_VARTYPE_STOCK, SIMLIN_VARTYPE_FLOW, SIMLIN_VARTYPE_AUX, SIMLIN_VARTYPE_MODULE } from './model';
export { Sim } from './sim';
export { Run } from './run';
export type { RunData } from './run';
export { ModelPatchBuilder } from './patch';

// Error utilities
export { errorCodeDescription, ErrorCode } from './errors';

// High-level types
export * from './types';
export * from './json-types';

// Internal types needed for error handling
export type { ErrorDetail } from './internal/types';
export { SimlinErrorKind, SimlinUnitErrorKind } from './internal/types';

// Backend interface and handle types
export type { EngineBackend, ProjectHandle, ModelHandle, SimHandle } from './backend';

// WASM configuration - routed through backend factory
import { getBackend } from '@simlin/engine/internal/backend-factory';
export type { WasmConfig, WasmSource, WasmSourceProvider } from '@simlin/engine/internal/wasm';

/**
 * Configure the WASM source for the engine.
 * Must be called before ready().
 */
export function configureWasm(config: import('@simlin/engine/internal/wasm').WasmConfig): void {
  getBackend().configureWasm(config);
}

/**
 * Initialize the engine (load WASM module).
 * Must be called (and awaited) before using any engine operations.
 */
export async function ready(wasmSource?: import('@simlin/engine/internal/wasm').WasmSourceProvider): Promise<void> {
  await getBackend().init(wasmSource);
}

/**
 * Check if the engine has been initialized.
 */
export function isReady(): boolean {
  return getBackend().isInitialized();
}

/**
 * Reset the engine state (for testing).
 */
export async function resetWasm(): Promise<void> {
  await getBackend().reset();
}
