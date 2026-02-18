// Copyright 2026 The Simlin Authors. All rights reserved.
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
import { Stock, Flow, Aux, Module, Variable, TimeSpec, Link, Loop, ModelIssue, GraphicalFunction } from './types';
import {
  JsonStock,
  JsonFlow,
  JsonAuxiliary,
  JsonModule,
  JsonGraphicalFunction,
  JsonProjectPatch,
  JsonSimSpecs,
} from './json-types';
import { ErrorCode } from './errors';
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
 * Normalize a model name for comparison.
 * The engine reports canonical model names in error details (lowercase, underscored),
 * while models store their display names. This lets check() match them correctly.
 */
function canonicalizeModelName(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[\s_]+/g, '_');
}

/** Type mask constants matching SIMLIN_VARTYPE_* from the C FFI. */
export const SIMLIN_VARTYPE_STOCK = 1 << 0;
export const SIMLIN_VARTYPE_FLOW = 1 << 1;
export const SIMLIN_VARTYPE_AUX = 1 << 2;
export const SIMLIN_VARTYPE_MODULE = 1 << 3;

/**
 * JSON shape returned by the simlin_model_get_var_json FFI.
 * Each variable has a "type" discriminator field alongside the camelCase fields
 * matching the json-types.ts interfaces.
 */
type JsonVarWithType =
  | ({ type: 'stock' } & JsonStock)
  | ({ type: 'flow' } & JsonFlow)
  | ({ type: 'aux' } & JsonAuxiliary)
  | ({ type: 'module' } & JsonModule);

