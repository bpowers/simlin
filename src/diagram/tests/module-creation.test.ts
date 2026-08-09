// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core (tests for pure functions)

import { describe, it, expect } from '@rstest/core';

import type { Variable, Module as ModuleVar, Stock, Aux, Flow } from '@simlin/core/datamodel';

import { anyModuleHasModelReference } from '../module-warning';

// -- Test helpers --

function makeModuleVar(ident: string, modelName: string): ModuleVar {
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

function makeStockVar(ident: string): Stock {
  return {
    type: 'stock',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    inflows: [],
    outflows: [],
    nonNegative: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
  };
}

function makeAuxVar(ident: string): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    gf: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
  };
}

function makeFlowVar(ident: string): Flow {
  return {
    type: 'flow',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    gf: undefined,
    nonNegative: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
  };
}

// -- anyModuleHasModelReference tests --

describe('anyModuleHasModelReference', () => {
  it('returns false for an empty variables map', () => {
    const variables = new Map<string, Variable>();
    expect(anyModuleHasModelReference(variables)).toBe(false);
  });

  it('returns false when map contains only non-module variables', () => {
    const variables = new Map<string, Variable>([
      ['population', makeStockVar('population')],
      ['growth_rate', makeAuxVar('growth_rate')],
      ['births', makeFlowVar('births')],
    ]);
    expect(anyModuleHasModelReference(variables)).toBe(false);
  });

  it('returns false when one module has empty modelName', () => {
    const variables = new Map<string, Variable>([['hare_population', makeModuleVar('hare_population', '')]]);
    expect(anyModuleHasModelReference(variables)).toBe(false);
  });

  it('returns false when two modules both have empty modelName', () => {
    const variables = new Map<string, Variable>([
      ['hare_population', makeModuleVar('hare_population', '')],
      ['lynx_population', makeModuleVar('lynx_population', '')],
    ]);
    expect(anyModuleHasModelReference(variables)).toBe(false);
  });

  it('returns true when one module has a non-empty modelName', () => {
    const variables = new Map<string, Variable>([['hare_population', makeModuleVar('hare_population', 'hares')]]);
    expect(anyModuleHasModelReference(variables)).toBe(true);
  });

  it('returns true when one module is configured and another is not', () => {
    const variables = new Map<string, Variable>([
      ['hare_population', makeModuleVar('hare_population', 'hares')],
      ['lynx_population', makeModuleVar('lynx_population', '')],
    ]);
    expect(anyModuleHasModelReference(variables)).toBe(true);
  });

  it('returns true even with non-module variables mixed in', () => {
    const variables = new Map<string, Variable>([
      ['population', makeStockVar('population')],
      ['hare_population', makeModuleVar('hare_population', 'hares')],
      ['growth_rate', makeAuxVar('growth_rate')],
    ]);
    expect(anyModuleHasModelReference(variables)).toBe(true);
  });
});
