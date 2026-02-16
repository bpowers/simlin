// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Tests for DirectBackend: verifies the backend interface contract
 * with the same operations and expected results as api.test.ts.
 */

import * as fs from 'fs';
import * as path from 'path';

import { DirectBackend } from '../src/direct-backend';
import { ProjectHandle, ModelHandle, SimHandle } from '../src/backend';
import { SimlinJsonFormat } from '../src/internal/types';
import { LinkPolarity } from '../src/types';

function loadWasmBuffer(): Buffer {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  return fs.readFileSync(wasmPath);
}

function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('DirectBackend', () => {
  let backend: DirectBackend;

  beforeAll(async () => {
    backend = new DirectBackend();
    backend.reset();
    backend.configureWasm({ source: loadWasmBuffer() });
    await backend.init();
  });

  afterAll(() => {
    backend.reset();
  });

  describe('lifecycle', () => {
    it('should report initialized after init', () => {
      expect(backend.isInitialized()).toBe(true);
    });
  });

  describe('project operations', () => {
    let projectHandle: ProjectHandle;

    beforeEach(() => {
      projectHandle = backend.projectOpenXmile(loadTestXmile());
    });

    afterEach(() => {
      backend.projectDispose(projectHandle);
    });

    it('should open XMILE project and return a valid handle', () => {
      expect(typeof projectHandle).toBe('number');
      expect(projectHandle).toBeGreaterThan(0);
    });

    it('should get model count', () => {
      const count = backend.projectGetModelCount(projectHandle);
      expect(count).toBeGreaterThan(0);
    });

    it('should get model names', () => {
      const names = backend.projectGetModelNames(projectHandle);
      expect(Array.isArray(names)).toBe(true);
      expect(names.length).toBeGreaterThan(0);
    });

    it('should check simulatable', () => {
      const simulatable = backend.projectIsSimulatable(projectHandle, null);
      expect(simulatable).toBe(true);
    });

    it('should serialize to protobuf and reopen', () => {
      const protobuf = backend.projectSerializeProtobuf(projectHandle);
      expect(protobuf).toBeInstanceOf(Uint8Array);
      expect(protobuf.length).toBeGreaterThan(0);

      const project2 = backend.projectOpenProtobuf(protobuf);
      const names2 = backend.projectGetModelNames(project2);
      expect(names2).toEqual(backend.projectGetModelNames(projectHandle));
      backend.projectDispose(project2);
    });

    it('should serialize to JSON', () => {
      const jsonBytes = backend.projectSerializeJson(projectHandle, SimlinJsonFormat.Native);
      const json = new TextDecoder().decode(jsonBytes);
      const parsed = JSON.parse(json);
      expect(parsed).toHaveProperty('models');
      expect(parsed).toHaveProperty('simSpecs');
    });

    it('should serialize to XMILE', () => {
      const xmile = backend.projectSerializeXmile(projectHandle);
      expect(xmile).toBeInstanceOf(Uint8Array);
      expect(xmile.length).toBeGreaterThan(0);
    });

    it('should get loops', () => {
      const loops = backend.projectGetLoops(projectHandle);
      expect(Array.isArray(loops)).toBe(true);
    });

    it('should get errors (none for valid model)', () => {
      const errors = backend.projectGetErrors(projectHandle);
      expect(Array.isArray(errors)).toBe(true);
      expect(errors.length).toBe(0);
    });

    it('should open from JSON string', () => {
      const jsonBytes = backend.projectSerializeJson(projectHandle, SimlinJsonFormat.Native);
      const project2 = backend.projectOpenJson(jsonBytes, SimlinJsonFormat.Native);
      expect(backend.projectGetModelNames(project2).length).toBeGreaterThan(0);
      backend.projectDispose(project2);
    });
  });

  describe('model operations', () => {
    let projectHandle: ProjectHandle;
    let modelHandle: ModelHandle;

    beforeEach(() => {
      projectHandle = backend.projectOpenXmile(loadTestXmile());
      modelHandle = backend.projectGetModel(projectHandle, null);
    });

    afterEach(() => {
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('should get model handle', () => {
      expect(typeof modelHandle).toBe('number');
      expect(modelHandle).toBeGreaterThan(0);
    });

    it('should get causal links', () => {
      const links = backend.modelGetLinks(modelHandle);
      expect(Array.isArray(links)).toBe(true);
      for (const link of links) {
        expect(typeof link.from).toBe('string');
        expect(typeof link.to).toBe('string');
        expect([LinkPolarity.Positive, LinkPolarity.Negative, LinkPolarity.Unknown]).toContain(link.polarity);
      }
    });

    it('should get LaTeX equation', () => {
      const latex = backend.modelGetLatexEquation(modelHandle, 'teacup_temperature');
      expect(latex).not.toBeNull();
      expect(typeof latex).toBe('string');
    });

    it('should return null for non-existent variable LaTeX', () => {
      const latex = backend.modelGetLatexEquation(modelHandle, 'nonexistent_xyz');
      expect(latex).toBeNull();
    });

    it('should get single variable JSON', () => {
      const bytes = backend.modelGetVarJson(modelHandle, 'teacup_temperature');
      expect(bytes).toBeInstanceOf(Uint8Array);
      expect(bytes.length).toBeGreaterThan(0);
      const parsed = JSON.parse(new TextDecoder().decode(bytes));
      expect(parsed.type).toBe('stock');
      expect(parsed.name).toBe('teacup temperature');
    });

    it('should get variable names', () => {
      const names = backend.modelGetVarNames(modelHandle);
      expect(Array.isArray(names)).toBe(true);
      expect(names.length).toBeGreaterThan(0);
      for (const name of names) {
        expect(typeof name).toBe('string');
      }
    });

    it('should get variable names with type mask', () => {
      const allNames = backend.modelGetVarNames(modelHandle);
      const stockNames = backend.modelGetVarNames(modelHandle, 1);  // SIMLIN_VARTYPE_STOCK
      const flowNames = backend.modelGetVarNames(modelHandle, 2);   // SIMLIN_VARTYPE_FLOW
      const auxNames = backend.modelGetVarNames(modelHandle, 4);    // SIMLIN_VARTYPE_AUX
      const moduleNames = backend.modelGetVarNames(modelHandle, 8); // SIMLIN_VARTYPE_MODULE

      expect(stockNames.length).toBeGreaterThan(0);
      expect(allNames.length).toBe(stockNames.length + flowNames.length + auxNames.length + moduleNames.length);
    });

    it('should get sim specs JSON', () => {
      const bytes = backend.modelGetSimSpecsJson(modelHandle);
      expect(bytes).toBeInstanceOf(Uint8Array);
      expect(bytes.length).toBeGreaterThan(0);
      const parsed = JSON.parse(new TextDecoder().decode(bytes));
      expect(typeof parsed.startTime).toBe('number');
      expect(typeof parsed.endTime).toBe('number');
    });
  });

  describe('sim operations', () => {
    let projectHandle: ProjectHandle;
    let modelHandle: ModelHandle;
    let simHandle: SimHandle;

    beforeEach(() => {
      projectHandle = backend.projectOpenXmile(loadTestXmile());
      modelHandle = backend.projectGetModel(projectHandle, null);
      simHandle = backend.simNew(modelHandle, false);
    });

    afterEach(() => {
      backend.simDispose(simHandle);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('should create sim', () => {
      expect(typeof simHandle).toBe('number');
      expect(simHandle).toBeGreaterThan(0);
    });

    it('should get initial time', () => {
      const time = backend.simGetTime(simHandle);
      expect(typeof time).toBe('number');
    });

    it('should run to a specific time', () => {
      const targetTime = 5;
      backend.simRunTo(simHandle, targetTime);
      const time = backend.simGetTime(simHandle);
      expect(time).toBeGreaterThanOrEqual(targetTime);
    });

    it('should run to end', () => {
      backend.simRunToEnd(simHandle);
      const stepCount = backend.simGetStepCount(simHandle);
      expect(stepCount).toBeGreaterThan(0);
    });

    it('should get and set value', () => {
      backend.simSetValue(simHandle, 'room temperature', 100);
      const value = backend.simGetValue(simHandle, 'room temperature');
      expect(value).toBe(100);
    });

    it('should get time series after running', () => {
      backend.simRunToEnd(simHandle);
      const series = backend.simGetSeries(simHandle, 'teacup_temperature');
      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBeGreaterThan(0);
      // Temperature should decrease (cooling)
      expect(series[0]).toBeGreaterThan(series[series.length - 1]);
    });

    it('should get variable names', () => {
      const varNames = backend.simGetVarNames(simHandle);
      expect(Array.isArray(varNames)).toBe(true);
      expect(varNames).toContain('teacup_temperature');
    });

    it('should reset simulation', () => {
      backend.simRunToEnd(simHandle);
      backend.simReset(simHandle);
      const time = backend.simGetTime(simHandle);
      // After reset, time should be back to start
      expect(time).toBeLessThan(1);
    });

    it('should get links', () => {
      backend.simRunToEnd(simHandle);
      const links = backend.simGetLinks(simHandle);
      expect(Array.isArray(links)).toBe(true);
    });
  });

  describe('disposal semantics', () => {
    it('should allow double-dispose of project (idempotent)', () => {
      const handle = backend.projectOpenXmile(loadTestXmile());
      backend.projectDispose(handle);
      expect(() => backend.projectDispose(handle)).not.toThrow();
    });

    it('should allow double-dispose of model (idempotent)', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      backend.modelDispose(modelHandle);
      expect(() => backend.modelDispose(modelHandle)).not.toThrow();
      backend.projectDispose(projectHandle);
    });

    it('should allow double-dispose of sim (idempotent)', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const simHandle = backend.simNew(modelHandle, false);
      backend.simDispose(simHandle);
      expect(() => backend.simDispose(simHandle)).not.toThrow();
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('should throw when operating on disposed project', () => {
      const handle = backend.projectOpenXmile(loadTestXmile());
      backend.projectDispose(handle);
      expect(() => backend.projectGetModelNames(handle)).toThrow(/disposed/);
    });

    it('should throw when operating on disposed model', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      backend.modelDispose(modelHandle);
      expect(() => backend.modelGetLinks(modelHandle)).toThrow(/disposed/);
      backend.projectDispose(projectHandle);
    });

    it('should throw when operating on disposed sim', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const simHandle = backend.simNew(modelHandle, false);
      backend.simDispose(simHandle);
      expect(() => backend.simRunToEnd(simHandle)).toThrow(/disposed/);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('should invalidate child handles when project is disposed', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const simHandle = backend.simNew(modelHandle, false);

      backend.projectDispose(projectHandle);

      expect(() => backend.modelGetLinks(modelHandle)).toThrow(/disposed/);
      expect(() => backend.simRunToEnd(simHandle)).toThrow(/disposed/);
    });
  });

  describe('apply patch', () => {
    it('should apply an empty patch', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const errors = backend.projectApplyPatch(projectHandle, { models: [] }, false, false);
      expect(errors.length).toBe(0);
      backend.projectDispose(projectHandle);
    });

    it('should apply a patch that adds a variable', () => {
      const projectHandle = backend.projectOpenXmile(loadTestXmile());
      const modelNames = backend.projectGetModelNames(projectHandle);

      const patch = {
        models: [
          {
            name: modelNames[0],
            ops: [
              {
                type: 'upsertAux' as const,
                payload: {
                  aux: {
                    name: 'test_constant',
                    equation: '42',
                  },
                },
              },
            ],
          },
        ],
      };

      const errors = backend.projectApplyPatch(projectHandle, patch, false, true);
      expect(Array.isArray(errors)).toBe(true);

      // Verify the variable was added by serializing and checking
      const jsonBytes = backend.projectSerializeJson(projectHandle, SimlinJsonFormat.Native);
      const json = new TextDecoder().decode(jsonBytes);
      expect(json).toContain('test_constant');

      backend.projectDispose(projectHandle);
    });
  });

  describe('Vensim support', () => {
    it('should open Vensim MDL file', () => {
      const mdlPath = path.join(__dirname, '..', '..', '..', 'test', 'test-models', 'samples', 'teacup', 'teacup.mdl');
      if (!fs.existsSync(mdlPath)) {
        throw new Error('Required test MDL model not found: ' + mdlPath);
      }
      const mdlData = fs.readFileSync(mdlPath);
      const handle = backend.projectOpenVensim(mdlData);
      expect(backend.projectGetModelCount(handle)).toBeGreaterThan(0);
      backend.projectDispose(handle);
    });
  });
});
