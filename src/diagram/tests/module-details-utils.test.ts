// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment node
 */

import type { Project, Model, Variable, Aux, Stock, Module } from '@simlin/core/datamodel';

import {
  countModelInstances,
  wouldCreateCycle,
  getAvailableModels,
  getInputPorts,
  getPublicVariables,
} from '../module-details-utils';

// -- Test fixtures --

function makeAux(ident: string, overrides?: Partial<Aux>): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    gf: undefined,
    canBeModuleInput: false,
    isPublic: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeStock(ident: string, overrides?: Partial<Stock>): Stock {
  return {
    type: 'stock',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    inflows: [],
    outflows: [],
    nonNegative: false,
    canBeModuleInput: false,
    isPublic: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeModule(ident: string, modelName: string): Module {
  return {
    type: 'module',
    ident,
    modelName,
    documentation: '',
    units: '',
    references: [],
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
  };
}

function makeModel(name: string, variables: ReadonlyArray<Variable>): Model {
  const varMap = new Map<string, Variable>();
  for (const v of variables) {
    varMap.set(v.ident, v);
  }
  return {
    name,
    variables: varMap,
    views: [],
    loopMetadata: [],
    groups: [],
  };
}

function makeProject(models: ReadonlyArray<Model>): Project {
  const modelMap = new Map<string, Model>();
  for (const m of models) {
    modelMap.set(m.name, m);
  }
  return {
    name: 'test_project',
    simSpecs: {
      start: 0,
      stop: 10,
      dt: { value: 1, isReciprocal: false },
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: undefined,
    },
    models: modelMap,
    dimensions: new Map(),
    hasNoEquations: false,
    source: undefined,
  };
}

// -- Tests --

describe('countModelInstances', () => {
  it('returns 0 when no modules reference the model', () => {
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
      makeModel('hares', [makeAux('y')]),
    ]);
    expect(countModelInstances(project, 'hares')).toBe(0);
  });

  it('returns 1 when one module references the model', () => {
    const project = makeProject([
      makeModel('main', [makeModule('hares_mod', 'hares')]),
      makeModel('hares', [makeAux('population')]),
    ]);
    expect(countModelInstances(project, 'hares')).toBe(1);
  });

  it('returns 2 when two modules in different models reference the same model', () => {
    const project = makeProject([
      makeModel('main', [makeModule('hares1', 'hares')]),
      makeModel('outer', [makeModule('hares2', 'hares')]),
      makeModel('hares', [makeAux('population')]),
    ]);
    expect(countModelInstances(project, 'hares')).toBe(2);
  });

  it('does not count modules referencing a different model', () => {
    const project = makeProject([
      makeModel('main', [makeModule('foxes_mod', 'foxes'), makeModule('hares_mod', 'hares')]),
      makeModel('hares', [makeAux('h')]),
      makeModel('foxes', [makeAux('f')]),
    ]);
    expect(countModelInstances(project, 'hares')).toBe(1);
  });

  it('counts multiple modules in the same model', () => {
    const project = makeProject([
      makeModel('main', [makeModule('h1', 'hares'), makeModule('h2', 'hares')]),
      makeModel('hares', [makeAux('x')]),
    ]);
    expect(countModelInstances(project, 'hares')).toBe(2);
  });
});

describe('wouldCreateCycle', () => {
  it('detects direct self-reference', () => {
    const project = makeProject([
      makeModel('main', [makeModule('self', 'main')]),
    ]);
    expect(wouldCreateCycle(project, 'main', 'main')).toBe(true);
  });

  it('detects cycle in A -> B -> C -> A', () => {
    // Existing: A has module pointing to B, B has module pointing to C.
    // Proposed: add module in C pointing to A -- should detect cycle.
    const project = makeProject([
      makeModel('a', [makeModule('b_mod', 'b')]),
      makeModel('b', [makeModule('c_mod', 'c')]),
      makeModel('c', [makeAux('x')]),
    ]);
    expect(wouldCreateCycle(project, 'c', 'a')).toBe(true);
  });

  it('returns false when no cycle exists', () => {
    // A -> B. Adding A -> C does not create a cycle.
    const project = makeProject([
      makeModel('a', [makeModule('b_mod', 'b')]),
      makeModel('b', [makeAux('x')]),
      makeModel('c', [makeAux('y')]),
    ]);
    expect(wouldCreateCycle(project, 'a', 'c')).toBe(false);
  });

  it('returns false for a leaf model with no modules', () => {
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
      makeModel('sub', [makeAux('y')]),
    ]);
    expect(wouldCreateCycle(project, 'main', 'sub')).toBe(false);
  });

  it('handles models that do not exist in the project', () => {
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
    ]);
    // Referencing a nonexistent model cannot create a cycle
    expect(wouldCreateCycle(project, 'main', 'nonexistent')).toBe(false);
  });
});

