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

import { Project, configureWasm, ready, resetWasm } from '../src';

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
