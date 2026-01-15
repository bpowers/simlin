// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Run class for completed simulation results.
 *
 * A Run contains the results of a completed simulation, including
 * time series data for all variables, overrides used, and loop analysis.
 */

import { Link, Loop } from './types';
import { Sim } from './sim';

/**
 * Results of a completed simulation run.
 *
 * Run provides access to simulation results as a Map of variable names
 * to Float64Array time series, plus metadata about the simulation.
 */
export class Run {
  private _sim: Sim;
  private _cachedResults: Map<string, Float64Array> | null = null;
  private _cachedVarNames: string[] | null = null;

  /**
   * Create a Run from a completed Sim.
   * This is internal - use Sim.getRun() or Model.run() instead.
   */
  constructor(sim: Sim) {
    this._sim = sim;
  }

  /**
   * The overrides applied to this simulation run.
   */
  get overrides(): Record<string, number> {
    return this._sim.overrides;
  }

  /**
   * Variable names in this run.
   */
  get varNames(): string[] {
    if (this._cachedVarNames === null) {
      this._cachedVarNames = this._sim.getVarNames();
    }
    return this._cachedVarNames;
  }

  /**
   * Get the time series for all variables.
   * @returns Map of variable name to Float64Array
   */
  get results(): Map<string, Float64Array> {
    if (this._cachedResults !== null) {
      return this._cachedResults;
    }

    const results = new Map<string, Float64Array>();
    const varNames = this.varNames;

    for (const name of varNames) {
      const series = this._sim.getSeries(name);
      results.set(name, series);
    }

    // Add time series if not already present
    if (!results.has('time')) {
      const timeSeries = this._sim.getSeries('time');
      results.set('time', timeSeries);
    }

    this._cachedResults = results;
    return results;
  }

  /**
   * Get the time series for a specific variable.
   * @param name Variable name
   * @returns Float64Array with time series data
   */
  getSeries(name: string): Float64Array {
    return this.results.get(name) ?? this._sim.getSeries(name);
  }

  /**
   * Get the time array.
   */
  get time(): Float64Array {
    return this.getSeries('time');
  }

  /**
   * Get feedback loops with behavior data.
   *
   * Returns the structural loops from the model. For loop scores,
   * use the links with LTM enabled.
   */
  get loops(): Loop[] {
    return this._sim.model.loops;
  }

  /**
   * Get causal links, potentially with LTM scores.
   */
  get links(): Link[] {
    return this._sim.getLinks();
  }

  /**
   * Get the number of simulation steps.
   */
  get stepCount(): number {
    return this._sim.getStepCount();
  }
}
