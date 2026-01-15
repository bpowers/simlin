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

import {
  simlin_model_ref,
  simlin_model_unref,
  simlin_model_get_incoming_links,
  simlin_model_get_links,
} from '../model';
import { readLinks, simlin_free_links } from '../analysis';
import { SimlinModelPtr, SimlinLinkPolarity, Link as LowLevelLink } from '../types';
import {
  Stock,
  Flow,
  Aux,
  Variable,
  TimeSpec,
  Link,
  Loop,
  LinkPolarity,
  ModelIssue,
  GraphicalFunction,
  GraphicalFunctionScale,
} from './types';
import { JsonModel, JsonStock, JsonFlow, JsonAuxiliary, JsonGraphicalFunction, JsonProjectPatch } from './json-types';
import { Project } from './project';
import { Sim } from './sim';
import { Run } from './run';
import { ModelPatchBuilder } from './patch';

/**
 * Convert low-level link polarity to high-level type with validation.
 * Validates that the polarity value is within expected range.
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
 * Models are obtained from Project.getModel() or Project.mainModel.
 * They provide access to variables, structure, and simulation capabilities.
 */
export class Model {
  private _ptr: SimlinModelPtr;
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

  /**
   * Create a Model from a WASM pointer.
   * This is internal - use Project.getModel() or Project.mainModel instead.
   */
  constructor(ptr: SimlinModelPtr, project: Project | null, name: string | null) {
    if (ptr === 0) {
      throw new Error('Cannot create Model from null pointer');
    }
    this._ptr = ptr;
    this._project = project;
    this._name = name;

    // Increment reference count since we're holding a reference
    simlin_model_ref(ptr);
  }

