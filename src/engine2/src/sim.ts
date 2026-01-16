// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Sim class for step-by-step simulation control.
 *
 * Use Model.simulate() to create a Sim instance for gaming applications
 * where you need to inspect state and modify variables during simulation.
 * For batch analysis, use Model.run() instead.
 */

import {
  simlin_sim_new,
  simlin_sim_unref,
  simlin_sim_run_to,
  simlin_sim_run_to_end,
  simlin_sim_reset,
  simlin_sim_get_stepcount,
  simlin_sim_get_value,
  simlin_sim_set_value,
  simlin_sim_get_series,
} from './internal/sim';
import { simlin_model_get_var_names } from './internal/model';
import { simlin_analyze_get_links, simlin_free_links, readLinks } from './internal/analysis';
import { SimlinSimPtr, SimlinLinkPolarity, Link as LowLevelLink } from './internal/types';
import { Link, LinkPolarity } from './types';
import { registerFinalizer, unregisterFinalizer } from './internal/dispose';

/**
 * Convert low-level link polarity to high-level type with validation.
 */
function convertLinkPolarity(rawPolarity: SimlinLinkPolarity): LinkPolarity {
  switch (rawPolarity) {
    case SimlinLinkPolarity.Positive:
      return LinkPolarity.Positive;
    case SimlinLinkPolarity.Negative:
      return LinkPolarity.Negative;
    case SimlinLinkPolarity.Unknown:
      return LinkPolarity.Unknown;
    default:
      throw new Error(`Invalid link polarity value: ${rawPolarity}`);
  }
}
import { Model } from './model';
import { Run } from './run';

/**
 * A simulation context for step-by-step execution.
 *
 * Sim provides low-level control over simulation execution, allowing
 * you to run to specific times, inspect and modify variable values,
 * and get time series data.
 */
export class Sim {
  private _ptr: SimlinSimPtr;
  private _model: Model;
  private _overrides: Record<string, number>;
  private _disposed: boolean = false;
  private _enableLtm: boolean;

  /**
   * Create a Sim from a Model.
   * This is internal - use Model.simulate() instead.
   */
  constructor(model: Model, overrides: Record<string, number> = {}, enableLtm: boolean = false) {
    const ptr = simlin_sim_new(model.ptr, enableLtm);
    this._ptr = ptr;
    this._model = model;
    this._overrides = { ...overrides };
    this._enableLtm = enableLtm;
    registerFinalizer(this, ptr, simlin_sim_unref);

    // Apply any overrides
    for (const [name, value] of Object.entries(overrides)) {
      simlin_sim_set_value(ptr, name, value);
    }
  }

  /**
   * Get the internal WASM pointer. For internal use only.
   */
  get ptr(): SimlinSimPtr {
    this.checkDisposed();
    return this._ptr;
  }

  /**
   * The Model this simulation is based on.
   */
  get model(): Model {
    return this._model;
  }

  /**
   * The overrides applied to this simulation.
   */
  get overrides(): Record<string, number> {
    return { ...this._overrides };
  }

  /**
   * Whether LTM (Loops That Matter) analysis is enabled.
   */
  get ltmEnabled(): boolean {
    return this._enableLtm;
  }

  private checkDisposed(): void {
    if (this._disposed) {
      throw new Error('Sim has been disposed');
    }
  }

  /**
   * Get the current simulation time.
   */
  get time(): number {
    this.checkDisposed();
    return simlin_sim_get_value(this._ptr, 'time');
  }

  /**
   * Run the simulation to a specific time.
   * @param time Target time
   */
  runTo(time: number): void {
    this.checkDisposed();
    simlin_sim_run_to(this._ptr, time);
  }

  /**
   * Run the simulation to the end.
   */
  runToEnd(): void {
    this.checkDisposed();
    simlin_sim_run_to_end(this._ptr);
  }

  /**
   * Reset the simulation to initial state.
   */
  reset(): void {
    this.checkDisposed();
    simlin_sim_reset(this._ptr);

    // Re-apply overrides after reset
    for (const [name, value] of Object.entries(this._overrides)) {
      simlin_sim_set_value(this._ptr, name, value);
    }
  }

  /**
   * Get the number of simulation steps completed.
   */
  getStepCount(): number {
    this.checkDisposed();
    return simlin_sim_get_stepcount(this._ptr);
  }

  /**
   * Get the current value of a variable.
   * @param name Variable name
   * @returns Current value
   */
  getValue(name: string): number {
    this.checkDisposed();
    return simlin_sim_get_value(this._ptr, name);
  }

  /**
   * Set the value of a variable.
   * @param name Variable name
   * @param value New value
   */
  setValue(name: string, value: number): void {
    this.checkDisposed();
    simlin_sim_set_value(this._ptr, name, value);
  }

  /**
   * Get time series data for a variable.
   * @param name Variable name
   * @returns Float64Array with time series data
   */
  getSeries(name: string): Float64Array {
    this.checkDisposed();
    const stepCount = this.getStepCount();
    return simlin_sim_get_series(this._ptr, name, stepCount);
  }

  /**
   * Get variable names available in this simulation.
   * @returns Array of variable names
   */
  getVarNames(): string[] {
    this.checkDisposed();
    return simlin_model_get_var_names(this._model.ptr);
  }

  /**
   * Get causal links with LTM scores (if enabled).
   * @returns Array of Link objects
   */
  getLinks(): Link[] {
    this.checkDisposed();

    const linksPtr = simlin_analyze_get_links(this._ptr);
    if (linksPtr === 0) {
      return [];
    }

    const rawLinks = readLinks(linksPtr);
    simlin_free_links(linksPtr);

    return rawLinks.map((link: LowLevelLink) => ({
      from: link.from,
      to: link.to,
      polarity: convertLinkPolarity(link.polarity),
      score: link.score || undefined,
    }));
  }

  /**
   * Convert this simulation to a Run object.
   * @returns Run object with results and analysis
   */
  getRun(): Run {
    this.checkDisposed();
    return new Run(this);
  }

  /**
   * Dispose this simulation and free WASM resources.
   */
  dispose(): void {
    if (this._disposed) {
      return;
    }

    unregisterFinalizer(this);
    simlin_sim_unref(this._ptr);
    this._ptr = 0;
    this._disposed = true;
  }

  /**
   * Symbol.dispose support for using statement.
   */
  [Symbol.dispose](): void {
    this.dispose();
  }
}
