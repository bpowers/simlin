// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Tests for resource cleanup behavior.
 *
 * These tests verify that WASM resources are properly cleaned up
 * when dispose is called on Project, Model, and Sim objects.
 */

import * as fs from 'fs';
import * as path from 'path';

import { Project, Model, Sim, configureWasm, ready, resetWasm } from '../src';
import type { EngineBackend, ProjectHandle, ModelHandle, SimHandle } from '../src/backend';

// Helper to load the WASM module
async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

// Load the teacup test model in XMILE format
function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

// Minimal mock backend that rejects on dispose operations
function createRejectingBackend(): EngineBackend {
  return {
    init: () => Promise.resolve(),
    isInitialized: () => true,
    reset: () => Promise.resolve(),
    configureWasm: () => {},
    projectOpenXmile: () => Promise.reject(new Error('not implemented')),
    projectOpenProtobuf: () => Promise.reject(new Error('not implemented')),
    projectOpenJson: () => Promise.reject(new Error('not implemented')),
    projectOpenVensim: () => Promise.reject(new Error('not implemented')),
    projectDispose: () => Promise.reject(new Error('project dispose failed')),
    projectGetModelCount: () => Promise.reject(new Error('not implemented')),
    projectGetModelNames: () => Promise.reject(new Error('not implemented')),
    projectGetModel: () => Promise.reject(new Error('not implemented')),
    projectIsSimulatable: () => Promise.reject(new Error('not implemented')),
    projectSerializeProtobuf: () => Promise.reject(new Error('not implemented')),
    projectSerializeJson: () => Promise.reject(new Error('not implemented')),
    projectSerializeXmile: () => Promise.reject(new Error('not implemented')),
    projectRenderSvg: () => Promise.reject(new Error('not implemented')),
    projectGetLoops: () => Promise.reject(new Error('not implemented')),
    projectGetErrors: () => Promise.reject(new Error('not implemented')),
    projectApplyPatch: () => Promise.reject(new Error('not implemented')),
    modelDispose: () => Promise.reject(new Error('model dispose failed')),
    modelGetIncomingLinks: () => Promise.reject(new Error('not implemented')),
    modelGetLinks: () => Promise.reject(new Error('not implemented')),
    modelGetLatexEquation: () => Promise.reject(new Error('not implemented')),
    simNew: () => Promise.resolve(99 as SimHandle),
    simDispose: () => Promise.reject(new Error('sim dispose failed')),
    simRunTo: () => Promise.reject(new Error('not implemented')),
    simRunToEnd: () => Promise.reject(new Error('not implemented')),
    simReset: () => Promise.reject(new Error('not implemented')),
    simGetTime: () => Promise.reject(new Error('not implemented')),
    simGetStepCount: () => Promise.reject(new Error('not implemented')),
    simGetValue: () => Promise.reject(new Error('not implemented')),
    simSetValue: () => Promise.reject(new Error('not implemented')),
    simGetSeries: () => Promise.reject(new Error('not implemented')),
    simGetVarNames: () => Promise.reject(new Error('not implemented')),
    simGetLinks: () => Promise.reject(new Error('not implemented')),
  };
}

describe('dispose warns on async backend errors', () => {
  let warnSpy: jest.SpyInstance;

  beforeEach(() => {
    warnSpy = jest.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    warnSpy.mockRestore();
  });

  it('Project Symbol.dispose warns when backend rejects', async () => {
    const backend = createRejectingBackend();
    const project = new Project(1 as ProjectHandle, backend);

    project[Symbol.dispose]();

    // Wait for the promise rejection to be handled
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('Project'),
      expect.any(Error),
    );
  });

  it('Model Symbol.dispose warns when backend rejects', async () => {
    const backend = createRejectingBackend();
    const project = new Project(1 as ProjectHandle, backend);
    const model = new Model(2 as ModelHandle, project, 'main');

    model[Symbol.dispose]();

    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('Model'),
      expect.any(Error),
    );
  });

  it('Sim Symbol.dispose warns when backend rejects', async () => {
    const backend = createRejectingBackend();
    const project = new Project(1 as ProjectHandle, backend);
    const model = new Model(2 as ModelHandle, project, 'main');
    const sim = await Sim.create(model);

    sim[Symbol.dispose]();

    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('Sim'),
      expect.any(Error),
    );
  });
});

describe('cleanup on dispose', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  it('project dispose is idempotent', async () => {
    const project = await Project.open(loadTestXmile());

    // First dispose should succeed
    await project.dispose();

    // Second dispose should also succeed (idempotent)
    await project.dispose();
  });

  it('model dispose is idempotent', async () => {
    const project = await Project.open(loadTestXmile());
    const model = await project.mainModel();

    // First dispose should succeed
    await model.dispose();

    // Second dispose should also succeed (idempotent)
    await model.dispose();

    await project.dispose();
  });

  it('sim dispose is idempotent', async () => {
    const project = await Project.open(loadTestXmile());
    const model = await project.mainModel();
    const sim = await model.simulate();

    // First dispose should succeed
    await sim.dispose();

    // Second dispose should also succeed (idempotent)
    await sim.dispose();

    await project.dispose();
  });

  it('project dispose cascades to models', async () => {
    const project = await Project.open(loadTestXmile());
    const model = await project.mainModel();

    // Verify model works before dispose
    expect((await model.stocks()).length).toBeGreaterThan(0);

    // Dispose project
    await project.dispose();

    // Model should be disposed (throws on use)
    await expect(model.stocks()).rejects.toThrow();
  });

  it('operations on disposed project throw', async () => {
    const project = await Project.open(loadTestXmile());
    await project.dispose();

    await expect(project.getModelNames()).rejects.toThrow();
    await expect(project.mainModel()).rejects.toThrow();
    await expect(project.serializeJson()).rejects.toThrow();
    await expect(project.serializeProtobuf()).rejects.toThrow();
  });

  it('operations on disposed model throw', async () => {
    const project = await Project.open(loadTestXmile());
    const model = await project.mainModel();
    await model.dispose();

    await expect(model.stocks()).rejects.toThrow();
    await expect(model.flows()).rejects.toThrow();
    await expect(model.variables()).rejects.toThrow();
    await expect(model.getLinks()).rejects.toThrow();

    await project.dispose();
  });

  it('operations on disposed sim throw', async () => {
    const project = await Project.open(loadTestXmile());
    const model = await project.mainModel();
    const sim = await model.simulate();
    await sim.dispose();

    await expect(sim.time()).rejects.toThrow();
    await expect(sim.getValue('teacup temperature')).rejects.toThrow();
    await expect(sim.runToEnd()).rejects.toThrow();

    await project.dispose();
  });
});
