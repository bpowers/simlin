// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type {
  ArrayedEquation,
  ConnectorError,
  EquationError,
  UnitError,
  Variable,
  VariableWarning,
} from '@simlin/core/datamodel';

export interface VariableDetailsView {
  /**
   * Whether to render the results chart. False only when the variable has
   * equation/compile errors -- those mean it produced no valid data, so the
   * error list takes the chart's place.
   */
  readonly showChart: boolean;
  /** Equation/compile errors; rendered as the error list when showChart is false. */
  readonly equationErrors: readonly EquationError[];
  /** Non-fatal unit errors; surfaced as warnings beside the chart. */
  readonly unitWarnings: readonly UnitError[];
  /**
   * Non-fatal sketch-connector drift (connectors out of sync with the
   * equation); surfaced as warnings beside the chart, like unit warnings.
   */
  readonly connectorWarnings: readonly ConnectorError[];
  /** Engine advisories that are not unit errors; surfaced beside the chart too. */
  readonly warnings: readonly VariableWarning[];
}

/**
 * Decide what the variable-details panel shows for a variable. Unit errors,
 * connector drift and engine advisories are non-fatal -- the variable still
 * simulates and has data -- so they never hide the chart; only genuine
 * equation/compile errors (which leave the variable with no valid data)
 * replace it.
 */
export function variableDetailsView(variable: Variable): VariableDetailsView {
  const equationErrors = variable.errors ?? [];
  const unitWarnings = variable.unitErrors ?? [];
  const connectorWarnings = variable.connectorErrors ?? [];
  const warnings = variable.warnings ?? [];
  return {
    showChart: equationErrors.length === 0,
    equationErrors,
    unitWarnings,
    connectorWarnings,
    warnings,
  };
}

/**
 * The text of the panel's equation field: a scalar's equation (a stock's
 * initial value), or the one equation every element of an apply-to-all
 * variable follows. A per-element variable has no one text to edit, so its
 * panel has no field and lists `elementEquations` instead.
 */
export function equationFieldText(variable: Variable): string {
  return variable.type === 'module' || variable.equation.type === 'arrayed' ? '' : variable.equation.equation;
}

/**
 * The line that says which elements the equation below it is for; undefined for
 * a scalar.
 */
export function equationNote(variable: Variable): string | undefined {
  if (variable.type === 'module' || variable.equation.type === 'scalar') {
    return undefined;
  }
  const element = elementOf(variable.equation.dimensionNames);
  if (variable.equation.type === 'applyToAll') {
    return `Each ${element}`;
  }
  const what = variable.type === 'stock' ? 'initial values' : 'equations';
  return `The ${what} for each ${element} can't be edited here yet.`;
}

function elementOf(dimensionNames: readonly string[]): string {
  if (dimensionNames.length < 2) {
    return `element of ${dimensionNames.join('')}`;
  }
  const last = dimensionNames[dimensionNames.length - 1];
  return `combination of ${dimensionNames.slice(0, -1).join(', ')} and ${last}`;
}

/** One row of a per-element variable's equation list. */
export interface ElementEquationRow {
  readonly element: string;
  readonly equation: string;
}

/**
 * A per-element variable's equations as the panel lists them: each element that
 * has one of its own, in stored order, then the EXCEPT default the others
 * follow. A default that does not apply (`hasExceptDefault` false) is not in
 * effect -- elements with no equation evaluate to 0 -- so it is not listed.
 */
export function elementEquations(equation: ArrayedEquation): readonly ElementEquationRow[] {
  const rows: ElementEquationRow[] = Array.from(
    equation.elements,
    ([element, { equation: text, graphicalFunction }]) => ({
      element,
      equation: text.trim() === '' && graphicalFunction !== undefined ? '(lookup table)' : text,
    }),
  );
  if (equation.hasExceptDefault && equation.defaultEquation !== undefined) {
    rows.push({ element: 'all others', equation: equation.defaultEquation });
  }
  return rows;
}
