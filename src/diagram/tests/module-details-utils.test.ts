// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment node
 */

import type { Project, Model, Variable, Aux, Stock, Module, MacroSpec } from '@simlin/core/datamodel';

import {
  countModelInstances,
  wouldCreateCycle,
  getAvailableModels,
  getInputPorts,
  getPublicVariables,
} from '../module-details-utils';
import { isMacroModel } from '../module-navigation';

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

function makeModel(name: string, variables: ReadonlyArray<Variable>, macroSpec?: MacroSpec): Model {
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
    macroSpec,
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
    const project = makeProject([makeModel('main', [makeAux('x')]), makeModel('hares', [makeAux('y')])]);
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
    const project = makeProject([makeModel('main', [makeModule('self', 'main')])]);
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
    const project = makeProject([makeModel('main', [makeAux('x')]), makeModel('sub', [makeAux('y')])]);
    expect(wouldCreateCycle(project, 'main', 'sub')).toBe(false);
  });

  it('handles models that do not exist in the project', () => {
    const project = makeProject([makeModel('main', [makeAux('x')])]);
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

  it('always includes all 9 stdlib models from the registry', () => {
    const project = makeProject([makeModel('main', [makeAux('x')])]);
    const result = getAvailableModels(project, 'main');
    // All 9 stdlib models are offered even when none are in the project
    expect(result.stdlibModels).toHaveLength(9);
    expect(result.stdlibModels).toContain('stdlib\u{205A}systems_rate');
    expect(result.stdlibModels).toContain('stdlib\u{205A}delay1');
    expect(result.stdlibModels).toContain('stdlib\u{205A}smth1');
  });

  it('categorizes stdlib models separately from project models', () => {
    const project = makeProject([
      makeModel('main', [makeModule('rate_mod', 'stdlib\u{205A}systems_rate')]),
      makeModel('hares', [makeAux('x')]),
      makeModel('stdlib\u{205A}systems_rate', [makeAux('actual')]),
    ]);
    const result = getAvailableModels(project, 'main');
    expect(result.projectModels).toContain('hares');
    expect(result.projectModels).not.toContain('stdlib\u{205A}systems_rate');
    expect(result.stdlibModels).toContain('stdlib\u{205A}systems_rate');
  });

  it('does not duplicate stdlib models already in the project', () => {
    const project = makeProject([
      makeModel('main', [makeModule('rate_mod', 'stdlib\u{205A}systems_rate')]),
      makeModel('stdlib\u{205A}systems_rate', [makeAux('actual')]),
    ]);
    const result = getAvailableModels(project, 'main');
    // systems_rate should appear exactly once despite being both in the
    // project models map and in the stdlib registry
    const rateCount = result.stdlibModels.filter((n) => n === 'stdlib\u{205A}systems_rate').length;
    expect(rateCount).toBe(1);
    expect(result.projectModels).toHaveLength(0);
  });

  it('excludes stdlib shadow models that would create cycles', () => {
    // A user model shadowing a stdlib name that would create a cycle
    // must not be offered even though it appears in the stdlib registry.
    const project = makeProject([
      makeModel('main', [makeModule('rate_mod', 'stdlib\u{205A}systems_rate')]),
      // Shadow model that references main -- selecting it from main
      // would create a cycle: main -> systems_rate -> main
      makeModel('stdlib\u{205A}systems_rate', [makeModule('back', 'main')]),
    ]);
    const result = getAvailableModels(project, 'main');
    // systems_rate creates a cycle, so it should be excluded from the list
    expect(result.stdlibModels).not.toContain('stdlib\u{205A}systems_rate');
  });

  it('treats user models with bare stdlib names as project models', () => {
    // A user-created model named "delay1" (no stdlib prefix) should
    // appear in projectModels, not stdlibModels.
    const project = makeProject([makeModel('main', [makeModule('d', 'delay1')]), makeModel('delay1', [makeAux('x')])]);
    const result = getAvailableModels(project, 'main');
    expect(result.projectModels).toContain('delay1');
  });

  // macros.AC6.6: a macro-marked model is an ordinary project.models
  // entry after import, but it is a callable macro template -- never a
  // selectable module-reference target. It must be filtered out of the
  // model list so it does not appear in the module-reference dropdown.
  it('excludes macro-marked models but keeps ordinary submodels', () => {
    const macroSpec: MacroSpec = {
      parameters: ['input', 'parameter'],
      primaryOutput: 'expression_macro',
      additionalOutputs: [],
    };
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
      makeModel('hares', [makeAux('y')]),
      // A macro-marked model (e.g. an imported Vensim :MACRO: block) with
      // synthesized port variables. It must NOT be offered as a module
      // reference even though it is a normal entry in project.models.
      makeModel(
        'expression_macro',
        [makeAux('input', { canBeModuleInput: true }), makeAux('parameter', { canBeModuleInput: true })],
        macroSpec,
      ),
    ]);
    const result = getAvailableModels(project, 'main');
    expect(result.projectModels).toContain('hares');
    expect(result.projectModels).not.toContain('expression_macro');
    // And it is not misclassified into stdlibModels either.
    expect(result.stdlibModels).not.toContain('expression_macro');
  });

  it('still excludes the macro model when navigating from another model', () => {
    const macroSpec: MacroSpec = {
      parameters: ['a'],
      primaryOutput: 'm',
      additionalOutputs: [],
    };
    const project = makeProject([
      makeModel('main', [makeAux('x')]),
      makeModel('hares', [makeAux('y')]),
      makeModel('m', [makeAux('a', { canBeModuleInput: true })], macroSpec),
    ]);
    // From `hares`, `main` and the macro must both be unavailable as a
    // reference target (main would be fine here, but the macro never is).
    const result = getAvailableModels(project, 'hares');
    expect(result.projectModels).toContain('main');
    expect(result.projectModels).not.toContain('m');
  });
});

