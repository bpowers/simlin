// Copyright 2025 The Simlin Authors. All rights reserved.
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
 * import { Project } from '@system-dynamics/engine2';
 *
 * const project = await Project.open(xmileData);
 * const model = project.mainModel;
 *
 * // Run simulation with default parameters
 * const run = model.run();
 * console.log(run.results.get('population'));
 *
 * // Run with overrides
 * const runWithOverrides = model.run({ birth_rate: 0.05 });
 *
 * // Edit the model
 * model.edit((vars, patch) => {
 *   patch.upsertAux({ name: 'new_var', equation: '42' });
 * });
 *
 * // Serialize the project back out
 * const updatedXmile = project.toXmile();
 */

// High-level API classes
export { Project } from './project';
export { Model } from './model';
export { Sim } from './sim';
export { Run } from './run';
export { ModelPatchBuilder } from './patch';

// High-level types
export * from './types';
export * from './json-types';

// Optional WASM configuration for advanced use cases
export {
  configureWasm,
  ensureInitialized as ready,
  isInitialized as isReady,
} from '@system-dynamics/engine2/internal/wasm';
export type { WasmConfig, WasmSource, WasmSourceProvider } from '@system-dynamics/engine2/internal/wasm';
