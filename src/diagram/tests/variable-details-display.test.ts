// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { ErrorCode } from '@simlin/core/datamodel';
import type {
  ArrayedElement,
  ArrayedEquation,
  Aux,
  Equation,
  EquationError,
  Module,
  Stock,
  UnitError,
  Variable,
  VariableWarning,
} from '@simlin/core/datamodel';

import { elementEquations, equationFieldText, equationNote, variableDetailsView } from '../variable-details-display';

function aux(overrides: Partial<Aux> = {}): Variable {
  return {
    type: 'aux',
    ident: 'x',
    equation: { type: 'scalar', equation: '1' },
    documentation: '',
    units: '',
    gf: undefined,
    canBeModuleInput: false,
    isPublic: false,
    activeInitial: undefined,
    dataSource: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: 1,
    ...overrides,
  };
}

const equationError: EquationError = { code: ErrorCode.EmptyEquation, start: 0, end: 0 };
const unitError: UnitError = {
  code: ErrorCode.BadTable,
  start: 0,
  end: 0,
  kind: 'consistency',
  details: 'dimensions are not equal',
};

describe('variableDetailsView', () => {
  it('shows the chart when the variable has no errors', () => {
    expect(variableDetailsView(aux())).toEqual({
      showChart: true,
      equationErrors: [],
      unitWarnings: [],
      connectorWarnings: [],
      warnings: [],
    });
  });

  it('keeps the chart and surfaces engine advisories as warnings (not fatal)', () => {
    const advisory: VariableWarning = {
      code: ErrorCode.Generic,
      details: "stock 'level': its inflow list repeats 'f'",
    };
    const view = variableDetailsView(aux({ warnings: [advisory] }));
    expect(view.showChart).toBe(true);
    expect(view.warnings).toEqual([advisory]);
    expect(view.equationErrors).toEqual([]);
  });

  it('surfaces connector-sync drift as non-fatal warnings (chart stays)', () => {
    const view = variableDetailsView(aux({ connectorErrors: [{ kind: 'missingConnector', ident: 'a', name: 'a' }] }));
    expect(view.showChart).toBe(true);
    expect(view.connectorWarnings).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
  });

  it('keeps the chart and surfaces unit errors as warnings (not fatal)', () => {
    const view = variableDetailsView(aux({ unitErrors: [unitError] }));
    expect(view.showChart).toBe(true);
    expect(view.unitWarnings).toEqual([unitError]);
    expect(view.equationErrors).toEqual([]);
  });

  it('replaces the chart with equation/compile errors (no valid data)', () => {
    const view = variableDetailsView(aux({ errors: [equationError] }));
    expect(view.showChart).toBe(false);
    expect(view.equationErrors).toEqual([equationError]);
  });

  it('prefers equation errors over the chart even when unit errors also exist', () => {
    const view = variableDetailsView(aux({ errors: [equationError], unitErrors: [unitError] }));
    expect(view.showChart).toBe(false);
    expect(view.equationErrors).toEqual([equationError]);
    expect(view.unitWarnings).toEqual([unitError]);
  });
});

const element = (equation: string, overrides: Partial<ArrayedElement> = {}): ArrayedElement => ({
  equation,
  graphicalFunction: undefined,
  activeInitial: undefined,
  ...overrides,
});

function perElement(overrides: Partial<ArrayedEquation> = {}): ArrayedEquation {
  return {
    type: 'arrayed',
    dimensionNames: ['Region'],
    elements: new Map([
      ['north', element('1')],
      ['south', element('a + b')],
    ]),
    defaultEquation: undefined,
    hasExceptDefault: false,
    ...overrides,
  };
}

const module: Module = {
  type: 'module',
  ident: 'm',
  modelName: 'child',
  documentation: '',
  units: '',
  references: [],
  canBeModuleInput: false,
  isPublic: false,
  dataSource: undefined,
  data: undefined,
  errors: undefined,
  unitErrors: undefined,
  uid: 1,
};

