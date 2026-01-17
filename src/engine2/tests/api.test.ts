// Copyright 2025 The Simlin Authors. All rights reserved.
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

import { Project, Model, Sim, Run, LinkPolarity, ModelPatchBuilder, configureWasm, ready } from '../src';
import { JsonStock, JsonFlow, JsonAuxiliary } from '../src/json-types';
import { reset } from '@system-dynamics/engine2/internal/wasm';

// Helper to load the WASM module
async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  reset();
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
      project.dispose();
    });

    it('should get model names', async () => {
      const project = await openTestProject();

      const modelNames = project.getModelNames();
      expect(Array.isArray(modelNames)).toBe(true);
      expect(modelNames.length).toBeGreaterThan(0);

      project.dispose();
    });

    it('should get the main model', async () => {
      const project = await openTestProject();

      const model = project.mainModel;
      expect(model).toBeInstanceOf(Model);

      project.dispose();
    });

    it('should get a model by name', async () => {
      const project = await openTestProject();

      const modelNames = project.getModelNames();
      const model = project.getModel(modelNames[0]);
      expect(model).toBeInstanceOf(Model);

      project.dispose();
    });

    it('should check if project is simulatable', async () => {
      const project = await openTestProject();

      const isSimulatable = project.isSimulatable();
      expect(isSimulatable).toBe(true);

      project.dispose();
    });

    it('should serialize to protobuf and back', async () => {
      const project1 = await openTestProject();

      const protobuf = project1.serializeProtobuf();
      expect(protobuf).toBeInstanceOf(Uint8Array);
      expect(protobuf.length).toBeGreaterThan(0);

      const project2 = await Project.openProtobuf(protobuf);
      expect(project2.getModelNames()).toEqual(project1.getModelNames());

      project1.dispose();
      project2.dispose();
    });

    it('should serialize to JSON', async () => {
      const project = await openTestProject();

      const json = project.serializeJson();
      expect(typeof json).toBe('string');

      const parsed = JSON.parse(json);
      expect(parsed).toHaveProperty('models');
      expect(parsed).toHaveProperty('simSpecs');

      project.dispose();
    });

    it('should get loops', async () => {
      const project = await openTestProject();

      const loops = project.getLoops();
      expect(Array.isArray(loops)).toBe(true);
      // The teacup model may or may not have feedback loops

      project.dispose();
    });

    it('should get errors', async () => {
      const project = await openTestProject();

      // The teacup model should have no errors
      const errors = project.getErrors();
      expect(Array.isArray(errors)).toBe(true);
      expect(errors.length).toBe(0);

      project.dispose();
    });
  });

  describe('Model class', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(() => {
      project.dispose();
    });

    it('should have a reference to its project', () => {
      const model = project.mainModel;
      expect(model.project).toBe(project);
    });

    it('should get stocks', () => {
      const model = project.mainModel;
      const stocks = model.stocks;

      expect(Array.isArray(stocks)).toBe(true);
      // teacup model has at least one stock (teacup temperature)
      expect(stocks.length).toBeGreaterThan(0);

      const stock = stocks[0];
      expect(stock.type).toBe('stock');
      expect(typeof stock.name).toBe('string');
      expect(typeof stock.initialEquation).toBe('string');
      expect(Array.isArray(stock.inflows)).toBe(true);
      expect(Array.isArray(stock.outflows)).toBe(true);
    });

    it('should get flows', () => {
      const model = project.mainModel;
      const flows = model.flows;

      expect(Array.isArray(flows)).toBe(true);
      // teacup model has flows

      for (const flow of flows) {
        expect(flow.type).toBe('flow');
        expect(typeof flow.name).toBe('string');
        expect(typeof flow.equation).toBe('string');
      }
    });

    it('should get auxiliaries', () => {
      const model = project.mainModel;
      const auxs = model.auxs;

      expect(Array.isArray(auxs)).toBe(true);

      for (const aux of auxs) {
        expect(aux.type).toBe('aux');
        expect(typeof aux.name).toBe('string');
        expect(typeof aux.equation).toBe('string');
      }
    });

    it('should get all variables', () => {
      const model = project.mainModel;
      const variables = model.variables;

      expect(Array.isArray(variables)).toBe(true);
      expect(variables.length).toBe(model.stocks.length + model.flows.length + model.auxs.length);
    });

    it('should include teacup temperature variable', () => {
      const model = project.mainModel;
      const variables = model.variables;

      const teacupTemp = variables.find((v) => v.name === 'teacup temperature');
      expect(teacupTemp).toBeDefined();
      expect(teacupTemp!.type).toBe('stock');
    });

    it('should get time spec', () => {
      const model = project.mainModel;
      const timeSpec = model.timeSpec;

      expect(typeof timeSpec.start).toBe('number');
      expect(typeof timeSpec.stop).toBe('number');
      expect(typeof timeSpec.dt).toBe('number');
      expect(timeSpec.stop).toBeGreaterThan(timeSpec.start);
      expect(timeSpec.dt).toBeGreaterThan(0);
    });

    it('should get structural loops', () => {
      const model = project.mainModel;
      const loops = model.loops;

      expect(Array.isArray(loops)).toBe(true);
    });

    it('should get incoming links for a variable', () => {
      const model = project.mainModel;

      // Find a flow that has dependencies
      const flow = model.flows[0];
      if (flow) {
        const incomingLinks = model.getIncomingLinks(flow.name);
        expect(Array.isArray(incomingLinks)).toBe(true);
      }
    });

    it('should get all causal links', () => {
      const model = project.mainModel;
      const links = model.getLinks();

      expect(Array.isArray(links)).toBe(true);
      for (const link of links) {
        expect(typeof link.from).toBe('string');
        expect(typeof link.to).toBe('string');
        expect([LinkPolarity.Positive, LinkPolarity.Negative, LinkPolarity.Unknown]).toContain(link.polarity);
      }
    });

    it('should explain a variable', () => {
      const model = project.mainModel;
      const explanation = model.explain('teacup temperature');

      expect(typeof explanation).toBe('string');
      expect(explanation.length).toBeGreaterThan(0);
      expect(explanation).toContain('teacup temperature');
    });

    it('should check model for issues', () => {
      const model = project.mainModel;
      const issues = model.check();

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

    afterAll(() => {
      project.dispose();
    });

    it('should create a simulation from a model', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      expect(sim).toBeInstanceOf(Sim);
      sim.dispose();
    });

    it('should get current time', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      const time = sim.time;
      expect(typeof time).toBe('number');
      expect(time).toBe(model.timeSpec.start);

      sim.dispose();
    });

    it('should run to a specific time', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      const targetTime = model.timeSpec.start + 5;
      sim.runTo(targetTime);

      // Time should be at or past the target
      expect(sim.time).toBeGreaterThanOrEqual(targetTime);

      sim.dispose();
    });

    it('should run to end', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();

      // Time should be at the end
      expect(sim.time).toBe(model.timeSpec.stop);

      sim.dispose();
    });

    it('should reset simulation', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();
      sim.reset();

      expect(sim.time).toBe(model.timeSpec.start);

      sim.dispose();
    });

    it('should get step count', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();

      const stepCount = sim.getStepCount();
      expect(stepCount).toBeGreaterThan(0);

      sim.dispose();
    });

    it('should get variable value', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runTo(model.timeSpec.start + 1);

      const value = sim.getValue('teacup temperature');
      expect(typeof value).toBe('number');
      expect(isFinite(value)).toBe(true);

      sim.dispose();
    });

    it('should set variable value', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      const newValue = 100;
      sim.setValue('teacup temperature', newValue);

      const value = sim.getValue('teacup temperature');
      expect(value).toBe(newValue);

      sim.dispose();
    });

    it('should get time series for a variable', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();
      const series = sim.getSeries('teacup temperature');

      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBe(sim.getStepCount());

      // Verify temperature decreases over time (cooling)
      expect(series[0]).toBeGreaterThan(series[series.length - 1]);

      sim.dispose();
    });

    it('should get variable names', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      const varNames = sim.getVarNames();
      expect(Array.isArray(varNames)).toBe(true);
      // Simulation uses canonical names (underscores)
      expect(varNames).toContain('teacup_temperature');

      sim.dispose();
    });

    it('should convert to a Run object', () => {
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();
      const run = sim.getRun();

      expect(run).toBeInstanceOf(Run);

      sim.dispose();
    });

    it('should create simulation with overrides', () => {
      const model = project.mainModel;
      // Simulation uses canonical names (underscores)
      // Note: room_temperature is a constant aux, so it can be overridden
      const sim = model.simulate({ room_temperature: 30 });

      // Override should be tracked
      expect(sim.overrides).toEqual({ room_temperature: 30 });

      // Override should affect initial state
      const initialRoomTemp = sim.getValue('room_temperature');
      expect(initialRoomTemp).toBe(30);

      sim.dispose();
    });

    it('should create simulation with LTM enabled', () => {
      const model = project.mainModel;
      const sim = model.simulate({}, { enableLtm: true });

      sim.runToEnd();

      // Should be able to get links with LTM scores
      const links = sim.getLinks();
      expect(Array.isArray(links)).toBe(true);

      sim.dispose();
    });
  });

  describe('Run class (completed simulation results)', () => {
    let project: Project;

    beforeAll(async () => {
      project = await openTestProject();
    });

    afterAll(() => {
      project.dispose();
    });

    it('should run a simulation and get Run object', () => {
      const model = project.mainModel;
      const run = model.run();

      expect(run).toBeInstanceOf(Run);
    });

    it('should get results as a map of series', () => {
      const model = project.mainModel;
      const run = model.run();

      const results = run.results;
      expect(results).toBeInstanceOf(Map);
      // Results use canonical names (underscores)
      expect(results.has('teacup_temperature')).toBe(true);
      expect(results.has('time')).toBe(true);

      const tempSeries = results.get('teacup_temperature')!;
      expect(tempSeries).toBeInstanceOf(Float64Array);
    });

    it('should get series for a specific variable', () => {
      const model = project.mainModel;
      const run = model.run();

      // Results use canonical names (underscores)
      const series = run.getSeries('teacup_temperature');
      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBeGreaterThan(0);
    });

    it('should get time series', () => {
      const model = project.mainModel;
      const run = model.run();

      const time = run.time;
      expect(time).toBeInstanceOf(Float64Array);
      expect(time[0]).toBe(model.timeSpec.start);
      expect(time[time.length - 1]).toBe(model.timeSpec.stop);
    });

    it('should get overrides', () => {
      const model = project.mainModel;
      // Overrides use canonical names (underscores)
      const overrides = { room_temperature: 25 };
      const run = model.run(overrides);

      expect(run.overrides).toEqual(overrides);
    });

    it('should get loops with behavior data', () => {
      const model = project.mainModel;
      const run = model.run();

      const loops = run.loops;
      expect(Array.isArray(loops)).toBe(true);
    });

    it('should get variable names', () => {
      const model = project.mainModel;
      const run = model.run();

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

    afterAll(() => {
      project.dispose();
    });

    it('should compute base case on first access', () => {
      const model = project.mainModel;
      const baseCase = model.baseCase;

      expect(baseCase).toBeInstanceOf(Run);
      expect(baseCase.overrides).toEqual({});
    });

    it('should cache base case', () => {
      const model = project.mainModel;
      const baseCase1 = model.baseCase;
      const baseCase2 = model.baseCase;

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

    afterEach(() => {
      project.dispose();
    });

    it('should provide current variables and patch builder', () => {
      const model = project.mainModel;

      model.edit((currentVars, patch) => {
        expect(typeof currentVars).toBe('object');
        expect(patch).toBeInstanceOf(ModelPatchBuilder);

        // Should have the existing variables
        expect(currentVars['teacup temperature']).toBeDefined();
      });
    });

    it('should apply patch after edit completes', () => {
      const model = project.mainModel;

      // Add a new auxiliary variable
      model.edit((currentVars, patch) => {
        patch.upsertAux({
          name: 'new_constant',
          equation: '42',
        });
      });

      // After edit, the model should have the new variable
      const auxs = model.auxs;
      const newConst = auxs.find((a) => a.name === 'new_constant');
      expect(newConst).toBeDefined();
      expect(newConst!.equation).toBe('42');
    });

    it('should not apply patch if no operations added', () => {
      const model = project.mainModel;
      const originalAuxCount = model.auxs.length;

      model.edit((currentVars, patch) => {
        // Don't add any operations
      });

      // No change should occur
      expect(model.auxs.length).toBe(originalAuxCount);
    });

    it('should support dry run mode', () => {
      const model = project.mainModel;
      const originalAuxCount = model.auxs.length;

      model.edit(
        (currentVars, patch) => {
          patch.upsertAux({
            name: 'dry_run_aux',
            equation: '123',
          });
        },
        { dryRun: true },
      );

      // In dry run mode, changes should NOT be applied
      expect(model.auxs.length).toBe(originalAuxCount);
      expect(model.auxs.find((a) => a.name === 'dry_run_aux')).toBeUndefined();
    });

    it('should invalidate caches after edit', () => {
      const model = project.mainModel;

      // Access stocks to populate cache
      const stocksBefore = model.stocks;
      expect(stocksBefore.length).toBeGreaterThan(0);

      // Add a new stock
      model.edit((currentVars, patch) => {
        patch.upsertStock({
          name: 'new_stock',
          initialEquation: '50',
          inflows: [],
          outflows: [],
        });
      });

      // Cache should be invalidated, stocks should include new stock
      const stocksAfter = model.stocks;
      expect(stocksAfter.length).toBe(stocksBefore.length + 1);
      expect(stocksAfter.find((s) => s.name === 'new_stock')).toBeDefined();
    });
  });

  describe('Project.open* factory methods', () => {
    it('should load from XMILE data and access mainModel', async () => {
      const project = await openTestProject();
      const model = project.mainModel;

      expect(model).toBeInstanceOf(Model);
      expect(model.variables.find((v) => v.name === 'teacup temperature')).toBeDefined();

      project.dispose();
    });

    it('should load from JSON string and access mainModel', async () => {
      const project1 = await openTestProject();
      const json = project1.serializeJson();
      project1.dispose();

      const project2 = await Project.openJson(json);
      const model = project2.mainModel;
      expect(model).toBeInstanceOf(Model);

      project2.dispose();
    });
  });

  describe('Resource management', () => {
    it('should properly dispose project', async () => {
      const project = await openTestProject();

      // Access model before dispose
      const model = project.mainModel;
      expect(model).toBeInstanceOf(Model);

      // Dispose should not throw
      project.dispose();

      // Accessing disposed project should throw or return invalid state
      expect(() => project.getModelNames()).toThrow();
    });

    it('should properly dispose simulation', async () => {
      const project = await openTestProject();
      const model = project.mainModel;
      const sim = model.simulate();

      sim.runToEnd();

      // Dispose should not throw
      sim.dispose();

      // Accessing disposed sim should throw
      expect(() => sim.getValue('teacup temperature')).toThrow();

      project.dispose();
    });

    it('should support using statement pattern (Symbol.dispose)', async () => {
      // Test that dispose method exists and can be called
      const project = await openTestProject();
      expect(typeof project.dispose).toBe('function');
      project.dispose();
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
      const modelWithOverride = project.getModel('model_with_override');
      expect(modelWithOverride.timeSpec.start).toBe(10);
      expect(modelWithOverride.timeSpec.stop).toBe(50);
      expect(modelWithOverride.timeSpec.dt).toBe(0.5);

      // Model without override should use project-level sim_specs
      const modelWithoutOverride = project.getModel('model_without_override');
      expect(modelWithoutOverride.timeSpec.start).toBe(0);
      expect(modelWithoutOverride.timeSpec.stop).toBe(100);
      expect(modelWithoutOverride.timeSpec.dt).toBe(1);

      project.dispose();
    });

    // Test for: Stock initialEquation should read from arrayedEquation.initialEquation
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
                  initialEquation: '1000',
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
      const model = project.mainModel;

      const stock = model.stocks.find((s) => s.name === 'population');
      expect(stock).toBeDefined();
      expect(stock!.initialEquation).toBe('1000');
      expect(stock!.dimensions).toEqual(['Region']);

      project.dispose();
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
      const modelNames = project.getModelNames();
      expect(modelNames.length).toBeGreaterThan(1);

      // Get all project errors to understand what we're filtering
      const allProjectErrors = project.getErrors();

      // For each model, check() should only return errors for THAT model
      for (const modelName of modelNames) {
        const model = project.getModel(modelName);
        const modelIssues = model.check();

        // Get the actual model name from JSON for comparison
        // (since modelName could be null for main model)
        const projectJson = JSON.parse(project.serializeJson());
        const modelJson = projectJson.models.find(
          (m: { name: string }) => m.name === modelName || (modelName === null && m.name),
        );
        const actualModelName = modelJson?.name;

        // Filter project errors to find only those for this model
        const expectedErrorsForModel = allProjectErrors.filter((e) => e.modelName === actualModelName);

        // The model's check() should return exactly the errors for this model
        expect(modelIssues.length).toBe(expectedErrorsForModel.length);
      }

      project.dispose();
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
      const allErrors = project.getErrors();

      // main model should report the error
      const mainModel = project.mainModel;
      const mainIssues = mainModel.check();

      // If project reports errors for 'main', main model should report them
      const mainErrors = allErrors.filter((e) => e.modelName === 'main');
      expect(mainIssues.length).toBe(mainErrors.length);

      // Verify the error is about the unknown reference
      if (mainIssues.length > 0) {
        expect(mainIssues[0].message).toContain('unknown_reference');
      }

      project.dispose();
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
      const allErrors = project.getErrors();
      const modelNames = project.getModelNames();

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
        const model = project.getModel(modelName);
        const issues = model.check();
        const expectedCount = errorCountByModel.get(modelName) || 0;
        expect(issues.length).toBe(expectedCount);
      }

      project.dispose();
    });

    // Test for: Edit callback should not crash if callback throws
    it('should handle errors in edit callback gracefully', async () => {
      const project = await openTestProject();
      const model = project.mainModel;

      // Callback that throws an error
      expect(() => {
        model.edit((currentVars, patch) => {
          throw new Error('Simulated user error');
        });
      }).toThrow('Simulated user error');

      // Model should still be usable after failed edit
      expect(model.stocks.length).toBeGreaterThan(0);
      expect(() => model.variables).not.toThrow();

      project.dispose();
    });

    // Test for: Project.dispose() should dispose cached models
    it('should dispose cached models when project is disposed', async () => {
      const project = await openTestProject();

      // Access the main model to cache it
      const model = project.mainModel;
      expect(model).toBeDefined();

      // Dispose project
      project.dispose();

      // Accessing the model after project disposal should throw
      // (because the model was disposed along with the project)
      expect(() => model.variables).toThrow();
    });

    // Test for: Link polarity should be validated at runtime
    it('should have valid link polarity values', async () => {
      const project = await openTestProject();
      const model = project.mainModel;

      const links = model.getLinks();

      for (const link of links) {
        expect([LinkPolarity.Positive, LinkPolarity.Negative, LinkPolarity.Unknown]).toContain(link.polarity);
      }

      project.dispose();
    });
  });
});