describe('isMacroModel', () => {
  it('returns true for a model with a macroSpec', () => {
    const macroSpec: MacroSpec = {
      parameters: ['x'],
      primaryOutput: 'm',
      additionalOutputs: [],
    };
    const model = makeModel('m', [makeAux('x', { canBeModuleInput: true })], macroSpec);
    expect(isMacroModel(model)).toBe(true);
  });

  it('returns false for an ordinary model with no macroSpec', () => {
    const model = makeModel('hares', [makeAux('y')]);
    expect(isMacroModel(model)).toBe(false);
  });

  it('returns false for a stdlib model (stdlib models are not macros)', () => {
    const model = makeModel('stdlib\u{205A}delay1', [makeAux('input', { canBeModuleInput: true })]);
    expect(isMacroModel(model)).toBe(false);
  });
});

// Regression test: stdlib modules (hiring model scenario) should have
// their model definitions accessible for input ports and public variables.
describe('stdlib module wiring (regression)', () => {
  it('can read input ports from a stdlib model in the project', () => {
    const project = makeProject([
      makeModel('main', [makeModule('outflows', 'stdlib\u{205A}systems_rate')]),
      makeModel('stdlib\u{205A}systems_rate', [
        makeAux('available', { canBeModuleInput: true }),
        makeAux('requested', { canBeModuleInput: true }),
        makeAux('dest_capacity', { canBeModuleInput: true }),
        makeAux('actual', { isPublic: true }),
      ]),
    ]);
    const stdlibModel = project.models.get('stdlib\u{205A}systems_rate')!;
    expect(stdlibModel).toBeDefined();

    const inputs = getInputPorts(stdlibModel);
    expect(inputs).toHaveLength(3);
    expect(inputs.map((v) => v.ident).sort()).toEqual(['available', 'dest_capacity', 'requested']);
  });

  it('can read public variables from a stdlib model in the project', () => {
    const project = makeProject([
      makeModel('main', [makeModule('outflows', 'stdlib\u{205A}systems_rate')]),
      makeModel('stdlib\u{205A}systems_rate', [
        makeAux('available', { canBeModuleInput: true }),
        makeAux('actual', { isPublic: true }),
      ]),
    ]);
    const stdlibModel = project.models.get('stdlib\u{205A}systems_rate')!;
    const pubs = getPublicVariables(stdlibModel);
    expect(pubs).toHaveLength(1);
    expect(pubs[0].ident).toBe('actual');
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
    const model = makeModel('hares', [makeAux('food'), makeAux('rate')]);
    const inputs = getInputPorts(model);
    expect(inputs).toHaveLength(0);
  });

  it('excludes module variables (modules cannot be input ports)', () => {
    const model = makeModel('hares', [makeAux('food', { canBeModuleInput: true }), makeModule('sub', 'sub_model')]);
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
    const model = makeModel('hares', [makeAux('food'), makeStock('level')]);
    const pubs = getPublicVariables(model);
    expect(pubs).toHaveLength(0);
  });

  it('excludes module variables', () => {
    const model = makeModel('hares', [makeAux('food', { isPublic: true }), makeModule('sub', 'sub_model')]);
    const pubs = getPublicVariables(model);
    expect(pubs).toHaveLength(1);
    expect(pubs[0].ident).toBe('food');
  });
});
