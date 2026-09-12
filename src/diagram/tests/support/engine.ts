// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The real WASM engine for the invariant tests that derive their inputs through
 * it (corpus imports, the rename patch path, scene round trips).
 *
 * On a clean checkout without `libsimlin.wasm` those suites skip so the package
 * stays testable standalone. Under CI they never skip: the workflow builds the
 * engine first (`DISABLE_WASM_OPT=1 pnpm build`), so a missing module there is a
 * broken build, and the suite runs and fails on it instead of reporting green.
 */

import { describe } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import { projectFromJson, type Model } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

export type EngineModule = typeof import('@simlin/engine');
export type EngineProject = InstanceType<EngineModule['Project']>;

export const WASM_PATH = path.join(__dirname, '..', '..', '..', 'engine', 'core', 'libsimlin.wasm');

export function engineSuiteMode(wasmBuilt: boolean, ci: string | undefined): 'run' | 'skip' {
  const underCi = ci !== undefined && ci !== '' && ci !== 'false';
  return wasmBuilt || underCi ? 'run' : 'skip';
}

const mode = engineSuiteMode(fs.existsSync(WASM_PATH), process.env.CI);

export const describeWithEngine: typeof describe = mode === 'run' ? describe : describe.skip;

if (mode === 'skip') {
  console.warn(`[invariant tests] skipping engine-backed checks: ${WASM_PATH} not found; run \`pnpm build\`.`);
}

export async function loadEngine(): Promise<EngineModule> {
  // A dynamic import, so a checkout without the built engine package still
  // resolves the modules of the skipped suites.
  const engine = await import('@simlin/engine');
  await engine.resetWasm();
  // Under jsdom a node Buffer is not an instance of this realm's Uint8Array,
  // which the engine checks; copy the bytes into a buffer allocated here.
  const bytes = fs.readFileSync(WASM_PATH);
  const buffer = new ArrayBuffer(bytes.length);
  new Uint8Array(buffer).set(bytes);
  engine.configureWasm({ source: new Uint8Array(buffer) });
  await engine.ready();
  return engine;
}

/** The editor's load path: the engine's JSON serialization through the production deserializer. */
export async function editorModel(project: EngineProject, name = 'main'): Promise<Model> {
  const json = JSON.parse(await project.serializeJson()) as JsonProject;
  return mainModel(projectFromJson(json).models, name);
}

export function mainModel(models: ReadonlyMap<string, Model>, name = 'main'): Model {
  const model = models.get(name) ?? [...models.values()][0];
  if (model === undefined) {
    throw new Error('project has no models');
  }
  return model;
}
