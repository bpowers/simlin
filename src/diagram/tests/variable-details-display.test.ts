// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { ErrorCode } from '@simlin/core/datamodel';
import type { Aux, EquationError, UnitError, Variable } from '@simlin/core/datamodel';

import { variableDetailsView } from '../variable-details-display';

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
  isConsistencyError: true,
  details: 'dimensions are not equal',
};

describe('variableDetailsView', () => {
  it('shows the chart when the variable has no errors', () => {
    expect(variableDetailsView(aux())).toEqual({
      showChart: true,
      equationErrors: [],
      unitWarnings: [],
      connectorWarnings: [],
    });
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