describe('getAvailableModels', () => {
  it('excludes the current model name from project models', () => {
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
      makeModel('hares', [makeAux('y')]),
      makeModel('foxes', [makeAux('z')]),
    ]);
    const result = getAvailableModels(project, 'main');
    expect(result.projectModels).not.toContain('main');
    expect(result.projectModels).toContain('hares');
    expect(result.projectModels).toContain('foxes');
  });

  it('excludes models that would create cycles', () => {
    // main has module pointing to hares; hares is the current model.
    // From hares, referencing main would create a cycle, so main should be excluded.
    const project = makeProject([
      makeModel('main', [makeModule('h', 'hares')]),
      makeModel('hares', [makeAux('x')]),
      makeModel('foxes', [makeAux('y')]),
    ]);
    const result = getAvailableModels(project, 'hares');
    // main -> hares already exists; adding hares -> main creates cycle
    expect(result.projectModels).not.toContain('main');
    expect(result.projectModels).not.toContain('hares');
    expect(result.projectModels).toContain('foxes');
  });

  it('returns empty stdlib list (not yet exposed through project serialization)', () => {
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
    ]);
    const result = getAvailableModels(project, 'main');
    expect(result.stdlibModels).toEqual([]);
  });
});

describe('getInputPorts', () => {
  it('returns only variables with canBeModuleInput=true', () => {
    const model = makeModel('hares', [
      makeAux('food', { canBeModuleInput: true }),
      makeAux('rate', { canBeModuleInput: false }),
      makeStock('population', { canBeModuleInput: true }),
    ]);
    const inputs = getInputPorts(model);
    expect(inputs).toHaveLength(2);
    const idents = inputs.map((v) => v.ident);
    expect(idents).toContain('food');
    expect(idents).toContain('population');
    expect(idents).not.toContain('rate');
  });

  it('returns empty array when model has no input ports', () => {
    const model = makeModel('hares', [
      makeAux('food'),
      makeAux('rate'),
    ]);
    const inputs = getInputPorts(model);
    expect(inputs).toHaveLength(0);
  });

  it('excludes module variables (modules cannot be input ports)', () => {
    const model = makeModel('hares', [
      makeAux('food', { canBeModuleInput: true }),
      makeModule('sub', 'sub_model'),
    ]);
    const inputs = getInputPorts(model);
    expect(inputs).toHaveLength(1);
    expect(inputs[0].ident).toBe('food');
  });
});

describe('getPublicVariables', () => {
  it('returns only variables with isPublic=true', () => {
    const model = makeModel('hares', [
      makeAux('population', { isPublic: true }),
      makeAux('internal_rate', { isPublic: false }),
      makeStock('level', { isPublic: true }),
    ]);
    const pubs = getPublicVariables(model);
    expect(pubs).toHaveLength(2);
    const idents = pubs.map((v) => v.ident);
    expect(idents).toContain('population');
    expect(idents).toContain('level');
    expect(idents).not.toContain('internal_rate');
  });

  it('returns empty array when model has no public variables', () => {
    const model = makeModel('hares', [
      makeAux('food'),
      makeStock('level'),
    ]);
    const pubs = getPublicVariables(model);
    expect(pubs).toHaveLength(0);
  });

  it('excludes module variables', () => {
    const model = makeModel('hares', [
      makeAux('food', { isPublic: true }),
      makeModule('sub', 'sub_model'),
    ]);
    const pubs = getPublicVariables(model);
    expect(pubs).toHaveLength(1);
    expect(pubs[0].ident).toBe('food');
  });
});
