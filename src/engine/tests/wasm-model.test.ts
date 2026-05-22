// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// Public-API parity tests for engine selection. These drive the full facade
// (Project -> Model -> Sim -> Run) rather than DirectBackend directly. The
// bytecode VM is the correctness oracle: a wasm-engine Sim/Run is compared
// against the VM-engine result for the same model within a tight tolerance
// (the wasm blob mirrors the VM opcode-for-opcode). The default (no-engine)
// path must keep behaving exactly as the VM does today.

import * as fs from 'fs';
import * as path from 'path';

import { Project, Model, Sim, Run, configureWasm, ready, resetWasm } from '../src';

// Configure the node DirectBackend with the libsimlin wasm singleton. Mirrors
// api.test.ts; the per-model wasm blob compiled for engine:'wasm' is a separate
// WebAssembly.Instance, independent of this singleton.
async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

// Teacup: a scalar Euler model the wasm backend fully supports.
function loadTeacupXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

// Tolerance for VM-vs-wasm comparison: both executors run the same compiled
// simulation, so any difference is at most floating-point reassociation noise.
const TOL = 1e-9;

function expectSeriesClose(actual: Float64Array, expected: Float64Array): void {
  expect(actual.length).toBe(expected.length);
  for (let i = 0; i < expected.length; i++) {
    expect(Math.abs(actual[i] - expected[i])).toBeLessThanOrEqual(TOL);
  }
}

describe('Model/Sim engine selection (public API)', () => {
  let project: Project;

  beforeAll(async () => {
    await loadWasm();
    project = await Project.open(loadTeacupXmile());
  });

  afterAll(async () => {
    await project.dispose();
  });

  describe('AC1.1: simulate({engine}) drives the selected backend', () => {
    it("simulate({engine:'wasm'}) runToEnd+getSeries match the VM", async () => {
      const model = await project.mainModel();
      const vmSim = await model.simulate({}, { engine: 'vm' });
      const wasmSim = await model.simulate({}, { engine: 'wasm' });

      await vmSim.runToEnd();
      await wasmSim.runToEnd();

      const names = await wasmSim.getVarNames();
      expect(names.length).toBeGreaterThan(0);
      for (const name of names) {
        expectSeriesClose(await wasmSim.getSeries(name), await vmSim.getSeries(name));
      }

      await vmSim.dispose();
      await wasmSim.dispose();
    });

    it('returns a Sim regardless of engine selection', async () => {
      const model = await project.mainModel();
      const defaultSim = await model.simulate();
      const vmSim = await model.simulate({}, { engine: 'vm' });
      const wasmSim = await model.simulate({}, { engine: 'wasm' });

      expect(defaultSim).toBeInstanceOf(Sim);
      expect(vmSim).toBeInstanceOf(Sim);
      expect(wasmSim).toBeInstanceOf(Sim);

      await defaultSim.dispose();
      await vmSim.dispose();
      await wasmSim.dispose();
    });

    it('actually drives the wasm backend, not the VM, for engine:wasm', async () => {
      // The behavioral discriminator at the facade level: a wasm-backed Sim
      // rejects getLinks ("not supported on the wasm engine"), whereas the VM
      // and default sims return a links array. This fails if engine selection
      // is silently dropped and a VM sim is created instead.
      const model = await project.mainModel();
      const wasmSim = await model.simulate({}, { engine: 'wasm' });
      const vmSim = await model.simulate({}, { engine: 'vm' });
      const defaultSim = await model.simulate();

      await expect(wasmSim.getLinks()).rejects.toThrow(/not supported on the wasm engine/i);
      await expect(vmSim.getLinks()).resolves.toEqual(expect.any(Array));
      await expect(defaultSim.getLinks()).resolves.toEqual(expect.any(Array));

      await wasmSim.dispose();
      await vmSim.dispose();
      await defaultSim.dispose();
    });

    it("simulate() and simulate({engine:'vm'}) agree (both VM-backed)", async () => {
      const model = await project.mainModel();
      const defaultSim = await model.simulate();
      const vmSim = await model.simulate({}, { engine: 'vm' });

      await defaultSim.runToEnd();
      await vmSim.runToEnd();

      for (const name of await defaultSim.getVarNames()) {
        expectSeriesClose(await defaultSim.getSeries(name), await vmSim.getSeries(name));
      }

      await defaultSim.dispose();
      await vmSim.dispose();
    });
  });

  describe('AC1.2: run({engine}) series parity', () => {
    it("run({engine:'wasm'}) series equal run({engine:'vm'}) within tolerance", async () => {
      const model = await project.mainModel();
      const vmRun = await model.run({}, { engine: 'vm' });
      const wasmRun = await model.run({}, { engine: 'wasm' });

      expect(wasmRun).toBeInstanceOf(Run);
      expect(wasmRun.varNames).toEqual(vmRun.varNames);
      expect(wasmRun.varNames.length).toBeGreaterThan(0);
      for (const name of wasmRun.varNames) {
        expectSeriesClose(wasmRun.getSeries(name), vmRun.getSeries(name));
      }
      // The time axis is collected even though it is not in varNames.
      expectSeriesClose(wasmRun.time, vmRun.time);
    });

    it("run({engine:'wasm'}) respects a constant override, matching the VM", async () => {
      const model = await project.mainModel();
      const overrides = { room_temperature: 40 };
      const vmRun = await model.run(overrides, { engine: 'vm' });
      const wasmRun = await model.run(overrides, { engine: 'wasm' });

      expect(wasmRun.overrides).toEqual(overrides);
      for (const name of wasmRun.varNames) {
        expectSeriesClose(wasmRun.getSeries(name), vmRun.getSeries(name));
      }
    });
  });

  describe('AC1.3: no-engine calls behave exactly as before (VM)', () => {
    it('run() with no engine equals run({engine:vm})', async () => {
      const model = await project.mainModel();
      const defaultRun = await model.run();
      const vmRun = await model.run({}, { engine: 'vm' });

      expect(defaultRun.varNames).toEqual(vmRun.varNames);
      for (const name of defaultRun.varNames) {
        expectSeriesClose(defaultRun.getSeries(name), vmRun.getSeries(name));
      }
    });

    it('default run() reproduces the documented teacup cooling result', async () => {
      const model = await project.mainModel();
      const run = await model.run();
      // teacup_temperature cools monotonically from its initial value.
      const temp = run.getSeries('teacup_temperature');
      expect(temp.length).toBeGreaterThan(0);
      expect(temp[0]).toBeGreaterThan(temp[temp.length - 1]);
    });

    it('default simulate() with an override tracks the override (VM behavior)', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate({ room_temperature: 30 });
      expect(sim.overrides).toEqual({ room_temperature: 30 });
      expect(await sim.getValue('room_temperature')).toBe(30);
      await sim.dispose();
    });
  });

  describe('AC6.3: run({engine:wasm}) yields a Run with empty links and never calls getLinks', () => {
    it('resolves to a Run whose links array is empty', async () => {
      const model = await project.mainModel();
      const run = await model.run({}, { engine: 'wasm' });

      expect(run).toBeInstanceOf(Run);
      expect(run.links).toEqual([]);
    });

    it('does not throw despite getLinks being unsupported on the wasm engine', async () => {
      const model = await project.mainModel();
      // getRun must not call getLinks() on the wasm sim (which would throw the
      // "not supported on the wasm engine" error); LTM gating skips it instead.
      await expect(model.run({}, { engine: 'wasm' })).resolves.toBeInstanceOf(Run);
    });
  });
});
