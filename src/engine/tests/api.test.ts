// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Tests for the high-level TypeScript API for Simlin.
 *
 * These tests define the expected behavior of the API, following TDD principles.
 * The API should be idiomatic TypeScript and mirror the pysimlin API for consistency.
 */

import * as fs from 'fs';
import * as path from 'path';

import {
  Project,
  Model,
  Sim,
  Run,
  LinkPolarity,
  ModelPatchBuilder,
  configureWasm,
  ready,
  resetWasm,
  SIMLIN_VARTYPE_STOCK,
  SIMLIN_VARTYPE_FLOW,
  SIMLIN_VARTYPE_AUX,
  SIMLIN_VARTYPE_MODULE,
} from '../src';
import { JsonStock, JsonFlow, JsonAuxiliary } from '../src/json-types';

// Helper to load the WASM module
async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

// Load the teacup test model in XMILE format from pysimlin fixtures
function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

// Load the teacup test model in Vensim MDL format
function loadTestMdl(): Uint8Array {
  const mdlPath = path.join(__dirname, '..', '..', '..', 'test', 'test-models', 'samples', 'teacup', 'teacup.mdl');
  if (!fs.existsSync(mdlPath)) {
    throw new Error('Required test MDL model not found: ' + mdlPath);
  }
  return fs.readFileSync(mdlPath);
}

async function openTestProject(): Promise<Project> {
  return Project.open(loadTestXmile());
}

