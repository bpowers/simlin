// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Run class for completed simulation results.
 *
 * A Run is a pure data holder containing the results of a completed
 * simulation, including time series data for all variables, overrides
 * used, and loop/link analysis. It has no WASM references.
 */

import { Link, Loop } from './types';

/**
 * Data needed to construct a Run.
 */
export interface RunData {
  varNames: string[];
  results: Map<string, Float64Array>;
  loops: readonly Loop[];
  links: readonly Link[];
  stepCount: number;
  overrides: Record<string, number>;
}

/**
 * Results of a completed simulation run.
 *
 * Run is a pure data holder with no WASM access. All data is
 * pre-collected during Sim.getRun() or Model.run().
 */
export class Run {
  private _varNames: string[];
  private _results: Map<string, Float64Array>;
  private _loops: readonly Loop[];
  private _links: readonly Link[];
  private _stepCount: number;
  private _overrides: Record<string, number>;

  /**
   * Create a Run from pre-collected simulation data.
   * This is internal - use Sim.getRun() or Model.run() instead.
   */
  constructor(data: RunData) {
    this._varNames = data.varNames;
    this._results = data.results;
    this._loops = data.loops;
    this._links = data.links;
    this._stepCount = data.stepCount;
    this._overrides = { ...data.overrides };
  }

  /**
   * The overrides applied to this simulation run.
   */
  get overrides(): Record<string, number> {
    return { ...this._overrides };
  }

  /**
   * Variable names in this run.
   */
  get varNames(): readonly string[] {
    return this._varNames;
  }

  /**
   * Get the time series for all variables.
   * @returns Map of variable name to Float64Array
   */
  get results(): ReadonlyMap<string, Float64Array> {
    return this._results;
  }

  /**
   * Get the time series for a specific variable.
   * @param name Variable name
   * @returns Float64Array with time series data
   */
  getSeries(name: string): Float64Array {
    const series = this._results.get(name);
    if (!series) {
      throw new Error(`Variable '${name}' not found in run results`);
    }
    return series;
  }

  /**
   * Get the time array.
   */
  get time(): Float64Array {
    return this.getSeries('time');
  }

  /**
   * Get feedback loops with behavior data.
   */
  get loops(): readonly Loop[] {
    return this._loops;
  }

  /**
   * Get causal links, potentially with LTM scores.
   */
  get links(): readonly Link[] {
    return this._links;
  }

  /**
   * Get the number of simulation steps.
   */
  get stepCount(): number {
    return this._stepCount;
  }
}
