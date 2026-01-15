// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * High-level TypeScript API for Simlin.
 *
 * This module provides an idiomatic TypeScript interface for working with
 * system dynamics models. It mirrors the pysimlin Python API for consistency.
 *
 * @example
 * // Load a model and run simulation
 * import { load, init } from '@system-dynamics/engine2/api';
 *
 * await init(); // Initialize WASM
 * const model = await load('model.stmx');
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
 */

import * as fs from 'fs';
import * as path from 'path';

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

// Import Project for local use
import { Project } from './project';
import { Model } from './model';
import { SimlinJsonFormat } from '../types';

/**
 * Detect file format from content or extension.
 * @param data File content
 * @param filePath Optional file path for extension-based detection
 * @returns 'xmile' | 'json' | 'protobuf'
 */
function detectFormat(data: Uint8Array, filePath?: string): 'xmile' | 'json' | 'protobuf' {
  // Check extension first if file path is provided
  if (filePath) {
    const ext = path.extname(filePath).toLowerCase();
    if (ext === '.stmx' || ext === '.xmile' || ext === '.xml') {
      return 'xmile';
    }
    if (ext === '.json') {
      return 'json';
    }
    if (ext === '.pb' || ext === '.protobuf') {
      return 'protobuf';
    }
  }

  // Content-based detection
  // Check for XML declaration or xmile tag
  const text = new TextDecoder().decode(data.slice(0, 100));
  if (text.startsWith('<?xml') || text.includes('<xmile') || text.includes('<XMILE')) {
    return 'xmile';
  }

  // Check for JSON object start
  const trimmed = text.trim();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    return 'json';
  }

  // Default to protobuf
  return 'protobuf';
}

/**
 * Load a model from a file path.
 *
 * Auto-detects format from file extension or content:
 * - .stmx, .xmile, .xml -> XMILE format
 * - .json -> JSON format
 * - .pb, .protobuf -> Protobuf format
 *
 * @param filePath Path to the model file
 * @returns The main Model from the loaded project
 *
 * @example
 * const model = await load('teacup.stmx');
 * const run = model.run();
 */
export async function load(filePath: string): Promise<Model> {
  // Read file - use fs.promises for async
  const data = await fs.promises.readFile(filePath);
  const format = detectFormat(data, filePath);

  let project: Project;
  switch (format) {
    case 'xmile':
      project = Project.fromXmile(data);
      break;
    case 'json':
      project = Project.fromJson(data);
      break;
    case 'protobuf':
      project = Project.fromProtobuf(data);
      break;
  }

  return project.mainModel;
}

/**
 * Load a model from XMILE data.
 *
 * @param data XMILE XML data as Uint8Array or string
 * @returns The main Model from the loaded project
 *
 * @example
 * const xmileData = fs.readFileSync('model.stmx');
 * const model = loadFromXmile(xmileData);
 */
export function loadFromXmile(data: Uint8Array | string): Model {
  const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
  const project = Project.fromXmile(bytes);
  return project.mainModel;
}

/**
 * Load a model from JSON data.
 *
 * @param data JSON string or Uint8Array
 * @param format JSON format (Native or SDAI)
 * @returns The main Model from the loaded project
 *
 * @example
 * const json = fs.readFileSync('model.json', 'utf-8');
 * const model = loadFromJson(json);
 */
export function loadFromJson(data: string | Uint8Array, format: SimlinJsonFormat = SimlinJsonFormat.Native): Model {
  const project = Project.fromJson(data, format);
  return project.mainModel;
}

/**
 * Load a model from protobuf data.
 *
 * @param data Protobuf-encoded project data
 * @returns The main Model from the loaded project
 *
 * @example
 * const pbData = fs.readFileSync('model.pb');
 * const model = loadFromProtobuf(pbData);
 */
export function loadFromProtobuf(data: Uint8Array): Model {
  const project = Project.fromProtobuf(data);
  return project.mainModel;
}