describe('High-Level API', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  describe('Project class', () => {
    it('should load from XMILE data', async () => {
      const project = await openTestProject();
      expect(project).toBeInstanceOf(Project);
      await project.dispose();
    });

    it('should get model names', async () => {
      const project = await openTestProject();

      const modelNames = await project.getModelNames();
      expect(Array.isArray(modelNames)).toBe(true);
      expect(modelNames.length).toBeGreaterThan(0);

      await project.dispose();
    });

    it('should get the main model', async () => {
      const project = await openTestProject();

      const model = await project.mainModel();
      expect(model).toBeInstanceOf(Model);

      await project.dispose();
    });

    it('should get a model by name', async () => {
      const project = await openTestProject();

      const modelNames = await project.getModelNames();
      const model = await project.getModel(modelNames[0]);
      expect(model).toBeInstanceOf(Model);

      await project.dispose();
    });

    it('should get the default model with null name', async () => {
      const project = await openTestProject();

      const model = await project.getModel(null);
      expect(model).toBeInstanceOf(Model);

      await project.dispose();
    });

    it('should throw for nonexistent model name', async () => {
      const project = await openTestProject();

      await expect(project.getModel('nonexistent_model_xyz')).rejects.toThrow(/not found/);

      await project.dispose();
    });

    it('should check if project is simulatable', async () => {
      const project = await openTestProject();

      const isSimulatable = await project.isSimulatable();
      expect(isSimulatable).toBe(true);

      await project.dispose();
    });

    it('should serialize to protobuf and back', async () => {
      const project1 = await openTestProject();

      const protobuf = await project1.serializeProtobuf();
      expect(protobuf).toBeInstanceOf(Uint8Array);
      expect(protobuf.length).toBeGreaterThan(0);

      const project2 = await Project.openProtobuf(protobuf);
      expect(await project2.getModelNames()).toEqual(await project1.getModelNames());

      await project1.dispose();
      await project2.dispose();
    });

    it('should serialize to JSON', async () => {
      const project = await openTestProject();

      const json = await project.serializeJson();
      expect(typeof json).toBe('string');

      const parsed = JSON.parse(json);
      expect(parsed).toHaveProperty('models');
      expect(parsed).toHaveProperty('simSpecs');

      await project.dispose();
    });

    it('should render SVG', async () => {
      const project = await openTestProject();

      const svg = await project.renderSvg('main');
      expect(svg).toBeInstanceOf(Uint8Array);
      expect(svg.length).toBeGreaterThan(0);

      const svgString = new TextDecoder().decode(svg);
      expect(svgString).toContain('<svg');

      await project.dispose();
    });

    it('should render SVG string', async () => {
      const project = await openTestProject();

      const svgString = await project.renderSvgString('main');
      expect(typeof svgString).toBe('string');
      expect(svgString).toContain('<svg');

      await project.dispose();
    });

    it('should render PNG at intrinsic size', async () => {
      const project = await openTestProject();

      const png = await project.renderPng('main');
      expect(png).toBeInstanceOf(Uint8Array);
      expect(png.length).toBeGreaterThan(8);

      // Verify PNG signature
      expect(png[0]).toBe(137);
      expect(png[1]).toBe(80); // P
      expect(png[2]).toBe(78); // N
      expect(png[3]).toBe(71); // G

      await project.dispose();
    });

    it('should render PNG with explicit width', async () => {
      const project = await openTestProject();

      const png = await project.renderPng('main', 800);
      expect(png).toBeInstanceOf(Uint8Array);
      expect(png.length).toBeGreaterThan(8);
      expect(png[0]).toBe(137);

      await project.dispose();
    });

    it('should throw for PNG render of nonexistent model', async () => {
      const project = await openTestProject();

      await expect(project.renderPng('nonexistent_model_xyz')).rejects.toThrow();

      await project.dispose();
    });

    it('should get loops via model', async () => {
      const project = await openTestProject();

      const model = await project.mainModel();
      const loops = await model.loops();
      expect(Array.isArray(loops)).toBe(true);

      await project.dispose();
    });

    it('should get errors', async () => {
      const project = await openTestProject();

      // The teacup model should have no errors
      const errors = await project.getErrors();
      expect(Array.isArray(errors)).toBe(true);
      expect(errors.length).toBe(0);

      await project.dispose();
    });
  });

  describe('Model class', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(async () => {
      await project.dispose();
    });

    it('should have a reference to its project', async () => {
      const model = await project.mainModel();
      expect(model.project).toBe(project);
    });

    it('should get stock variable names', async () => {
      const model = await project.mainModel();
      const stockNames = await model.getVarNames(SIMLIN_VARTYPE_STOCK);

      expect(Array.isArray(stockNames)).toBe(true);
      // teacup model has at least one stock (teacup temperature)
      expect(stockNames.length).toBeGreaterThan(0);

      const stock = await model.getVariable(stockNames[0]);
      expect(stock).toBeDefined();
      expect(stock!.type).toBe('stock');
      expect(typeof stock!.name).toBe('string');
      if (stock!.type === 'stock') {
        expect(typeof stock!.initialEquation).toBe('string');
        expect(Array.isArray(stock!.inflows)).toBe(true);
        expect(Array.isArray(stock!.outflows)).toBe(true);
      }
    });

    it('should get flow variable names', async () => {
      const model = await project.mainModel();
      const flowNames = await model.getVarNames(SIMLIN_VARTYPE_FLOW);

      expect(Array.isArray(flowNames)).toBe(true);
      // teacup model has flows

      for (const name of flowNames) {
        const flow = await model.getVariable(name);
        expect(flow).toBeDefined();
        expect(flow!.type).toBe('flow');
        expect(typeof flow!.name).toBe('string');
        if (flow!.type === 'flow') {
          expect(typeof flow!.equation).toBe('string');
        }
      }
    });

    it('should get auxiliary variable names', async () => {
      const model = await project.mainModel();
      const auxNames = await model.getVarNames(SIMLIN_VARTYPE_AUX);

      expect(Array.isArray(auxNames)).toBe(true);

      for (const name of auxNames) {
        const aux = await model.getVariable(name);
        expect(aux).toBeDefined();
        expect(aux!.type).toBe('aux');
        expect(typeof aux!.name).toBe('string');
        if (aux!.type === 'aux') {
          expect(typeof aux!.equation).toBe('string');
        }
      }
    });

    it('should get all variable names', async () => {
      const model = await project.mainModel();
      const allNames = await model.getVarNames();
      const stockNames = await model.getVarNames(SIMLIN_VARTYPE_STOCK);
      const flowNames = await model.getVarNames(SIMLIN_VARTYPE_FLOW);
      const auxNames = await model.getVarNames(SIMLIN_VARTYPE_AUX);
      const moduleNames = await model.getVarNames(SIMLIN_VARTYPE_MODULE);

      expect(Array.isArray(allNames)).toBe(true);
      expect(allNames.length).toBe(stockNames.length + flowNames.length + auxNames.length + moduleNames.length);
    });

    it('should include teacup temperature variable', async () => {
      const model = await project.mainModel();

      const teacupTemp = await model.getVariable('teacup temperature');
      expect(teacupTemp).toBeDefined();
      expect(teacupTemp!.type).toBe('stock');
    });

    it('should get a single variable by name', async () => {
      const model = await project.mainModel();

      const teacupTemp = await model.getVariable('teacup temperature');
      expect(teacupTemp).toBeDefined();
      expect(teacupTemp!.type).toBe('stock');
      expect(teacupTemp!.name).toBe('teacup temperature');
    });

    it('should return undefined for non-existent variable', async () => {
      const model = await project.mainModel();

      const result = await model.getVariable('nonexistent_variable_xyz');
      expect(result).toBeUndefined();
    });

    it('getVarNames with stock type mask returns only stocks', async () => {
      const model = await project.mainModel();
      const stockNames = await model.getVarNames(SIMLIN_VARTYPE_STOCK);

      for (const name of stockNames) {
        const v = await model.getVariable(name);
        expect(v!.type).toBe('stock');
      }
    });

    it('getVarNames with flow type mask returns only flows', async () => {
      const model = await project.mainModel();
      const flowNames = await model.getVarNames(SIMLIN_VARTYPE_FLOW);

      for (const name of flowNames) {
        const v = await model.getVariable(name);
        expect(v!.type).toBe('flow');
      }
    });

    it('getVarNames with aux type mask returns only auxiliaries', async () => {
      const model = await project.mainModel();
      const auxNames = await model.getVarNames(SIMLIN_VARTYPE_AUX);

      for (const name of auxNames) {
        const v = await model.getVariable(name);
        expect(v!.type).toBe('aux');
      }
    });

    it('should get time spec', async () => {
      const model = await project.mainModel();
      const timeSpec = await model.timeSpec();

      expect(typeof timeSpec.start).toBe('number');
      expect(typeof timeSpec.stop).toBe('number');
      expect(typeof timeSpec.dt).toBe('number');
      expect(timeSpec.stop).toBeGreaterThan(timeSpec.start);
      expect(timeSpec.dt).toBeGreaterThan(0);
    });

    it('should get structural loops', async () => {
      const model = await project.mainModel();
      const loops = await model.loops();

      expect(Array.isArray(loops)).toBe(true);
    });

    it('should get incoming links for a variable', async () => {
      const model = await project.mainModel();
      const flowNames = await model.getVarNames(SIMLIN_VARTYPE_FLOW);

      // Find a flow that has dependencies
      if (flowNames.length > 0) {
        const incomingLinks = await model.getIncomingLinks(flowNames[0]);
        expect(Array.isArray(incomingLinks)).toBe(true);
      }
    });

    it('should get all causal links', async () => {
      const model = await project.mainModel();
      const links = await model.getLinks();

      expect(Array.isArray(links)).toBe(true);
      for (const link of links) {
        expect(typeof link.from).toBe('string');
        expect(typeof link.to).toBe('string');
        expect([LinkPolarity.Positive, LinkPolarity.Negative, LinkPolarity.Unknown]).toContain(link.polarity);
      }
    });

    it('should explain a variable', async () => {
      const model = await project.mainModel();
      const explanation = await model.explain('teacup temperature');

      expect(typeof explanation).toBe('string');
      expect(explanation.length).toBeGreaterThan(0);
      expect(explanation).toContain('teacup temperature');
    });

    it('should check model for issues', async () => {
      const model = await project.mainModel();
      const issues = await model.check();

      expect(Array.isArray(issues)).toBe(true);
      // teacup model should have no issues
      expect(issues.length).toBe(0);
    });
  });

  describe('Sim class (step-by-step simulation)', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(async () => {
      await project.dispose();
    });

    it('should create a simulation from a model', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      expect(sim).toBeInstanceOf(Sim);
      await sim.dispose();
    });

    it('should get current time', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();
      const timeSpec = await model.timeSpec();

      const time = await sim.time();
      expect(typeof time).toBe('number');
      expect(time).toBe(timeSpec.start);

      await sim.dispose();
    });

    it('should run to a specific time', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();
      const timeSpec = await model.timeSpec();

      const targetTime = timeSpec.start + 5;
      await sim.runTo(targetTime);

      // Time should be at or past the target
      expect(await sim.time()).toBeGreaterThanOrEqual(targetTime);

      await sim.dispose();
    });

    it('should run to end', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();
      const timeSpec = await model.timeSpec();

      await sim.runToEnd();

      // Time should be at the end
      expect(await sim.time()).toBe(timeSpec.stop);

      await sim.dispose();
    });

    it('should reset simulation', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();
      const timeSpec = await model.timeSpec();

      await sim.runToEnd();
      await sim.reset();

      expect(await sim.time()).toBe(timeSpec.start);

      await sim.dispose();
    });

    it('should get step count', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      await sim.runToEnd();

      const stepCount = await sim.getStepCount();
      expect(stepCount).toBeGreaterThan(0);

      await sim.dispose();
    });

    it('should get variable value', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();
      const timeSpec = await model.timeSpec();

      await sim.runTo(timeSpec.start + 1);

      const value = await sim.getValue('teacup temperature');
      expect(typeof value).toBe('number');
      expect(isFinite(value)).toBe(true);

      await sim.dispose();
    });

    it('should set variable value', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      const newValue = 100;
      await sim.setValue('room temperature', newValue);

      const value = await sim.getValue('room temperature');
      expect(value).toBe(newValue);

      await sim.dispose();
    });

    it('should get time series for a variable', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      await sim.runToEnd();
      const series = await sim.getSeries('teacup temperature');

      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBe(await sim.getStepCount());

      // Verify temperature decreases over time (cooling)
      expect(series[0]).toBeGreaterThan(series[series.length - 1]);

      await sim.dispose();
    });

    it('should get variable names', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      const varNames = await sim.getVarNames();
      expect(Array.isArray(varNames)).toBe(true);
      // Simulation uses canonical names (underscores)
      expect(varNames).toContain('teacup_temperature');

      await sim.dispose();
    });

    it('should convert to a Run object', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate();

      await sim.runToEnd();
      const run = await sim.getRun();

      expect(run).toBeInstanceOf(Run);

      await sim.dispose();
    });

    it('should create simulation with overrides', async () => {
      const model = await project.mainModel();
      // Simulation uses canonical names (underscores)
      // Note: room_temperature is a constant aux, so it can be overridden
      const sim = await model.simulate({ room_temperature: 30 });

      // Override should be tracked
      expect(sim.overrides).toEqual({ room_temperature: 30 });

      // Override should affect initial state
      const initialRoomTemp = await sim.getValue('room_temperature');
      expect(initialRoomTemp).toBe(30);

      await sim.dispose();
    });

    it('should create simulation with LTM enabled', async () => {
      const model = await project.mainModel();
      const sim = await model.simulate({}, { enableLtm: true });

      await sim.runToEnd();

      // Should be able to get links with LTM scores
      const links = await sim.getLinks();
      expect(Array.isArray(links)).toBe(true);

      await sim.dispose();
    });
  });

  describe('Run class (completed simulation results)', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(async () => {
      await project.dispose();
    });

    it('should run a simulation and get Run object', async () => {
      const model = await project.mainModel();
      const run = await model.run();

      expect(run).toBeInstanceOf(Run);
    });

    it('should get results as a map of series', async () => {
      const model = await project.mainModel();
      const run = await model.run();

      const results = run.results;
      expect(results).toBeInstanceOf(Map);
      // Results use canonical names (underscores)
      expect(results.has('teacup_temperature')).toBe(true);
      expect(results.has('time')).toBe(true);

      const tempSeries = results.get('teacup_temperature')!;
      expect(tempSeries).toBeInstanceOf(Float64Array);
    });

    it('should get series for a specific variable', async () => {
      const model = await project.mainModel();
      const run = await model.run();

      // Results use canonical names (underscores)
      const series = run.getSeries('teacup_temperature');
      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBeGreaterThan(0);
    });

    it('should get time series', async () => {
      const model = await project.mainModel();
      const run = await model.run();
      const timeSpec = await model.timeSpec();

      const time = run.time;
      expect(time).toBeInstanceOf(Float64Array);
      expect(time[0]).toBe(timeSpec.start);
      expect(time[time.length - 1]).toBe(timeSpec.stop);
    });

    it('should get overrides', async () => {
      const model = await project.mainModel();
      // Overrides use canonical names (underscores)
      const overrides = { room_temperature: 25 };
      const run = await model.run(overrides);

      expect(run.overrides).toEqual(overrides);
    });

    it('should get loops with behavior data', async () => {
      const model = await project.mainModel();
      const run = await model.run();

      const loops = run.loops;
      expect(Array.isArray(loops)).toBe(true);
    });

    it('should get variable names', async () => {
      const model = await project.mainModel();
      const run = await model.run();

      const varNames = run.varNames;
      expect(Array.isArray(varNames)).toBe(true);
      // Variable names use canonical form (underscores)
      expect(varNames).toContain('teacup_temperature');
    });
  });

  describe('Model.baseCase', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(async () => {
      await project.dispose();
    });

    it('should compute base case on first access', async () => {
      const model = await project.mainModel();
      const baseCase = await model.baseCase();

      expect(baseCase).toBeInstanceOf(Run);
      expect(baseCase.overrides).toEqual({});
    });

    it('should cache base case', async () => {
      const model = await project.mainModel();
      const baseCase1 = await model.baseCase();
      const baseCase2 = await model.baseCase();

      // Should be the same instance (cached)
      expect(baseCase1).toBe(baseCase2);
    });
  });

  describe('ModelPatchBuilder', () => {
    it('should build an empty patch', () => {
      const builder = new ModelPatchBuilder('test_model');
      expect(builder.hasOperations()).toBe(false);

      const patch = builder.build();
      expect(patch.name).toBe('test_model');
      expect(patch.ops).toEqual([]);
    });

    it('should add upsert stock operation', () => {
      const builder = new ModelPatchBuilder('test_model');
      const stock: JsonStock = {
        name: 'population',
        initialEquation: '100',
        inflows: ['births'],
        outflows: ['deaths'],
      };

      builder.upsertStock(stock);

      expect(builder.hasOperations()).toBe(true);
      const patch = builder.build();
      expect(patch.ops.length).toBe(1);
      expect(patch.ops[0]).toEqual({ type: 'upsertStock', payload: { stock } });
    });

    it('should add upsert flow operation', () => {
      const builder = new ModelPatchBuilder('test_model');
      const flow: JsonFlow = {
        name: 'births',
        equation: 'population * birth_rate',
      };

      builder.upsertFlow(flow);

      const patch = builder.build();
      expect(patch.ops[0]).toEqual({ type: 'upsertFlow', payload: { flow } });
    });

    it('should add upsert aux operation', () => {
      const builder = new ModelPatchBuilder('test_model');
      const aux: JsonAuxiliary = {
        name: 'birth_rate',
        equation: '0.03',
      };

      builder.upsertAux(aux);

      const patch = builder.build();
      expect(patch.ops[0]).toEqual({ type: 'upsertAux', payload: { aux } });
    });

    it('should add delete variable operation', () => {
      const builder = new ModelPatchBuilder('test_model');
      builder.deleteVariable('old_var');

      const patch = builder.build();
      expect(patch.ops[0]).toEqual({ type: 'deleteVariable', payload: { ident: 'old_var' } });
    });

    it('should add rename variable operation', () => {
      const builder = new ModelPatchBuilder('test_model');
      builder.renameVariable('old_name', 'new_name');

      const patch = builder.build();
      expect(patch.ops[0]).toEqual({ type: 'renameVariable', payload: { from: 'old_name', to: 'new_name' } });
    });

    it('should accumulate multiple operations', () => {
      const builder = new ModelPatchBuilder('test_model');

      builder.upsertStock({ name: 'stock1', initialEquation: '0' });
      builder.upsertFlow({ name: 'flow1', equation: '1' });
      builder.upsertAux({ name: 'aux1', equation: '2' });
      builder.deleteVariable('old_var');

      const patch = builder.build();
      expect(patch.ops.length).toBe(4);
    });
  });

  describe('Model.edit() context', () => {
    let project: Project;

    beforeEach(async () => {
      project = await openTestProject();
    });

    afterEach(async () => {
      await project.dispose();
    });

    it('should provide current variables and patch builder', async () => {
      const model = await project.mainModel();

      await model.edit((currentVars, patch) => {
        expect(typeof currentVars).toBe('object');
        expect(patch).toBeInstanceOf(ModelPatchBuilder);

        // Should have the existing variables
        expect(currentVars['teacup temperature']).toBeDefined();
      });
    });

    it('should apply patch after edit completes', async () => {
      const model = await project.mainModel();

      // Add a new auxiliary variable
      await model.edit((currentVars, patch) => {
        patch.upsertAux({
          name: 'new_constant',
          equation: '42',
        });
      });

      // After edit, the model should have the new variable
      const newConst = await model.getVariable('new_constant');
      expect(newConst).toBeDefined();
      expect(newConst!.type).toBe('aux');
      if (newConst!.type === 'aux') {
        expect(newConst!.equation).toBe('42');
      }
    });

    it('should not apply patch if no operations added', async () => {
      const model = await project.mainModel();
      const originalAuxCount = (await model.getVarNames(SIMLIN_VARTYPE_AUX)).length;

      await model.edit(() => {
        // Don't add any operations
      });

      // No change should occur
      expect((await model.getVarNames(SIMLIN_VARTYPE_AUX)).length).toBe(originalAuxCount);
    });

    it('should support dry run mode', async () => {
      const model = await project.mainModel();
      const originalAuxCount = (await model.getVarNames(SIMLIN_VARTYPE_AUX)).length;

      await model.edit(
        (currentVars, patch) => {
          patch.upsertAux({
            name: 'dry_run_aux',
            equation: '123',
          });
        },
        { dryRun: true },
      );

      // In dry run mode, changes should NOT be applied
      expect((await model.getVarNames(SIMLIN_VARTYPE_AUX)).length).toBe(originalAuxCount);
      const dryRunAux = await model.getVariable('dry_run_aux');
      expect(dryRunAux).toBeUndefined();
    });

    it('should invalidate caches after edit', async () => {
      const model = await project.mainModel();

      // Get stock count before
      const stockNamesBefore = await model.getVarNames(SIMLIN_VARTYPE_STOCK);
      expect(stockNamesBefore.length).toBeGreaterThan(0);

      // Add a new stock
      await model.edit((currentVars, patch) => {
        patch.upsertStock({
          name: 'new_stock',
          initialEquation: '50',
          inflows: [],
          outflows: [],
        });
      });

      // Stocks should include new stock
      const stockNamesAfter = await model.getVarNames(SIMLIN_VARTYPE_STOCK);
      expect(stockNamesAfter.length).toBe(stockNamesBefore.length + 1);
      const newStock = await model.getVariable('new_stock');
      expect(newStock).toBeDefined();
    });
  });

  describe('Project.open* factory methods', () => {
    it('should load from XMILE data and access mainModel', async () => {
      const project = await openTestProject();
      const model = await project.mainModel();

      expect(model).toBeInstanceOf(Model);
      const teacupTemp = await model.getVariable('teacup temperature');
      expect(teacupTemp).toBeDefined();

      await project.dispose();
    });

    it('should load from JSON string and access mainModel', async () => {
      const project1 = await openTestProject();
      const json = await project1.serializeJson();
      await project1.dispose();

      const project2 = await Project.openJson(json);
      const model = await project2.mainModel();
      expect(model).toBeInstanceOf(Model);

      await project2.dispose();
    });
  });

  describe('Resource management', () => {
    it('should properly dispose project', async () => {
      const project = await openTestProject();

      // Access model before dispose
      const model = await project.mainModel();
      expect(model).toBeInstanceOf(Model);

      // Dispose should not throw
      await project.dispose();

      // Accessing disposed project should throw or return invalid state
      await expect(project.getModelNames()).rejects.toThrow();
    });

    it('should properly dispose simulation', async () => {
      const project = await openTestProject();
      const model = await project.mainModel();
      const sim = await model.simulate();

      await sim.runToEnd();

      // Dispose should not throw
      await sim.dispose();

      // Accessing disposed sim should throw
      await expect(sim.getValue('teacup temperature')).rejects.toThrow();

      await project.dispose();
    });

    it('should support using statement pattern (Symbol.dispose)', async () => {
      // Test that dispose method exists and can be called
      const project = await openTestProject();
      expect(typeof project.dispose).toBe('function');
      await project.dispose();
    });
  });

  describe('Issue fixes', () => {
    // Test for: Model.timeSpec should use model-level simSpecs when present
    it('should use model-level simSpecs when present', async () => {
      // Create a project with model-level simSpecs override via JSON
      const projectJson = {
        name: 'test_project',
        simSpecs: {
          startTime: 0,
          endTime: 100,
          dt: '1',
          timeUnits: 'years',
        },
        models: [
          {
            name: 'model_with_override',
            simSpecs: {
              startTime: 10,
              endTime: 50,
              dt: '0.5',
              timeUnits: 'months',
            },
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'x', equation: '1' }],
          },
          {
            name: 'model_without_override',
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'y', equation: '2' }],
          },
        ],
      };

      const project = await Project.openJson(JSON.stringify(projectJson));

      // Model with override should use model-level sim_specs
      const modelWithOverride = await project.getModel('model_with_override');
      const overrideTimeSpec = await modelWithOverride.timeSpec();
      expect(overrideTimeSpec.start).toBe(10);
      expect(overrideTimeSpec.stop).toBe(50);
      expect(overrideTimeSpec.dt).toBe(0.5);

      // Model without override should use project-level sim_specs
      const modelWithoutOverride = await project.getModel('model_without_override');
      const defaultTimeSpec = await modelWithoutOverride.timeSpec();
      expect(defaultTimeSpec.start).toBe(0);
      expect(defaultTimeSpec.stop).toBe(100);
      expect(defaultTimeSpec.dt).toBe(1);

      await project.dispose();
    });

    // Test for: arrayed stock reads initial equation from arrayedEquation.equation
    it('should read arrayed stock initialEquation correctly', async () => {
      const projectJson = {
        name: 'test_project',
        simSpecs: {
          startTime: 0,
          endTime: 10,
          dt: '1',
        },
        dimensions: [{ name: 'Region', elements: ['north', 'south'] }],
        models: [
          {
            name: 'main',
            stocks: [
              {
                name: 'population',
                arrayedEquation: {
                  dimensions: ['Region'],
                  equation: '1000',
                },
                inflows: [],
                outflows: [],
              },
            ],
            flows: [],
            auxiliaries: [],
          },
        ],
      };

      const project = await Project.openJson(JSON.stringify(projectJson));
      const model = await project.mainModel();

      const stock = await model.getVariable('population');
      expect(stock).toBeDefined();
      expect(stock!.type).toBe('stock');
      if (stock!.type === 'stock') {
        expect(stock!.initialEquation).toBe('1000');
        expect(stock!.arrayedEquation?.dimensions).toEqual(['Region']);
      }

      await project.dispose();
    });

    // Test for: XMILE-sourced arrayed stocks store initial value in arrayedEquation.equation
    it('should read XMILE-sourced arrayed stock initialEquation from equation field', async () => {
      const subscriptedPath = path.join(
        __dirname,
        '..',
        '..',
        '..',
        'test',
        'test-models',
        'tests',
        'subscript_multiples',
        'test_multiple_subscripts.stmx',
      );
      const xmileData = fs.readFileSync(subscriptedPath);
      const project = await Project.open(xmileData);
      const model = await project.mainModel();

      const stockA = await model.getVariable('Stock A');
      expect(stockA).toBeDefined();
      expect(stockA!.type).toBe('stock');
      if (stockA!.type === 'stock') {
        expect(stockA!.initialEquation).toBe('0');
      }

      await project.dispose();
    });

    // Test for: Model.check() should filter results to this model only
    it('should filter check() results to this model only', async () => {
      // Use the modules test model which has multiple models
      const modulesPath = path.join(
        __dirname,
        '..',
        '..',
        '..',
        'test',
        'modules_with_complex_idents',
        'modules_with_complex_idents.stmx',
      );
      if (!fs.existsSync(modulesPath)) {
        throw new Error('Required test model not found: ' + modulesPath);
      }
      const xmileData = fs.readFileSync(modulesPath);
      const project = await Project.open(xmileData);

      // This project has multiple models (main, 'a', 'b')
      const modelNames = await project.getModelNames();
      expect(modelNames.length).toBeGreaterThan(1);

      // Get all project errors to understand what we're filtering
      const allProjectErrors = await project.getErrors();

      // For each model, check() should only return errors for THAT model
      for (const modelName of modelNames) {
        const model = await project.getModel(modelName);
        const modelIssues = await model.check();

        // Get the actual model name from JSON for comparison
        // (since modelName could be null for main model)
        const projectJson = JSON.parse(await project.serializeJson());
        const modelJson = projectJson.models.find(
          (m: { name: string }) => m.name === modelName || (modelName === null && m.name),
        );
        const actualModelName = modelJson?.name;

        // Filter project errors to find only those for this model
        const expectedErrorsForModel = allProjectErrors.filter((e) => e.modelName === actualModelName);

        // The model's check() should return exactly the errors for this model
        expect(modelIssues.length).toBe(expectedErrorsForModel.length);
      }

      await project.dispose();
    });

    // Test that main model errors don't leak to other models
    it('should not return errors from other models', async () => {
      // Create project with error in main model only
      const projectJson = {
        name: 'test_project',
        simSpecs: {
          startTime: 0,
          endTime: 10,
          dt: '1',
        },
        models: [
          {
            name: 'main',
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'bad_var', equation: 'unknown_reference' }],
          },
        ],
      };

      const project = await Project.openJson(JSON.stringify(projectJson));

      // Get all project errors
      const allErrors = await project.getErrors();

      // main model should report the error
      const mainModel = await project.mainModel();
      const mainIssues = await mainModel.check();

      // If project reports errors for 'main', main model should report them
      const mainErrors = allErrors.filter((e) => e.modelName === 'main');
      expect(mainIssues.length).toBe(mainErrors.length);

      // Verify the error is about the unknown reference
      if (mainIssues.length > 0) {
        expect(mainIssues[0].message).toContain('unknown_reference');
      }

      await project.dispose();
    });

    // Test filtering with actual multi-model errors
    it('should correctly attribute errors to their respective models', async () => {
      // Use the modules model and verify filtering logic
      const modulesPath = path.join(
        __dirname,
        '..',
        '..',
        '..',
        'test',
        'modules_with_complex_idents',
        'modules_with_complex_idents.stmx',
      );
      const xmileData = fs.readFileSync(modulesPath);
      const project = await Project.open(xmileData);

      // Get errors per model
      const allErrors = await project.getErrors();
      const modelNames = await project.getModelNames();

      // Count errors per model name
      const errorCountByModel = new Map<string, number>();
      for (const error of allErrors) {
        if (error.modelName) {
          const count = errorCountByModel.get(error.modelName) || 0;
          errorCountByModel.set(error.modelName, count + 1);
        }
      }

      // Verify each model's check() returns correct count
      for (const modelName of modelNames) {
        const model = await project.getModel(modelName);
        const issues = await model.check();
        const expectedCount = errorCountByModel.get(modelName) || 0;
        expect(issues.length).toBe(expectedCount);
      }

      await project.dispose();
    });

    // Test for: Edit callback should not crash if callback throws
    it('should handle errors in edit callback gracefully', async () => {
      const project = await openTestProject();
      const model = await project.mainModel();

      // Callback that throws an error
      await expect(
        model.edit(() => {
          throw new Error('Simulated user error');
        }),
      ).rejects.toThrow('Simulated user error');

      // Model should still be usable after failed edit
      expect((await model.getVarNames(SIMLIN_VARTYPE_STOCK)).length).toBeGreaterThan(0);
      await expect(model.getVarNames()).resolves.toBeDefined();

      await project.dispose();
    });

    // Test for: Project.dispose() should dispose cached models
    it('should dispose cached models when project is disposed', async () => {
      const project = await openTestProject();

      // Access the main model to cache it
      const model = await project.mainModel();
      expect(model).toBeDefined();

      // Dispose project
      await project.dispose();

      // Accessing the model after project disposal should throw
      // (because the model was disposed along with the project)
      await expect(model.getVarNames()).rejects.toThrow();
    });

    // Test for: Link polarity should be validated at runtime
    it('should have valid link polarity values', async () => {
      const project = await openTestProject();
      const model = await project.mainModel();

      const links = await model.getLinks();

      for (const link of links) {
        expect([LinkPolarity.Positive, LinkPolarity.Negative, LinkPolarity.Unknown]).toContain(link.polarity);
      }

      await project.dispose();
    });

    // Test for: Link view polarity and useLetteredPolarity should round-trip through JSON
    it('should preserve link polarity and useLetteredPolarity on JSON round-trip', async () => {
      const projectJson = {
        name: 'test_project',
        simSpecs: {
          startTime: 0,
          endTime: 10,
          dt: '1',
        },
        models: [
          {
            name: 'main',
            stocks: [],
            flows: [],
            auxiliaries: [
              { name: 'a', equation: '1' },
              { name: 'b', equation: 'a' },
            ],
            views: [
              {
                elements: [
                  { type: 'aux', uid: 1, name: 'a', x: 100, y: 100 },
                  { type: 'aux', uid: 2, name: 'b', x: 200, y: 100 },
                  { type: 'link', uid: 3, fromUid: 1, toUid: 2, polarity: '+' },
                ],
                useLetteredPolarity: true,
              },
            ],
          },
        ],
      };

      const project = await Project.openJson(JSON.stringify(projectJson));
      const json = await project.serializeJson();
      const parsed = JSON.parse(json);

      const view = parsed.models[0].views[0];
      expect(view.useLetteredPolarity).toBe(true);

      const linkElem = view.elements.find((e: { type: string }) => e.type === 'link');
      expect(linkElem).toBeDefined();
      expect(linkElem.polarity).toBe('+');

      await project.dispose();
    });
  });

  describe('Canonical model name resolution', () => {
    // The Rust FFI resolves canonical name variants (e.g. "my_model" -> "My Model"),
    // and the Model class must store the resolved display name so that edit()
    // patches and check() error filtering work correctly.

    function makeMultiModelProject(modelName: string): string {
      return JSON.stringify({
        name: 'test_project',
        simSpecs: { startTime: 0, endTime: 10, dt: '1' },
        models: [
          {
            name: modelName,
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'x', equation: '1' }],
          },
        ],
      });
    }

    it('should resolve model name for edit()', async () => {
      const project = await Project.openJson(makeMultiModelProject('My Model'));

      // Fetch via canonical alias -- the Rust FFI resolves this
      const model = await project.getModel('my_model');

      // edit() must use the resolved display name in the patch, not the alias.
      // allowErrors because the engine may report compilation warnings for
      // non-standard model names.
      await model.edit(
        (_currentVars, patch) => {
          patch.upsertAux({ name: 'new_var', equation: '42' });
        },
        { allowErrors: true },
      );

      const newVar = await model.getVariable('new_var');
      expect(newVar).toBeDefined();

      await project.dispose();
    });

    it('should resolve model name for check()', async () => {
      // Create a model with an error (unknown reference).
      // Use "Main" (capitalized) so the engine can still resolve and compile
      // the model, while we fetch via the lowercase canonical alias "main".
      const projectJson = JSON.stringify({
        name: 'test_project',
        simSpecs: { startTime: 0, endTime: 10, dt: '1' },
        models: [
          {
            name: 'Main',
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'bad_var', equation: 'unknown_ref' }],
          },
        ],
      });

      const project = await Project.openJson(projectJson);

      // Fetch via lowercase canonical alias
      const model = await project.getModel('main');
      const issues = await model.check();

      // check() should find the error despite the name casing difference
      expect(issues.length).toBeGreaterThan(0);
      expect(issues.some((i) => i.variable === 'bad_var')).toBe(true);

      await project.dispose();
    });

    it('should expose the resolved display name via model.name', async () => {
      const project = await Project.openJson(makeMultiModelProject('My Model'));

      const model = await project.getModel('my_model');
      expect(model.name).toBe('My Model');

      await project.dispose();
    });
  });

  describe('Vensim MDL support', () => {
    it('should load MDL file', async () => {
      const mdlData = loadTestMdl();
      const project = await Project.openVensim(mdlData);

      expect(project).toBeInstanceOf(Project);
      expect(await project.modelCount()).toBeGreaterThan(0);

      // The teacup model should have the expected variables
      const model = await project.mainModel();
      const varNames = await model.getVarNames();
      expect(varNames).toContain('teacup_temperature');

      await project.dispose();
    });

    it('should accept MDL data as string', async () => {
      const mdlData = loadTestMdl();
      const mdlString = new TextDecoder().decode(mdlData);
      const project = await Project.openVensim(mdlString);

      expect(project).toBeInstanceOf(Project);
      await project.dispose();
    });

    it('should simulate models loaded from MDL', async () => {
      const mdlData = loadTestMdl();
      const project = await Project.openVensim(mdlData);
      const model = await project.mainModel();

      // Run simulation
      const run = await model.run();
      expect(run).toBeInstanceOf(Run);

      // Get results
      const results = run.results;
      expect(results.size).toBeGreaterThan(0);

      // Check that teacup temperature series exists and has expected behavior
      // (temperature should decrease over time as teacup cools)
      const tempSeries = results.get('teacup_temperature');
      if (tempSeries && tempSeries.length > 1) {
        expect(tempSeries[0]).toBeGreaterThan(tempSeries[tempSeries.length - 1]);
      }

      await project.dispose();
    });
  });
});