function parseJsonGraphicalFunction(gf: JsonGraphicalFunction): GraphicalFunction {
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

function extractEquation(
  topLevel: string | undefined,
  arrayed: { equation?: string } | undefined,
): string {
  if (topLevel) {
    return topLevel;
  }
  if (arrayed?.equation) {
    return arrayed.equation;
  }
  return '';
}

/**
 * Extract a stock's initial equation. For stocks, the initial value can appear
 * in the top-level `initialEquation` or in the arrayed `equation` field
 * (XMILE-sourced data where `<eqn>` IS the initial value).
 */
function extractStockInitialEquation(
  topLevel: string | undefined,
  arrayed: { equation?: string } | undefined,
): string {
  if (topLevel) {
    return topLevel;
  }
  if (arrayed?.equation) {
    return arrayed.equation;
  }
  return '';
}

function jsonVarToVariable(v: JsonVarWithType): Variable {
  switch (v.type) {
    case 'stock': {
      const s: Stock = {
        type: 'stock',
        name: v.name,
        initialEquation: extractStockInitialEquation(v.initialEquation, v.arrayedEquation),
        inflows: v.inflows || [],
        outflows: v.outflows || [],
        units: v.units || undefined,
        documentation: v.documentation || undefined,
        nonNegative: v.nonNegative || false,
        arrayedEquation: v.arrayedEquation,
        compat: v.compat || undefined,
      };
      return s;
    }
    case 'flow': {
      let gf: GraphicalFunction | undefined;
      if (v.graphicalFunction) {
        gf = parseJsonGraphicalFunction(v.graphicalFunction);
      }
      const f: Flow = {
        type: 'flow',
        name: v.name,
        equation: extractEquation(v.equation, v.arrayedEquation),
        units: v.units || undefined,
        documentation: v.documentation || undefined,
        nonNegative: v.nonNegative || false,
        graphicalFunction: gf,
        arrayedEquation: v.arrayedEquation,
        compat: v.compat || undefined,
      };
      return f;
    }
    case 'aux': {
      let gf: GraphicalFunction | undefined;
      if (v.graphicalFunction) {
        gf = parseJsonGraphicalFunction(v.graphicalFunction);
      }
      const activeInitial = v.compat?.activeInitial || v.arrayedEquation?.compat?.activeInitial;
      const compat = activeInitial ? { activeInitial } : undefined;
      const a: Aux = {
        type: 'aux',
        name: v.name,
        equation: extractEquation(v.equation, v.arrayedEquation),
        units: v.units || undefined,
        documentation: v.documentation || undefined,
        graphicalFunction: gf,
        arrayedEquation: v.arrayedEquation,
        compat,
      };
      return a;
    }
    case 'module': {
      const m: Module = {
        type: 'module',
        name: v.name,
        modelName: v.modelName,
      };
      return m;
    }
  }
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

  // Only simulation results are cached, since the new targeted FFI calls
  // are efficient enough that caching model data is unnecessary.
  private _cachedBaseCase: Run | null = null;

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
   * Invalidate cached data. Called after model edits.
   */
  invalidateCaches(): void {
    this._cachedBaseCase = null;
  }

  /**
   * Get a single variable by name.
   * @param name Variable name
   * @returns The variable, or undefined if not found
   */
  async getVariable(name: string): Promise<Variable | undefined> {
    this.checkDisposed();
    try {
      const bytes = await this.backend.modelGetVarJson(this._handle, name);
      const jsonVar = JSON.parse(new TextDecoder().decode(bytes)) as JsonVarWithType;
      return jsonVarToVariable(jsonVar);
    } catch (e: unknown) {
      const code = (e as { code?: number }).code;
      if (code === ErrorCode.DoesNotExist) {
        return undefined;
      }
      throw e;
    }
  }

  /**
   * Get variable names from the model, optionally filtered by type and/or substring.
   * @param typeMask Bitmask of SIMLIN_VARTYPE_STOCK | FLOW | AUX | MODULE. 0 means all.
   * @param filter Substring filter on canonicalized names. null means no filter.
   * @returns Array of canonical variable names
   */
  async getVarNames(typeMask: number = 0, filter: string | null = null): Promise<string[]> {
    this.checkDisposed();
    return await this.backend.modelGetVarNames(this._handle, typeMask, filter);
  }

  /**
   * Time specification for simulation.
   * Retrieved directly from the engine via the model handle, which already
   * resolves the model-level vs project-level sim specs precedence.
   */
  async timeSpec(): Promise<TimeSpec> {
    this.checkDisposed();

    const bytes = await this.backend.modelGetSimSpecsJson(this._handle);
    const simSpecs = JSON.parse(new TextDecoder().decode(bytes)) as JsonSimSpecs;

    return {
      start: simSpecs.startTime ?? 0,
      stop: simSpecs.endTime ?? 10,
      dt: parseDt(simSpecs.dt ?? '1'),
      units: simSpecs.timeUnits || undefined,
    };
  }

  /**
   * Structural feedback loops (no behavior data).
   */
  async loops(): Promise<readonly Loop[]> {
    this.checkDisposed();
    return await this.backend.modelGetLoops(this._handle);
  }

  /**
   * Get the dependencies (incoming links) for a given variable.
   * @param varName The name of the variable to query
   * @returns List of variable names that this variable depends on
   */
  async getIncomingLinks(varName: string): Promise<string[]> {
    this.checkDisposed();
    return await this.backend.modelGetIncomingLinks(this._handle, varName);
  }

  /**
   * Get all causal links in the model (static analysis).
   * @returns List of Link objects representing causal relationships
   */
  async getLinks(): Promise<Link[]> {
    this.checkDisposed();
    return await this.backend.modelGetLinks(this._handle);
  }

  /**
   * Get human-readable explanation of a variable.
   * @param variable Variable name
   * @returns Textual description of what defines/drives this variable
   */
  async explain(variable: string): Promise<string> {
    this.checkDisposed();

    const v = await this.getVariable(variable);
    if (v === undefined) {
      throw new Error(`Variable '${variable}' not found in model`);
    }

    switch (v.type) {
      case 'stock': {
        const inflowsStr = v.inflows.length > 0 ? v.inflows.join(', ') : 'no inflows';
        const outflowsStr = v.outflows.length > 0 ? v.outflows.join(', ') : 'no outflows';
        return `${v.name} is a stock with initial value ${v.initialEquation}, increased by ${inflowsStr}, decreased by ${outflowsStr}`;
      }
      case 'flow':
        return `${v.name} is a flow computed as ${v.equation}`;
      case 'aux':
        if (v.compat?.activeInitial) {
          return `${v.name} is an auxiliary variable computed as ${v.equation} with initial value ${v.compat.activeInitial}`;
        }
        return `${v.name} is an auxiliary variable computed as ${v.equation}`;
      case 'module':
        return `${v.name} is a module instantiating model ${v.modelName}`;
    }
  }

  /**
   * Get the LaTeX representation of a variable's equation.
   * @param ident Variable identifier
   * @returns LaTeX string, or null if not found
   */
  async getLatexEquation(ident: string): Promise<string | null> {
    this.checkDisposed();
    return await this.backend.modelGetLatexEquation(this._handle, ident);
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

    // Use the model name directly. For the main model (where _name is null),
    // we need to figure out the actual name from the project model list.
    let actualModelName: string | null = this._name;
    if (actualModelName === null) {
      const names = await this._project.getModelNames();
      if (names.length > 0) {
        actualModelName = names[0];
      }
    }

    if (actualModelName === null) {
      return [];
    }

    const canonicalName = canonicalizeModelName(actualModelName);

    // Filter to errors for this model only, using canonical comparison
    // since error details report model names in canonical form.
    const modelErrors = errorDetails.filter((detail) => {
      if (!detail.modelName) {
        return false;
      }
      return canonicalizeModelName(detail.modelName) === canonicalName;
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
    return Sim.create(this, overrides, enableLtm);
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

    // Get current editable variable names (stocks + flows + auxs)
    const varNames = await this.getVarNames(SIMLIN_VARTYPE_STOCK | SIMLIN_VARTYPE_FLOW | SIMLIN_VARTYPE_AUX);

    const currentVars: Record<string, JsonStock | JsonFlow | JsonAuxiliary> = {};
    for (const name of varNames) {
      const bytes = await this.backend.modelGetVarJson(this._handle, name);
      const v = JSON.parse(new TextDecoder().decode(bytes)) as JsonVarWithType;
      switch (v.type) {
        case 'stock':
          currentVars[v.name] = v as JsonStock;
          break;
        case 'flow':
          currentVars[v.name] = v as JsonFlow;
          break;
        case 'aux':
          currentVars[v.name] = v as JsonAuxiliary;
          break;
      }
    }

    // The model name for the patch. Use _name if available, otherwise
    // look up the first model name from the project.
    let modelName = this._name;
    if (modelName === null) {
      const names = await this._project.getModelNames();
      if (names.length === 0) {
        throw new Error('No models in project');
      }
      modelName = names[0];
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
    if (this._disposed) {
      return;
    }

    await this.backend.modelDispose(this._handle);
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

    const result = this.backend.modelDispose(this._handle);
    if (result instanceof Promise) {
      result.catch((e) => console.warn('Model dispose failed:', e));
    }
    this._disposed = true;
  }
}
