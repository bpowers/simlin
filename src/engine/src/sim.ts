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

import { EngineBackend, SimHandle } from './backend';
import { Link } from './types';
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
  private _handle: SimHandle;
  private _model: Model;
  private _overrides: Record<string, number>;
  private _disposed: boolean = false;
  private _enableLtm: boolean;

  /** @internal Use Sim.create() instead. */
  private constructor(handle: SimHandle, model: Model, overrides: Record<string, number>, enableLtm: boolean) {
    this._handle = handle;
    this._model = model;
    this._overrides = { ...overrides };
    this._enableLtm = enableLtm;
  }

  /**
   * Create a Sim from a Model.
   * This is internal - use Model.simulate() instead.
   */
  static async create(model: Model, overrides: Record<string, number> = {}, enableLtm: boolean = false): Promise<Sim> {
    if (model.project === null) {
      throw new Error('Model is not attached to a Project');
    }
    const backend = model.project.backend;
    const handle = await backend.simNew(model.handle, enableLtm);

    // Apply any overrides
    for (const [name, value] of Object.entries(overrides)) {
      await backend.simSetValue(handle, name, value);
    }

    return new Sim(handle, model, overrides, enableLtm);
  }

  /** @internal */
  get handle(): SimHandle {
    this.checkDisposed();
    return this._handle;
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

  private get backend(): EngineBackend {
    if (this._model.project === null) {
      throw new Error('Model is not attached to a Project');
    }
    return this._model.project.backend;
  }

  private checkDisposed(): void {
    if (this._disposed) {
      throw new Error('Sim has been disposed');
    }
  }

  /**
   * Get the current simulation time.
   */
  async time(): Promise<number> {
    this.checkDisposed();
    return await this.backend.simGetTime(this._handle);
  }

  /**
   * Run the simulation to a specific time.
   * @param time Target time
   */
  async runTo(time: number): Promise<void> {
    this.checkDisposed();
    await this.backend.simRunTo(this._handle, time);
  }

  /**
   * Run the simulation to the end.
   */
  async runToEnd(): Promise<void> {
    this.checkDisposed();
    await this.backend.simRunToEnd(this._handle);
  }

  /**
   * Reset the simulation to initial state.
   */
  async reset(): Promise<void> {
    this.checkDisposed();
    await this.backend.simReset(this._handle);

    // Re-apply overrides after reset
    for (const [name, value] of Object.entries(this._overrides)) {
      await this.backend.simSetValue(this._handle, name, value);
    }
  }

  /**
   * Get the number of simulation steps completed.
   */
  async getStepCount(): Promise<number> {
    this.checkDisposed();
    return await this.backend.simGetStepCount(this._handle);
  }

  /**
   * Get the current value of a variable.
   * @param name Variable name
   * @returns Current value
   */
  async getValue(name: string): Promise<number> {
    this.checkDisposed();
    return await this.backend.simGetValue(this._handle, name);
  }

  /**
   * Set the value of a variable.
   * @param name Variable name
   * @param value New value
   */
  async setValue(name: string, value: number): Promise<void> {
    this.checkDisposed();
    await this.backend.simSetValue(this._handle, name, value);
  }

  /**
   * Get time series data for a variable.
   * @param name Variable name
   * @returns Float64Array with time series data
   */
  async getSeries(name: string): Promise<Float64Array> {
    this.checkDisposed();
    return await this.backend.simGetSeries(this._handle, name);
  }

  /**
   * Get variable names available in this simulation.
   * @returns Array of variable names
   */
  async getVarNames(): Promise<string[]> {
    this.checkDisposed();
    return await this.backend.simGetVarNames(this._handle);
  }

  /**
   * Get causal links with LTM scores (if enabled).
   * @returns Array of Link objects
   */
  async getLinks(): Promise<Link[]> {
    this.checkDisposed();
    return await this.backend.simGetLinks(this._handle);
  }

  /**
   * Convert this simulation to a Run object.
   * Collects all data from the simulation into a pure data holder.
   * @returns Run object with results and analysis
   */
  async getRun(): Promise<Run> {
    this.checkDisposed();

    const varNames = await this.getVarNames();

    // Fetch all series in parallel to avoid sequential round-trips
    // through the FIFO queue when using WorkerBackend.
    const allNames = varNames.includes('time') ? varNames : [...varNames, 'time'];
    const seriesArrays = await Promise.all(allNames.map((name) => this.getSeries(name)));
    const results = new Map<string, Float64Array>();
    for (let i = 0; i < allNames.length; i++) {
      results.set(allNames[i], seriesArrays[i]);
    }

    const [loops, links, stepCount] = await Promise.all([this._model.loops(), this.getLinks(), this.getStepCount()]);

    return new Run({
      varNames,
      results,
      loops,
      links,
      stepCount,
      overrides: this.overrides,
    });
  }

  /**
   * Dispose this simulation and free WASM resources.
   */
  async dispose(): Promise<void> {
    if (this._disposed) {
      return;
    }

    await this.backend.simDispose(this._handle);
    this._disposed = true;
  }

  /**
   * Symbol.dispose support for using statement.
   * Fire-and-forget for async backends.
   */
  [Symbol.dispose](): void {
    if (this._disposed) {
      return;
    }

    const result = this.backend.simDispose(this._handle);
    if (result instanceof Promise) {
      result.catch((e) => console.warn('Sim dispose failed:', e));
    }
    this._disposed = true;
  }
}
