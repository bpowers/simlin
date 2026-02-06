// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Model class for working with system dynamics models.
 *
 * A Model contains variables, equations, and structure that define
 * the system dynamics simulation. Models can be simulated by creating
 * Sim instances.
 */

import { EngineBackend, ModelHandle } from './backend';
import { Stock, Flow, Aux, Variable, TimeSpec, Link, Loop, ModelIssue, GraphicalFunction } from './types';
import { JsonModel, JsonStock, JsonFlow, JsonAuxiliary, JsonGraphicalFunction, JsonProjectPatch } from './json-types';
import { Project } from './project';
import { Sim } from './sim';
import { Run } from './run';
import { ModelPatchBuilder } from './patch';

/**
 * Parse a DT string to a number.
 * Handles fractional notation like "1/4" or plain numbers like "0.25".
 */
function parseDt(dt: string): number {
  if (!dt || dt.trim() === '') {
    return 1;
  }

  const trimmed = dt.trim();

  // Check for fraction notation
  if (trimmed.includes('/')) {
    const parts = trimmed.split('/');
    if (parts.length === 2) {
      const numerator = parseFloat(parts[0]);
      const denominator = parseFloat(parts[1]);
      if (!isNaN(numerator) && !isNaN(denominator) && denominator !== 0) {
        return numerator / denominator;
      }
    }
  }

  const value = parseFloat(trimmed);
  return isNaN(value) ? 1 : value;
}

/**
 * A system dynamics model.
 *
 * Models are obtained from Project.getModel() or Project.mainModel().
 * They provide access to variables, structure, and simulation capabilities.
 */
export class Model {
  private _handle: ModelHandle;
  private _project: Project | null;
  private _name: string | null;
  private _disposed: boolean = false;

  // Cached data
  private _cachedModelJson: JsonModel | null = null;
  private _cachedStocks: Stock[] | null = null;
  private _cachedFlows: Flow[] | null = null;
  private _cachedAuxs: Aux[] | null = null;
  private _cachedTimeSpec: TimeSpec | null = null;
  private _cachedBaseCase: Run | null = null;
  private _cachedVariables: Variable[] | null = null;

  /** @internal */
  constructor(handle: ModelHandle, project: Project | null, name: string | null) {
    this._handle = handle;
    this._project = project;
    this._name = name;
  }

  /** @internal */
  get handle(): ModelHandle {
    this.checkDisposed();
    return this._handle;
  }

  /**
   * The Project this model belongs to.
   */
  get project(): Project | null {
    return this._project;
  }

  /**
   * The model name.
   */
  get name(): string | null {
    return this._name;
  }