  /**
   * Get the internal WASM pointer. For internal use only.
   */
  get ptr(): SimlinModelPtr {
    this.checkDisposed();
    return this._ptr;
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

  private getModelJson(): JsonModel {
    if (this._cachedModelJson !== null) {
      return this._cachedModelJson;
    }

    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const projectJson = JSON.parse(this._project.serializeJson());
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
    arrayed: { equation?: string; initial_equation?: string } | undefined,
    field: 'equation' | 'initial_equation' = 'equation',
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
    let xPoints: number[] | undefined;
    let yPoints: number[];

    if (gf.points && gf.points.length > 0) {
      xPoints = gf.points.map((p) => p[0]);
      yPoints = gf.points.map((p) => p[1]);
    } else {
      yPoints = gf.y_points || [];
    }

    const xScale: GraphicalFunctionScale = {
      min: gf.x_scale?.min ?? 0,
      max: gf.x_scale?.max ?? (yPoints.length > 0 ? yPoints.length - 1 : 0),
    };

    const yScale: GraphicalFunctionScale = {
      min: gf.y_scale?.min ?? 0,
      max: gf.y_scale?.max ?? 1,
    };

    return {
      xPoints,
      yPoints,
      xScale,
      yScale,
      kind: gf.kind || 'continuous',
    };
  }

  /**
   * Stock variables in the model (immutable array).
   */
  get stocks(): Stock[] {
    this.checkDisposed();
    if (this._cachedStocks !== null) {
      return this._cachedStocks;
    }

    const model = this.getModelJson();
    this._cachedStocks = (model.stocks || []).map((s: JsonStock) => ({
      type: 'stock' as const,
      name: s.name,
      initialEquation: this.extractEquation(s.initial_equation, s.arrayed_equation, 'initial_equation'),
      inflows: s.inflows || [],
      outflows: s.outflows || [],
      units: s.units || undefined,
      documentation: s.documentation || undefined,
      dimensions: s.arrayed_equation?.dimensions || [],
      nonNegative: s.non_negative || false,
    }));

    return this._cachedStocks;
  }

  /**
   * Flow variables in the model (immutable array).
   */
  get flows(): Flow[] {
    this.checkDisposed();
    if (this._cachedFlows !== null) {
      return this._cachedFlows;
    }

    const model = this.getModelJson();
    this._cachedFlows = (model.flows || []).map((f: JsonFlow) => {
      let gf: GraphicalFunction | undefined;
      if (f.graphical_function) {
        gf = this.parseJsonGraphicalFunction(f.graphical_function);
      }

      return {
        type: 'flow' as const,
        name: f.name,
        equation: this.extractEquation(f.equation, f.arrayed_equation),
        units: f.units || undefined,
        documentation: f.documentation || undefined,
        dimensions: f.arrayed_equation?.dimensions || [],
        nonNegative: f.non_negative || false,
        graphicalFunction: gf,
      };
    });

    return this._cachedFlows;
  }

  /**
   * Auxiliary variables in the model (immutable array).
   */
  get auxs(): Aux[] {
    this.checkDisposed();
    if (this._cachedAuxs !== null) {
      return this._cachedAuxs;
    }

    const model = this.getModelJson();
    this._cachedAuxs = (model.auxiliaries || []).map((a: JsonAuxiliary) => {
      let gf: GraphicalFunction | undefined;
      if (a.graphical_function) {
        gf = this.parseJsonGraphicalFunction(a.graphical_function);
      }

      const equation = this.extractEquation(a.equation, a.arrayed_equation);
      const initialEquation = this.extractEquation(a.initial_equation, a.arrayed_equation, 'initial_equation');

      return {
        type: 'aux' as const,
        name: a.name,
        equation,
        initialEquation: initialEquation || undefined,
        units: a.units || undefined,
        documentation: a.documentation || undefined,
        dimensions: a.arrayed_equation?.dimensions || [],
        graphicalFunction: gf,
      };
    });

    return this._cachedAuxs;
  }

  /**
   * All variables in the model (stocks + flows + auxs).
   */
  get variables(): Variable[] {
    this.checkDisposed();
    if (this._cachedVariables !== null) {
      return this._cachedVariables;
    }

    this._cachedVariables = [...this.stocks, ...this.flows, ...this.auxs];
    return this._cachedVariables;
  }

  /**
   * Time specification for simulation.
   * Uses model-level sim_specs if present, otherwise falls back to project-level.
   */
  get timeSpec(): TimeSpec {
    this.checkDisposed();
    if (this._cachedTimeSpec !== null) {
      return this._cachedTimeSpec;
    }

    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const projectJson = JSON.parse(this._project.serializeJson());
    const modelJson = this.getModelJson();

    // Use model-level sim_specs if present, otherwise fall back to project-level
    const simSpecs = modelJson.sim_specs ?? projectJson.sim_specs;

    this._cachedTimeSpec = {
      start: simSpecs.start_time ?? 0,
      stop: simSpecs.end_time ?? 10,
      dt: parseDt(simSpecs.dt ?? '1'),
      units: simSpecs.time_units || undefined,
    };

    return this._cachedTimeSpec;
  }

  /**
   * Structural feedback loops (no behavior data).
   */
  get loops(): Loop[] {
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
  getIncomingLinks(varName: string): string[] {
    this.checkDisposed();

    // Validate variable exists
    const varNames = this.variables.map((v) => v.name);
    if (!varNames.includes(varName)) {
      throw new Error(`Variable not found: ${varName}`);
    }

    return simlin_model_get_incoming_links(this._ptr, varName);
  }

  /**
   * Get all causal links in the model (static analysis).
   * @returns List of Link objects representing causal relationships
   */
  getLinks(): Link[] {
    this.checkDisposed();

    const linksPtr = simlin_model_get_links(this._ptr);
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
   * Get human-readable explanation of a variable.
   * @param variable Variable name
   * @returns Textual description of what defines/drives this variable
   */
  explain(variable: string): string {
    this.checkDisposed();

    for (const stock of this.stocks) {
      if (stock.name === variable) {
        const inflowsStr = stock.inflows.length > 0 ? stock.inflows.join(', ') : 'no inflows';
        const outflowsStr = stock.outflows.length > 0 ? stock.outflows.join(', ') : 'no outflows';
        return `${stock.name} is a stock with initial value ${stock.initialEquation}, increased by ${inflowsStr}, decreased by ${outflowsStr}`;
      }
    }

    for (const flow of this.flows) {
      if (flow.name === variable) {
        return `${flow.name} is a flow computed as ${flow.equation}`;
      }
    }

    for (const aux of this.auxs) {
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
   * Check model for common issues.
   * @returns Array of ModelIssue objects, or empty array if no issues
   */
  check(): ModelIssue[] {
    this.checkDisposed();
    if (this._project === null) {
      return [];
    }

    const errorDetails = this._project.getErrors();

    // Filter to errors for this model only
    const modelErrors = errorDetails.filter((detail) => {
      // If no model name on error, it's a project-level error - exclude
      if (!detail.modelName) {
        return false;
      }
      // For the main model (null name), match errors with no model or matching model
      if (this._name === null) {
        // Main model: include if modelName matches any model name in project
        // or if it's an empty string (legacy format)
        return true;
      }
      return detail.modelName === this._name;
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
  simulate(overrides: Record<string, number> = {}, options: { enableLtm?: boolean } = {}): Sim {
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
  run(overrides: Record<string, number> = {}, options: { analyzeLtm?: boolean } = {}): Run {
    this.checkDisposed();
    const { analyzeLtm = true } = options;

    const sim = this.simulate(overrides, { enableLtm: analyzeLtm });
    sim.runToEnd();

    return sim.getRun();
  }

  /**
   * Simulation results with default parameters (cached).
   */
  get baseCase(): Run {
    this.checkDisposed();
    if (this._cachedBaseCase === null) {
      this._cachedBaseCase = this.run();
    }
    return this._cachedBaseCase;
  }

  /**
   * Edit the model using a callback with patch builder.
   * @param callback Function that receives current variables and a patch builder
   * @param options Edit options (dryRun, allowErrors)
   */
  edit(
    callback: (currentVars: Record<string, JsonStock | JsonFlow | JsonAuxiliary>, patch: ModelPatchBuilder) => void,
    options: { dryRun?: boolean; allowErrors?: boolean } = {},
  ): void {
    this.checkDisposed();
    if (this._project === null) {
      throw new Error('Model is not attached to a Project');
    }

    const { dryRun = false, allowErrors = false } = options;

    // Get current model state as JSON
    const modelJson = this.getModelJson();
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

    // Call user callback - if it throws, the patch won't be applied
    // and model state remains unchanged
    callback(currentVars, patch);

    // If no operations, return early
    if (!patch.hasOperations()) {
      return;
    }

    // Build and apply the patch
    const projectPatch: JsonProjectPatch = {
      models: [patch.build()],
    };

    this._project.applyPatch(projectPatch, { dryRun, allowErrors });

    // Invalidate caches if not dry run
    if (!dryRun) {
      this.invalidateCaches();
    }
  }

  /**
   * Dispose this model and free WASM resources.
   */
  dispose(): void {
    if (this._disposed) {
      return;
    }

    simlin_model_unref(this._ptr);
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
