/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// pattern: Imperative Shell (integration tests using real WASM engine)

import * as fs from 'fs';
import * as path from 'path';

import { Project, configureWasm, ready, resetWasm, SIMLIN_VARTYPE_MODULE } from '@simlin/engine';
import type { JsonProjectPatch } from '@simlin/engine';

async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', '..', 'engine', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('upsertModule patch operation', () => {
  // Reset WASM before each test to avoid state pollution between tests.
  // The WASM state can become corrupted after a panic (unreachable) which
  // causes all subsequent calls to fail.
  beforeEach(async () => {
    await loadWasm();
  });

  it('rejects module with empty modelName when allowErrors is false', async () => {
    const project = await Project.open(loadTestXmile());

    const patch: JsonProjectPatch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertModule',
              payload: {
                module: {
                  name: 'failing_module',
                  modelName: '',
                  references: [],
                },
              },
            },
          ],
        },
      ],
    };

    await expect(project.applyPatch(patch)).rejects.toThrow();

    await project.dispose();
  });

  it('creates a new model via addModel project operation', async () => {
    const project = await Project.open(loadTestXmile());

    const initialModelCount = await project.modelCount();

    const addModelPatch: JsonProjectPatch = {
      projectOps: [
        {
          type: 'addModel',
          payload: { name: 'population' },
        },
      ],
    };
    await project.applyPatch(addModelPatch);

    const modelNames = await project.getModelNames();
    expect(modelNames).toContain('population');
    expect(await project.modelCount()).toBe(initialModelCount + 1);

    await project.dispose();
  });

  it('creates a module that references a newly added model', async () => {
    const project = await Project.open(loadTestXmile());

    // Add a new model first
    await project.applyPatch({
      projectOps: [{ type: 'addModel', payload: { name: 'population' } }],
    });

    // Now create a module that references it
    const modulePatch: JsonProjectPatch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertModule',
              payload: {
                module: {
                  name: 'pop_module',
                  modelName: 'population',
                  references: [],
                },
              },
            },
          ],
        },
      ],
    };
    await project.applyPatch(modulePatch, { allowErrors: true });

    const mainModel = await project.mainModel();
    const moduleNames = await mainModel.getVarNames(SIMLIN_VARTYPE_MODULE);
    expect(moduleNames).toContain('pop_module');

    const variable = await mainModel.getVariable('pop_module');
    expect(variable).toBeDefined();
    expect(variable!.type).toBe('module');
    expect((variable as { modelName: string }).modelName).toBe('population');

    await project.dispose();
  });

  it('preserves a module through protobuf roundtrip', async () => {
    const project = await Project.open(loadTestXmile());

    // Add model then create module referencing it
    await project.applyPatch({
      projectOps: [{ type: 'addModel', payload: { name: 'ecosystem' } }],
    });
    await project.applyPatch(
      {
        models: [
          {
            name: 'main',
            ops: [
              {
                type: 'upsertModule',
                payload: {
                  module: {
                    name: 'eco_module',
                    modelName: 'ecosystem',
                    references: [],
                  },
                },
              },
            ],
          },
        ],
      },
      { allowErrors: true },
    );

    // Verify before roundtrip
    const mainModel = await project.mainModel();
    expect(await mainModel.getVarNames(SIMLIN_VARTYPE_MODULE)).toContain('eco_module');

    // Serialize and reopen
    const serialized = await project.serializeProtobuf();
    await project.dispose();

    const restored = await Project.openProtobuf(serialized);
    const restoredModel = await restored.mainModel();
    expect(await restoredModel.getVarNames(SIMLIN_VARTYPE_MODULE)).toContain('eco_module');

    const variable = await restoredModel.getVariable('eco_module');
    expect(variable).toBeDefined();
    expect(variable!.type).toBe('module');
    expect((variable as { modelName: string }).modelName).toBe('ecosystem');

    await restored.dispose();
  });

  it('can apply addModel and upsertModule in a single combined patch', async () => {
    const project = await Project.open(loadTestXmile());

    // Combine addModel + upsertModule in one patch.
    // Project ops run before model ops, so the model exists before
    // the module tries to reference it.
    const combinedPatch: JsonProjectPatch = {
      projectOps: [{ type: 'addModel', payload: { name: 'hares' } }],
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertModule',
              payload: {
                module: {
                  name: 'hare_population',
                  modelName: 'hares',
                  references: [],
                },
              },
            },
          ],
        },
      ],
    };
    await project.applyPatch(combinedPatch, { allowErrors: true });

    const modelNames = await project.getModelNames();
    expect(modelNames).toContain('hares');

    const mainModel = await project.mainModel();
    const variable = await mainModel.getVariable('hare_population');
    expect(variable).toBeDefined();
    expect(variable!.type).toBe('module');
    expect((variable as { modelName: string }).modelName).toBe('hares');

    await project.dispose();
  });
});
