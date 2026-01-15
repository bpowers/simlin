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
 * import { Project, init } from '@system-dynamics/engine2/api';
 *
 * await init(wasmBuffer); // Initialize WASM
 * const project = Project.fromXmile(xmileData);
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

// Re-export classes
export { Project } from './project';
export { Model } from './model';
export { Sim } from './sim';
export { Run } from './run';
export { ModelPatchBuilder } from './patch';

// Re-export types
export * from './types';
export * from './json-types';

// Re-export low-level WASM initialization
export { init, reset, isInitialized } from '../wasm';