// Every arm of Equation, plus a module, which has none.
describe('equationFieldText', () => {
  const rows: Array<[string, Variable, string]> = [
    ['a scalar', aux({ equation: { type: 'scalar', equation: 'a + b' } }), 'a + b'],
    [
      'an apply-to-all variable',
      aux({ equation: { type: 'applyToAll', dimensionNames: ['Region'], equation: 'a + b' } }),
      'a + b',
    ],
    ['a per-element variable', aux({ equation: perElement() }), ''],
    ['a per-element variable with a default', aux({ equation: perElement({ defaultEquation: '7' }) }), ''],
    ['a module', module, ''],
  ];
  it.each(rows)("%s: the one equation's text, or nothing", (_name, variable, text) => {
    expect(equationFieldText(variable)).toBe(text);
  });
});

describe('equationNote', () => {
  const applyToAll = (...dimensionNames: string[]): Equation => ({ type: 'applyToAll', dimensionNames, equation: '1' });
  const stock = { ...aux(), type: 'stock', inflows: [], outflows: [], nonNegative: false } as unknown as Stock;
  const rows: Array<[string, Variable, string | undefined]> = [
    ['a scalar', aux(), undefined],
    ['a module', module, undefined],
    ['one dimension', aux({ equation: applyToAll('Region') }), 'Each element of Region'],
    ['two dimensions', aux({ equation: applyToAll('Region', 'Age') }), 'Each combination of Region and Age'],
    [
      'three dimensions',
      aux({ equation: applyToAll('Region', 'Age', 'Sex') }),
      'Each combination of Region, Age and Sex',
    ],
    [
      'per-element equations',
      aux({ equation: perElement() }),
      "The equations for each element of Region can't be edited here yet.",
    ],
    [
      'per-element equations over two dimensions',
      aux({ equation: perElement({ dimensionNames: ['Region', 'Age'] }) }),
      "The equations for each combination of Region and Age can't be edited here yet.",
    ],
    ["an apply-to-all stock's initial value", { ...stock, equation: applyToAll('Region') }, 'Each element of Region'],
    [
      "a stock's per-element initial values",
      { ...stock, equation: perElement() },
      "The initial values for each element of Region can't be edited here yet.",
    ],
  ];
  it.each(rows)('%s', (_name, variable, note) => {
    expect(equationNote(variable)).toBe(note);
  });
});

describe('elementEquations', () => {
  const table = {
    kind: 'continuous' as const,
    xPoints: undefined,
    yPoints: [0, 1],
    xScale: { min: 0, max: 1 },
    yScale: { min: 0, max: 1 },
  };

  it('lists each element in stored order', () => {
    expect(elementEquations(perElement())).toEqual([
      { element: 'north', equation: '1' },
      { element: 'south', equation: 'a + b' },
    ]);
  });

  it('lists an EXCEPT default that applies, after the elements', () => {
    const equation = perElement({ defaultEquation: '7', hasExceptDefault: true });
    expect(elementEquations(equation).slice(2)).toEqual([{ element: 'all others', equation: '7' }]);
  });

  it('does not list a default that is not in effect', () => {
    expect(elementEquations(perElement({ defaultEquation: '7', hasExceptDefault: false }))).toHaveLength(2);
    expect(elementEquations(perElement({ defaultEquation: undefined, hasExceptDefault: true }))).toHaveLength(2);
  });

  it('says an element with only a lookup has one, and shows the input of an element with both', () => {
    const elements = new Map([
      ['only', element('', { graphicalFunction: table })],
      ['both', element('TIME', { graphicalFunction: table })],
      ['neither', element('')],
    ]);
    expect(elementEquations(perElement({ elements }))).toEqual([
      { element: 'only', equation: '(lookup table)' },
      { element: 'both', equation: 'TIME' },
      { element: 'neither', equation: '' },
    ]);
  });
});
