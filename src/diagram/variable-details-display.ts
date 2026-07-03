// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { ConnectorError, EquationError, UnitError, Variable } from '@simlin/core/datamodel';

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
}

/**
 * Decide what the variable-details panel shows for a variable. Unit errors are
 * non-fatal -- the variable still simulates and has data -- so they no longer
 * hide the chart; only genuine equation/compile errors (which leave the
 * variable with no valid data) replace it.
 */
export function variableDetailsView(variable: Variable): VariableDetailsView {
  const equationErrors = variable.errors ?? [];
  const unitWarnings = variable.unitErrors ?? [];
  const connectorWarnings = variable.connectorErrors ?? [];
  return {
    showChart: equationErrors.length === 0,
    equationErrors,
    unitWarnings,
    connectorWarnings,
  };
}