  private get backend(): EngineBackend {
    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }
    return this._project.backend;
  }

  private checkDisposed(): void {
    if (this._disposed) {
      throw new Error('Model has been disposed');
    }
  }

  /**
   * Invalidate all cached data. Called after model edits.
   */
  invalidateCaches(): void {
    this._cachedModelJson = null;
    this._cachedStocks = null;
    this._cachedFlows = null;
    this._cachedAuxs = null;
    this._cachedTimeSpec = null;
    this._cachedBaseCase = null;
    this._cachedVariables = null;
  }

  private async getModelJson(): Promise<JsonModel> {
    if (this._cachedModelJson !== null) {
      return this._cachedModelJson;
    }

    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const projectJson = JSON.parse(await this._project.serializeJson());
    for (const modelDict of projectJson.models || []) {
      if (modelDict.name === this._name || !this._name) {
        this._cachedModelJson = modelDict as JsonModel;
        return this._cachedModelJson;
      }
    }

    throw new Error(`Model '${this._name}' not found in project`);
  }

  private extractEquation(
    topLevel: string | undefined,
    arrayed: { equation?: string; initialEquation?: string } | undefined,
    field: 'equation' | 'initialEquation' = 'equation',
  ): string {
    if (topLevel) {
      return topLevel;
    }
    if (arrayed) {
      const value = arrayed[field];
      if (value) {
        return value;
      }
    }
    return '';
  }

  private parseJsonGraphicalFunction(gf: JsonGraphicalFunction): GraphicalFunction {
    let points: [number, number][] | undefined;
    let yPoints: number[] | undefined;

    if (gf.points && gf.points.length > 0) {
      points = gf.points;
    } else if (gf.yPoints && gf.yPoints.length > 0) {
      yPoints = gf.yPoints;
    }

    return {
      points,
      yPoints,
      xScale: gf.xScale ? { min: gf.xScale.min, max: gf.xScale.max } : undefined,
      yScale: gf.yScale ? { min: gf.yScale.min, max: gf.yScale.max } : undefined,
      kind: gf.kind,
    };
  }

  /**
   * Stock variables in the model.
   */
  async stocks(): Promise<readonly Stock[]> {
    this.checkDisposed();
    if (this._cachedStocks !== null) {
      return this._cachedStocks;
    }

    const model = await this.getModelJson();
    this._cachedStocks = (model.stocks || []).map((s: JsonStock) => ({
      type: 'stock' as const,
      name: s.name,
      initialEquation: this.extractEquation(s.initialEquation, s.arrayedEquation, 'initialEquation'),
      inflows: s.inflows || [],
      outflows: s.outflows || [],
      units: s.units || undefined,
      documentation: s.documentation || undefined,
      dimensions: s.arrayedEquation?.dimensions || [],
      nonNegative: s.nonNegative || false,
    }));

    return this._cachedStocks;
  }

  /**
   * Flow variables in the model.
   */
  async flows(): Promise<readonly Flow[]> {
    this.checkDisposed();
    if (this._cachedFlows !== null) {
      return this._cachedFlows;
    }

    const model = await this.getModelJson();
    this._cachedFlows = (model.flows || []).map((f: JsonFlow) => {
      let gf: GraphicalFunction | undefined;
      if (f.graphicalFunction) {
        gf = this.parseJsonGraphicalFunction(f.graphicalFunction);
      }

      return {
        type: 'flow' as const,
        name: f.name,
        equation: this.extractEquation(f.equation, f.arrayedEquation),
        units: f.units || undefined,
        documentation: f.documentation || undefined,
        dimensions: f.arrayedEquation?.dimensions || [],
        nonNegative: f.nonNegative || false,
        graphicalFunction: gf,
      };
    });

    return this._cachedFlows;
  }

  /**
   * Auxiliary variables in the model.
   */
  async auxs(): Promise<readonly Aux[]> {
    this.checkDisposed();
    if (this._cachedAuxs !== null) {
      return this._cachedAuxs;
    }

    const model = await this.getModelJson();
    this._cachedAuxs = (model.auxiliaries || []).map((a: JsonAuxiliary) => {
      let gf: GraphicalFunction | undefined;
      if (a.graphicalFunction) {
        gf = this.parseJsonGraphicalFunction(a.graphicalFunction);
      }

      const equation = this.extractEquation(a.equation, a.arrayedEquation);
      const initialEquation = this.extractEquation(a.initialEquation, a.arrayedEquation, 'initialEquation');

      return {
        type: 'aux' as const,
        name: a.name,
        equation,
        initialEquation: initialEquation || undefined,
        units: a.units || undefined,
        documentation: a.documentation || undefined,
        dimensions: a.arrayedEquation?.dimensions || [],
        graphicalFunction: gf,
      };
    });

    return this._cachedAuxs;
  }

  /**
   * All variables in the model (stocks + flows + auxs).
   */
  async variables(): Promise<readonly Variable[]> {
    this.checkDisposed();
    if (this._cachedVariables !== null) {
      return this._cachedVariables;
    }

    this._cachedVariables = [...(await this.stocks()), ...(await this.flows()), ...(await this.auxs())];
    return this._cachedVariables;
  }

  /**
   * Time specification for simulation.
   * Uses model-level sim_specs if present, otherwise falls back to project-level.
   */
  async timeSpec(): Promise<TimeSpec> {
    this.checkDisposed();
    if (this._cachedTimeSpec !== null) {
      return this._cachedTimeSpec;
    }

    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const projectJson = JSON.parse(await this._project.serializeJson());
    const modelJson = await this.getModelJson();

    // Use model-level sim_specs if present, otherwise fall back to project-level
    const simSpecs = modelJson.simSpecs ?? projectJson.simSpecs;

    this._cachedTimeSpec = {
      start: simSpecs.startTime ?? 0,
      stop: simSpecs.endTime ?? 10,
      dt: parseDt(simSpecs.dt ?? '1'),
      units: simSpecs.timeUnits || undefined,
    };

    return this._cachedTimeSpec;
  }

  /**
   * Structural feedback loops (no behavior data).
   */
  async loops(): Promise<readonly Loop[]> {
    this.checkDisposed();
    if (this._project === null) {
      return [];
    }
    return this._project.getLoops();
  }

  /**
   * Get the dependencies (incoming links) for a given variable.
   * @param varName The name of the variable to query
   * @returns List of variable names that this variable depends on
   */
  async getIncomingLinks(varName: string): Promise<string[]> {
    this.checkDisposed();

    // Validate variable exists
    const vars = await this.variables();
    const varNames = vars.map((v) => v.name);
    if (!varNames.includes(varName)) {
      throw new Error(`Variable not found: ${varName}`);
    }

    return this.backend.modelGetIncomingLinks(this._handle, varName);
  }

  /**
   * Get all causal links in the model (static analysis).
   * @returns List of Link objects representing causal relationships
   */
  async getLinks(): Promise<Link[]> {
    this.checkDisposed();
    return this.backend.modelGetLinks(this._handle);
  }

  /**
   * Get human-readable explanation of a variable.
   * @param variable Variable name
   * @returns Textual description of what defines/drives this variable
   */
  async explain(variable: string): Promise<string> {
    this.checkDisposed();

    for (const stock of await this.stocks()) {
      if (stock.name === variable) {
        const inflowsStr = stock.inflows.length > 0 ? stock.inflows.join(', ') : 'no inflows';
        const outflowsStr = stock.outflows.length > 0 ? stock.outflows.join(', ') : 'no outflows';
        return `${stock.name} is a stock with initial value ${stock.initialEquation}, increased by ${inflowsStr}, decreased by ${outflowsStr}`;
      }
    }

    for (const flow of await this.flows()) {
      if (flow.name === variable) {
        return `${flow.name} is a flow computed as ${flow.equation}`;
      }
    }

    for (const aux of await this.auxs()) {
      if (aux.name === variable) {
        if (aux.initialEquation) {
          return `${aux.name} is an auxiliary variable computed as ${aux.equation} with initial value ${aux.initialEquation}`;
        }
        return `${aux.name} is an auxiliary variable computed as ${aux.equation}`;
      }
    }

    throw new Error(`Variable '${variable}' not found in model`);
  }

  /**
   * Get the LaTeX representation of a variable's equation.
   * @param ident Variable identifier
   * @returns LaTeX string, or null if not found
   */
  async getLatexEquation(ident: string): Promise<string | null> {
    this.checkDisposed();
    return this.backend.modelGetLatexEquation(this._handle, ident);
  }

  /**
   * Check model for common issues.
   * @returns Array of ModelIssue objects, or empty array if no issues
   */
  async check(): Promise<ModelIssue[]> {
    this.checkDisposed();
    if (this._project === null) {
      return [];
    }

    const errorDetails = await this._project.getErrors();

    // Get the actual model name from JSON for comparison
    // (handles case where _name is null for main model)
    const modelJson = await this.getModelJson();
    const actualModelName = modelJson.name;

    // Filter to errors for this model only
    const modelErrors = errorDetails.filter((detail) => {
      if (!detail.modelName) {
        return false;
      }
      return detail.modelName === actualModelName;
    });

    return modelErrors.map((detail) => ({
      severity: 'error' as const,
      message: detail.message || 'Unknown error',
      variable: detail.variableName || undefined,
      suggestion: undefined,
    }));
  }

  /**
   * Create low-level simulation for step-by-step execution.
   * @param overrides Variable value overrides
   * @param options Simulation options
   * @returns Sim instance for step-by-step execution
   */
  async simulate(overrides: Record<string, number> = {}, options: { enableLtm?: boolean } = {}): Promise<Sim> {
    this.checkDisposed();
    const { enableLtm = false } = options;
    return new Sim(this, overrides, enableLtm);
  }

  /**
   * Run simulation with optional variable overrides.
   * @param overrides Override values for any model variables
   * @param options Run options
   * @returns Run object with results and analysis
   */
  async run(overrides: Record<string, number> = {}, options: { analyzeLtm?: boolean } = {}): Promise<Run> {
    this.checkDisposed();
    const { analyzeLtm = true } = options;

    const sim = await this.simulate(overrides, { enableLtm: analyzeLtm });
    await sim.runToEnd();

    return await sim.getRun();
  }

  /**
   * Simulation results with default parameters (cached).
   */
  async baseCase(): Promise<Run> {
    this.checkDisposed();
    if (this._cachedBaseCase === null) {
      this._cachedBaseCase = await this.run();
    }
    return this._cachedBaseCase;
  }

  /**
   * Edit the model using a callback with patch builder.
   * @param callback Function that receives current variables and a patch builder
   * @param options Edit options (dryRun, allowErrors)
   */
  async edit(
    callback: (currentVars: Record<string, JsonStock | JsonFlow | JsonAuxiliary>, patch: ModelPatchBuilder) => void,
    options: { dryRun?: boolean; allowErrors?: boolean } = {},
  ): Promise<void> {
    this.checkDisposed();
    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const { dryRun = false, allowErrors = false } = options;

    // Get current model state as JSON
    const modelJson = await this.getModelJson();
    const modelName = modelJson.name;

    // Build current variables map
    const currentVars: Record<string, JsonStock | JsonFlow | JsonAuxiliary> = {};
    for (const stock of modelJson.stocks || []) {
      currentVars[stock.name] = stock;
    }
    for (const flow of modelJson.flows || []) {
      currentVars[flow.name] = flow;
    }
    for (const aux of modelJson.auxiliaries || []) {
      currentVars[aux.name] = aux;
    }

    // Create patch builder
    const patch = new ModelPatchBuilder(modelName);

    // Call user callback
    callback(currentVars, patch);

    // If no operations, return early
    if (!patch.hasOperations()) {
      return;
    }

    // Build and apply the patch
    const projectPatch: JsonProjectPatch = {
      models: [patch.build()],
    };

    await this._project.applyPatch(projectPatch, { dryRun, allowErrors });

    // Invalidate caches if not dry run
    if (!dryRun) {
      this.invalidateCaches();
    }
  }

  /**
   * Dispose this model and free WASM resources.
   */
  async dispose(): Promise<void> {
    this.disposeSync();
  }

  /** @internal Synchronous dispose for Symbol.dispose and internal use */
  disposeSync(): void {
    if (this._disposed) {
      return;
    }

    this.backend.modelDispose(this._handle);
    this._disposed = true;
  }

  /**
   * Symbol.dispose support for using statement.
   */
  [Symbol.dispose](): void {
    this.disposeSync();
  }
}
