// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined, mapSet, mapValues, Series } from './common';

import { canonicalize } from './canonicalize';

import { ErrorCode } from './errors';

import {
  type JsonProject,
  type JsonModel,
  type JsonStock,
  type JsonFlow,
  type JsonAuxiliary,
  type JsonModule,
  type JsonModuleReference,
  type JsonSimSpecs,
  type JsonDimension,
  type JsonGraphicalFunction,
  type JsonGraphicalFunctionScale,
  type JsonArrayedEquation,
  type JsonElementEquation,
  type JsonDataSource,
  type JsonView,
  type JsonViewElement,
  type JsonStockViewElement,
  type JsonFlowViewElement,
  type JsonAuxiliaryViewElement,
  type JsonCloudViewElement,
  type JsonLinkViewElement,
  type JsonModuleViewElement,
  type JsonAliasViewElement,
  type JsonGroupViewElement,
  type JsonRect,
  type JsonFlowPoint,
  type JsonLinkPoint,
  type JsonLoopMetadata,
  type JsonMacroSpec,
  type JsonModelGroup,
  type JsonSource,
} from '@simlin/engine';

export { ErrorCode };

export type UID = number;

export interface EquationError {
  readonly code: ErrorCode;
  readonly start: number;
  readonly end: number;
}

export interface UnitError {
  readonly code: ErrorCode;
  readonly start: number;
  readonly end: number;
  readonly isConsistencyError: boolean;
  readonly details: string | undefined;
}

export interface SimError {
  readonly code: ErrorCode;
  readonly details: string | undefined;
}

// A sketch-hygiene issue: the sketch connectors for a variable have drifted out
// of sync with its equation. `missingConnector` -- the equation references
// `ident` but no connector is drawn from it; `staleConnector` -- a connector is
// drawn from `ident` but the equation does not reference it. `name` is the
// other variable's display (original-case) name for the details panel. Computed
// in the diagram layer (see diagram/connector-sync.ts) and attached to
// variables like `errors`/`unitErrors`; never serialized.
export type ConnectorErrorKind = 'missingConnector' | 'staleConnector';

export interface ConnectorError {
  readonly kind: ConnectorErrorKind;
  readonly ident: string;
  readonly name: string;
}

export interface ModelError {
  readonly code: ErrorCode;
  readonly details: string | undefined;
}

export type GraphicalFunctionKind = 'continuous' | 'extrapolate' | 'discrete';

export interface GraphicalFunctionScale {
  readonly min: number;
  readonly max: number;
}

export function graphicalFunctionScaleDefault(): GraphicalFunctionScale {
  return { min: 0, max: 0 };
}

export function graphicalFunctionScaleFromJson(json: JsonGraphicalFunctionScale): GraphicalFunctionScale {
  return { min: json.min, max: json.max };
}

export function graphicalFunctionScaleToJson(scale: GraphicalFunctionScale): JsonGraphicalFunctionScale {
  return { min: scale.min, max: scale.max };
}

export interface GraphicalFunction {
  readonly kind: GraphicalFunctionKind;
  readonly xPoints: readonly number[] | undefined;
  readonly yPoints: readonly number[];
  readonly xScale: GraphicalFunctionScale;
  readonly yScale: GraphicalFunctionScale;
}

export function graphicalFunctionFromJson(json: JsonGraphicalFunction): GraphicalFunction {
  let xPoints: readonly number[] | undefined;
  let yPoints: readonly number[];

  if (json.points && json.points.length > 0) {
    xPoints = json.points.map((p: [number, number]) => p[0]);
    yPoints = json.points.map((p: [number, number]) => p[1]);
  } else {
    xPoints = undefined;
    yPoints = json.yPoints ?? [];
  }

  const xScale = json.xScale
    ? graphicalFunctionScaleFromJson(json.xScale)
    : { min: 0, max: Math.max(0, yPoints.length - 1) };
  const yScale = json.yScale ? graphicalFunctionScaleFromJson(json.yScale) : graphicalFunctionScaleDefault();

  let kind: GraphicalFunctionKind = 'continuous';
  if (json.kind === 'discrete') {
    kind = 'discrete';
  } else if (json.kind === 'extrapolate') {
    kind = 'extrapolate';
  }

  return { kind, xPoints, yPoints, xScale, yScale };
}

export function graphicalFunctionToJson(gf: GraphicalFunction): JsonGraphicalFunction {
  const result: JsonGraphicalFunction = {};

  if (gf.xPoints && gf.xPoints.length > 0) {
    result.points = gf.xPoints.map((x, i) => [x, gf.yPoints[i]] as [number, number]);
  } else {
    result.yPoints = [...gf.yPoints];
  }

  if (gf.kind && gf.kind !== 'continuous') {
    result.kind = gf.kind;
  }

  if (gf.xScale) {
    result.xScale = graphicalFunctionScaleToJson(gf.xScale);
  }
  if (gf.yScale) {
    result.yScale = graphicalFunctionScaleToJson(gf.yScale);
  }

  return result;
}

// DataSource: external-data reference (Vensim GET DIRECT DATA / CONSTANTS /
// LOOKUPS / SUBSCRIPT). Carried on a variable's compat so an edit to any other
// field does not drop the reference. The wire shape and `kind` values are
// defined by the Rust json::JsonDataSource serializer (src/simlin-engine/src/json.rs).

export type DataSourceKind = 'data' | 'constants' | 'lookups' | 'subscript';

export interface DataSource {
  readonly kind: DataSourceKind;
  readonly file: string;
  readonly tabOrDelimiter: string;
  readonly rowOrCol: string;
  readonly cell: string;
}

function dataSourceKindFromJson(kind: string): DataSourceKind {
  // Mirror the Rust data_source_from_json fallback: any unrecognized kind
  // (including the default "data") maps to 'data'.
  switch (kind) {
    case 'constants':
      return 'constants';
    case 'lookups':
      return 'lookups';
    case 'subscript':
      return 'subscript';
    default:
      return 'data';
  }
}

export function dataSourceFromJson(json: JsonDataSource): DataSource {
  return {
    kind: dataSourceKindFromJson(json.kind),
    file: json.file ?? '',
    tabOrDelimiter: json.tabOrDelimiter ?? '',
    rowOrCol: json.rowOrCol ?? '',
    cell: json.cell ?? '',
  };
}

export function dataSourceToJson(ds: DataSource): JsonDataSource {
  return {
    kind: ds.kind,
    file: ds.file,
    tabOrDelimiter: ds.tabOrDelimiter,
    rowOrCol: ds.rowOrCol,
    cell: ds.cell,
  };
}

// Equation types

export interface ScalarEquation {
  readonly type: 'scalar';
  readonly equation: string;
}

export interface ApplyToAllEquation {
  readonly type: 'applyToAll';
  readonly dimensionNames: readonly string[];
  readonly equation: string;
}

// A single element of an arrayed (per-element) equation. Carries the
// element's equation plus any per-element graphical function and ACTIVE
// INITIAL equation (the only per-element compat field the engine round-trips).
export interface ArrayedElement {
  readonly equation: string;
  readonly graphicalFunction: GraphicalFunction | undefined;
  readonly activeInitial: string | undefined;
}

export interface ArrayedEquation {
  readonly type: 'arrayed';
  readonly dimensionNames: readonly string[];
  readonly elements: ReadonlyMap<string, ArrayedElement>;
  // The EXCEPT default equation applied to elements not listed in `elements`,
  // and the flag indicating it is an EXCEPT default. `hasExceptDefault` is only
  // meaningful when `defaultEquation` is set.
  readonly defaultEquation: string | undefined;
  readonly hasExceptDefault: boolean;
}

export type Equation = ScalarEquation | ApplyToAllEquation | ArrayedEquation;

// Build the datamodel ArrayedEquation from its JSON form, preserving per-element
// equations, graphical functions, and ACTIVE INITIAL equations as well as the
// EXCEPT default. Shared by stocks, flows, and auxes (the per-element shape is
// identical across variable kinds).
export function arrayedEquationFromJson(json: JsonArrayedEquation): ArrayedEquation {
  const dimensionNames: readonly string[] = json.dimensions ?? [];
  const elements = new Map<string, ArrayedElement>(
    (json.elements ?? []).map(
      (el: JsonElementEquation) =>
        [
          el.subscript,
          {
            equation: el.equation,
            graphicalFunction: el.graphicalFunction ? graphicalFunctionFromJson(el.graphicalFunction) : undefined,
            // The engine only round-trips activeInitial out of a per-element compat.
            activeInitial: el.compat?.activeInitial,
          },
        ] as [string, ArrayedElement],
    ),
  );
  // Mirror the engine: a legacy payload without hasExceptDefault infers `true`
  // when a default equation is present (preserving pre-flag behavior).
  const hasExceptDefault = json.hasExceptDefault ?? json.equation !== undefined;
  return {
    type: 'arrayed',
    dimensionNames,
    elements,
    defaultEquation: json.equation,
    hasExceptDefault,
  };
}

// Serialize a datamodel ArrayedEquation back to JSON. Only emits the EXCEPT
// default flag when a default equation is present (mirroring the engine, where
// the flag is meaningless without one).
export function arrayedEquationToJson(eq: ArrayedEquation): JsonArrayedEquation {
  const result: JsonArrayedEquation = {
    dimensions: [...eq.dimensionNames],
    elements: [...eq.elements].map(([subscript, el]) => {
      const jsonEl: JsonElementEquation = { subscript, equation: el.equation };
      if (el.graphicalFunction) {
        jsonEl.graphicalFunction = graphicalFunctionToJson(el.graphicalFunction);
      }
      if (el.activeInitial) {
        jsonEl.compat = { activeInitial: el.activeInitial };
      }
      return jsonEl;
    }),
  };
  if (eq.defaultEquation !== undefined) {
    result.equation = eq.defaultEquation;
    result.hasExceptDefault = eq.hasExceptDefault;
  }
  return result;
}

function stockEquationFromJson(
  initialEquation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
): Equation {
  if (arrayedEquation) {
    // The engine distinguishes ApplyToAll from Arrayed by the PRESENCE of the
    // `elements` field, not by it being non-empty (json.rs: ApplyToAll omits
    // `elements`; Arrayed always emits it, even as []). An Arrayed with no
    // explicit elements + an EXCEPT default + hasExceptDefault:false means every
    // element is missing and evaluates to 0, NOT the default (compiler/mod.rs:
    // a missing element uses the default only when apply_default_for_missing is
    // true). Collapsing that to applyToAll would silently change behavior, so
    // route on presence: `elements` present (even []) => arrayed.
    if (arrayedEquation.elements !== undefined) {
      return arrayedEquationFromJson(arrayedEquation);
    } else {
      return {
        type: 'applyToAll',
        dimensionNames: arrayedEquation.dimensions ?? [],
        equation: arrayedEquation.equation ?? '',
      };
    }
  }
  return { type: 'scalar', equation: initialEquation ?? '' };
}

function auxEquationFromJson(
  equation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
  gf: JsonGraphicalFunction | undefined,
): { equation: Equation; graphicalFunction: GraphicalFunction | undefined } {
  let graphicalFunction: GraphicalFunction | undefined;
  if (gf) {
    graphicalFunction = graphicalFunctionFromJson(gf);
  }

  if (arrayedEquation) {
    // Route on the PRESENCE of `elements`, not on it being non-empty -- see the
    // matching note in stockEquationFromJson. The engine emits `elements` for
    // every Arrayed (even []) and omits it for ApplyToAll; an Arrayed with no
    // explicit elements + hasExceptDefault:false has every element evaluate to 0,
    // which collapsing to applyToAll would silently change.
    if (arrayedEquation.elements !== undefined) {
      return { equation: arrayedEquationFromJson(arrayedEquation), graphicalFunction };
    } else {
      return {
        equation: {
          type: 'applyToAll',
          dimensionNames: arrayedEquation.dimensions ?? [],
          equation: arrayedEquation.equation ?? '',
        },
        graphicalFunction,
      };
    }
  }
  return {
    equation: { type: 'scalar', equation: equation ?? '' },
    graphicalFunction,
  };
}

function stockEquationToJson(equation: Equation): { initialEquation?: string; arrayedEquation?: JsonArrayedEquation } {
  if (equation.type === 'scalar') {
    return { initialEquation: equation.equation || undefined };
  } else if (equation.type === 'applyToAll') {
    return {
      arrayedEquation: {
        dimensions: [...equation.dimensionNames],
        equation: equation.equation || undefined,
      },
    };
  } else if (equation.type === 'arrayed') {
    return { arrayedEquation: arrayedEquationToJson(equation) };
  }
  return {};
}

function auxEquationToJson(equation: Equation): { equation?: string; arrayedEquation?: JsonArrayedEquation } {
  if (equation.type === 'scalar') {
    return { equation: equation.equation || undefined };
  } else if (equation.type === 'applyToAll') {
    return {
      arrayedEquation: {
        dimensions: [...equation.dimensionNames],
        equation: equation.equation || undefined,
      },
    };
  } else if (equation.type === 'arrayed') {
    return { arrayedEquation: arrayedEquationToJson(equation) };
  }
  return {};
}

// Variable types

export interface Stock {
  readonly type: 'stock';
  readonly ident: string;
  readonly equation: Equation;
  readonly documentation: string;
  readonly units: string;
  readonly inflows: readonly string[];
  readonly outflows: readonly string[];
  readonly nonNegative: boolean;
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
  // Vensim ACTIVE INITIAL: the variable's separate initialization equation.
  readonly activeInitial: string | undefined;
  // External-data reference (Vensim GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT).
  readonly dataSource: DataSource | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  // Sketch-connector drift, attached by the diagram layer (connector-sync.ts).
  // Optional so the many Variable literals that predate this feature stay valid;
  // absent and undefined are equivalent ("no connector issues").
  readonly connectorErrors?: readonly ConnectorError[] | undefined;
  readonly uid: number | undefined;
}

export interface Flow {
  readonly type: 'flow';
  readonly ident: string;
  readonly equation: Equation;
  readonly documentation: string;
  readonly units: string;
  readonly gf: GraphicalFunction | undefined;
  readonly nonNegative: boolean;
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
  readonly activeInitial: string | undefined;
  readonly dataSource: DataSource | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  // Sketch-connector drift, attached by the diagram layer (connector-sync.ts).
  // Optional so the many Variable literals that predate this feature stay valid;
  // absent and undefined are equivalent ("no connector issues").
  readonly connectorErrors?: readonly ConnectorError[] | undefined;
  readonly uid: number | undefined;
}

export interface Aux {
  readonly type: 'aux';
  readonly ident: string;
  readonly equation: Equation;
  readonly documentation: string;
  readonly units: string;
  readonly gf: GraphicalFunction | undefined;
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
  readonly activeInitial: string | undefined;
  readonly dataSource: DataSource | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  // Sketch-connector drift, attached by the diagram layer (connector-sync.ts).
  // Optional so the many Variable literals that predate this feature stay valid;
  // absent and undefined are equivalent ("no connector issues").
  readonly connectorErrors?: readonly ConnectorError[] | undefined;
  readonly uid: number | undefined;
}

export interface ModuleReference {
  readonly src: string;
  readonly dst: string;
}

export function moduleReferenceFromJson(json: JsonModuleReference): ModuleReference {
  return { src: json.src, dst: json.dst };
}

export function moduleReferenceToJson(ref: ModuleReference): JsonModuleReference {
  return { src: ref.src, dst: ref.dst };
}

export interface Module {
  readonly type: 'module';
  readonly ident: string;
  readonly modelName: string;
  readonly documentation: string;
  readonly units: string;
  readonly references: readonly ModuleReference[];
  // The engine reads only canBeModuleInput, isPublic, and dataSource out of a
  // module's compat (From<Module> in json.rs uses defaults for the rest), so
  // ACTIVE INITIAL and nonNegative are intentionally absent here.
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
  readonly dataSource: DataSource | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  // Sketch-connector drift, attached by the diagram layer (connector-sync.ts).
  // Optional so the many Variable literals that predate this feature stay valid;
  // absent and undefined are equivalent ("no connector issues").
  readonly connectorErrors?: readonly ConnectorError[] | undefined;
  readonly uid: number | undefined;
}

export type Variable = Stock | Flow | Aux | Module;

export function variableIsArrayed(v: Variable): boolean {
  if (v.type === 'module') return false;
  return v.equation.type === 'applyToAll' || v.equation.type === 'arrayed';
}

export function variableHasError(v: Variable): boolean {
  // Includes non-fatal warnings (unit errors, sketch-connector drift), matching
  // how the diagram surfaces every variable problem with the same indicator.
  // Simulatability is decided separately (engine.isSimulatable), so a
  // connector-only warning never blocks a run.
  return v.errors !== undefined || v.unitErrors !== undefined || v.connectorErrors !== undefined;
}

export function variableGf(v: Variable): GraphicalFunction | undefined {
  if (v.type === 'flow' || v.type === 'aux') return v.gf;
  return undefined;
}

export function variableEquation(v: Variable): Equation | undefined {
  if (v.type === 'module') return undefined;
  return v.equation;
}

export function stockFromJson(json: JsonStock): Stock {
  return {
    type: 'stock',
    ident: canonicalize(json.name),
    equation: stockEquationFromJson(json.initialEquation, json.arrayedEquation),
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    inflows: json.inflows ?? [],
    outflows: json.outflows ?? [],
    // OR-merge: old code never writes compat booleans and new code never
    // writes top-level booleans, so both cannot be meaningfully set at once.
    // Mirrors the engine's JSON reader (json.rs), which OR-merges all three
    // legacy top-level flags so a project saved in the old schema is not
    // silently stripped on the next edit.
    nonNegative: json.compat?.nonNegative || json.nonNegative || false,
    canBeModuleInput: json.compat?.canBeModuleInput || json.canBeModuleInput || false,
    isPublic: json.compat?.isPublic || json.isPublic || false,
    activeInitial: json.compat?.activeInitial,
    dataSource: json.compat?.dataSource ? dataSourceFromJson(json.compat.dataSource) : undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    connectorErrors: undefined,
    uid: json.uid,
  };
}

export function stockToJson(stock: Stock): JsonStock {
  const eqJson = stockEquationToJson(stock.equation);
  const result: JsonStock = {
    name: stock.ident,
    inflows: [...stock.inflows],
    outflows: [...stock.outflows],
  };
  if (stock.uid !== undefined) {
    result.uid = stock.uid;
  }
  if (eqJson.initialEquation) {
    result.initialEquation = eqJson.initialEquation;
  }
  if (eqJson.arrayedEquation) {
    result.arrayedEquation = eqJson.arrayedEquation;
  }
  if (stock.units) {
    result.units = stock.units;
  }
  if (stock.nonNegative) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.nonNegative = stock.nonNegative;
  }
  if (stock.canBeModuleInput) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.canBeModuleInput = true;
  }
  if (stock.isPublic) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.isPublic = true;
  }
  if (stock.activeInitial) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.activeInitial = stock.activeInitial;
  }
  if (stock.dataSource) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.dataSource = dataSourceToJson(stock.dataSource);
  }
  if (stock.documentation) {
    result.documentation = stock.documentation;
  }
  return result;
}

export function flowFromJson(json: JsonFlow): Flow {
  const { equation, graphicalFunction } = auxEquationFromJson(
    json.equation,
    json.arrayedEquation,
    json.graphicalFunction,
  );
  return {
    type: 'flow',
    ident: canonicalize(json.name),
    equation,
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    gf: graphicalFunction,
    // OR-merge: old code never writes compat booleans and new code never
    // writes top-level booleans, so both cannot be meaningfully set at once.
    // Mirrors the engine's JSON reader (json.rs), which OR-merges all three
    // legacy top-level flags so a project saved in the old schema is not
    // silently stripped on the next edit.
    nonNegative: json.compat?.nonNegative || json.nonNegative || false,
    canBeModuleInput: json.compat?.canBeModuleInput || json.canBeModuleInput || false,
    isPublic: json.compat?.isPublic || json.isPublic || false,
    // ACTIVE INITIAL: top-level compat wins, else fall back to the arrayed
    // equation's compat (a legacy/native JSON shape the engine reader accepts).
    activeInitial: json.compat?.activeInitial || json.arrayedEquation?.compat?.activeInitial,
    dataSource: json.compat?.dataSource ? dataSourceFromJson(json.compat.dataSource) : undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    connectorErrors: undefined,
    uid: json.uid,
  };
}

export function flowToJson(flow: Flow): JsonFlow {
  const eqJson = auxEquationToJson(flow.equation);
  const result: JsonFlow = {
    name: flow.ident,
  };
  if (flow.uid !== undefined) {
    result.uid = flow.uid;
  }
  if (eqJson.equation) {
    result.equation = eqJson.equation;
  }
  if (eqJson.arrayedEquation) {
    result.arrayedEquation = eqJson.arrayedEquation;
  }
  if (flow.gf) {
    result.graphicalFunction = graphicalFunctionToJson(flow.gf);
  }
  if (flow.units) {
    result.units = flow.units;
  }
  if (flow.nonNegative) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.nonNegative = flow.nonNegative;
  }
  if (flow.canBeModuleInput) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.canBeModuleInput = true;
  }
  if (flow.isPublic) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.isPublic = true;
  }
  if (flow.activeInitial) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.activeInitial = flow.activeInitial;
  }
  if (flow.dataSource) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.dataSource = dataSourceToJson(flow.dataSource);
  }
  if (flow.documentation) {
    result.documentation = flow.documentation;
  }
  return result;
}

export function auxFromJson(json: JsonAuxiliary): Aux {
  const { equation, graphicalFunction } = auxEquationFromJson(
    json.equation,
    json.arrayedEquation,
    json.graphicalFunction,
  );
  return {
    type: 'aux',
    ident: canonicalize(json.name),
    equation,
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    gf: graphicalFunction,
    // OR-merge legacy top-level flags with compat, mirroring the engine's JSON
    // reader (json.rs); old JSON wrote them at top level, new JSON under compat.
    canBeModuleInput: json.compat?.canBeModuleInput || json.canBeModuleInput || false,
    isPublic: json.compat?.isPublic || json.isPublic || false,
    // ACTIVE INITIAL: top-level compat wins, else fall back to the arrayed
    // equation's compat (a legacy/native JSON shape the engine reader accepts).
    activeInitial: json.compat?.activeInitial || json.arrayedEquation?.compat?.activeInitial,
    dataSource: json.compat?.dataSource ? dataSourceFromJson(json.compat.dataSource) : undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    connectorErrors: undefined,
    uid: json.uid,
  };
}

export function auxToJson(aux: Aux): JsonAuxiliary {
  const eqJson = auxEquationToJson(aux.equation);
  const result: JsonAuxiliary = {
    name: aux.ident,
  };
  if (aux.uid !== undefined) {
    result.uid = aux.uid;
  }
  if (eqJson.equation) {
    result.equation = eqJson.equation;
  }
  if (eqJson.arrayedEquation) {
    result.arrayedEquation = eqJson.arrayedEquation;
  }
  if (aux.gf) {
    result.graphicalFunction = graphicalFunctionToJson(aux.gf);
  }
  if (aux.units) {
    result.units = aux.units;
  }
  if (aux.canBeModuleInput) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.canBeModuleInput = true;
  }
  if (aux.isPublic) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.isPublic = true;
  }
  if (aux.activeInitial) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.activeInitial = aux.activeInitial;
  }
  if (aux.dataSource) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.dataSource = dataSourceToJson(aux.dataSource);
  }
  if (aux.documentation) {
    result.documentation = aux.documentation;
  }
  return result;
}

export function moduleFromJson(json: JsonModule): Module {
  return {
    type: 'module',
    ident: canonicalize(json.name),
    modelName: json.modelName,
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    references: (json.references ?? []).map((ref: JsonModuleReference) => moduleReferenceFromJson(ref)),
    // OR-merge legacy top-level flags with compat, mirroring the engine's JSON
    // reader (json.rs); old JSON wrote them at top level, new JSON under compat.
    canBeModuleInput: json.compat?.canBeModuleInput || json.canBeModuleInput || false,
    isPublic: json.compat?.isPublic || json.isPublic || false,
    dataSource: json.compat?.dataSource ? dataSourceFromJson(json.compat.dataSource) : undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    connectorErrors: undefined,
    uid: json.uid,
  };
}

export function moduleToJson(mod: Module): JsonModule {
  const result: JsonModule = {
    name: mod.ident,
    modelName: mod.modelName,
  };
  if (mod.uid !== undefined) {
    result.uid = mod.uid;
  }
  if (mod.references.length > 0) {
    result.references = mod.references.map((ref) => moduleReferenceToJson(ref));
  }
  if (mod.units) {
    result.units = mod.units;
  }
  if (mod.canBeModuleInput) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.canBeModuleInput = true;
  }
  if (mod.isPublic) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.isPublic = true;
  }
  if (mod.dataSource) {
    if (!result.compat) {
      result.compat = {};
    }
    result.compat.dataSource = dataSourceToJson(mod.dataSource);
  }
  if (mod.documentation) {
    result.documentation = mod.documentation;
  }
  return result;
}

function variableToJson(v: Variable): JsonStock | JsonFlow | JsonAuxiliary | JsonModule {
  switch (v.type) {
    case 'stock':
      return stockToJson(v);
    case 'flow':
      return flowToJson(v);
    case 'aux':
      return auxToJson(v);
    case 'module':
      return moduleToJson(v);
  }
}

// View types

export type LabelSide = 'top' | 'left' | 'center' | 'bottom' | 'right';

export interface AuxViewElement {
  readonly type: 'aux';
  readonly uid: number;
  readonly name: string;
  readonly ident: string;
  readonly var: Aux | undefined;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly isZeroRadius: boolean;
}

export interface StockViewElement {
  readonly type: 'stock';
  readonly uid: number;
  readonly name: string;
  readonly ident: string;
  readonly var: Stock | undefined;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly isZeroRadius: boolean;
  readonly inflows: readonly UID[];
  readonly outflows: readonly UID[];
}

export interface Point {
  readonly x: number;
  readonly y: number;
  readonly attachedToUid: number | undefined;
}

export function pointFromJson(json: JsonFlowPoint): Point {
  return { x: json.x, y: json.y, attachedToUid: json.attachedToUid };
}

export function pointToJson(point: Point): JsonFlowPoint {
  const result: JsonFlowPoint = { x: point.x, y: point.y };
  if (point.attachedToUid !== undefined) {
    result.attachedToUid = point.attachedToUid;
  }
  return result;
}

export interface FlowViewElement {
  readonly type: 'flow';
  readonly uid: number;
  readonly name: string;
  readonly ident: string;
  readonly var: Flow | undefined;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly points: readonly Point[];
  readonly isZeroRadius: boolean;
}

export interface LinkViewElement {
  readonly type: 'link';
  readonly uid: number;
  readonly fromUid: number;
  readonly toUid: number;
  readonly arc: number | undefined;
  readonly isStraight: boolean;
  readonly multiPoint: readonly Point[] | undefined;
  readonly polarity: string | undefined;
  readonly x: number;
  readonly y: number;
  readonly isZeroRadius: boolean;
  readonly ident: undefined;
}

export interface ModuleViewElement {
  readonly type: 'module';
  readonly uid: number;
  readonly name: string;
  readonly ident: string;
  readonly var: Module | undefined;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly isZeroRadius: boolean;
}

export interface AliasViewElement {
  readonly type: 'alias';
  readonly uid: number;
  readonly aliasOfUid: number;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly isZeroRadius: boolean;
  readonly ident: undefined;
}

export interface CloudViewElement {
  readonly type: 'cloud';
  readonly uid: number;
  readonly flowUid: number;
  readonly x: number;
  readonly y: number;
  readonly isZeroRadius: boolean;
  readonly ident: undefined;
}

export interface GroupViewElement {
  readonly type: 'group';
  readonly uid: number;
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  readonly isZeroRadius: boolean;
  readonly ident: undefined;
}

export type ViewElement =
  | AuxViewElement
  | StockViewElement
  | FlowViewElement
  | LinkViewElement
  | ModuleViewElement
  | AliasViewElement
  | CloudViewElement
  | GroupViewElement;

export type NamedViewElement = AuxViewElement | StockViewElement | ModuleViewElement | FlowViewElement;

export function isNamedViewElement(el: ViewElement): el is NamedViewElement {
  return el.type === 'stock' || el.type === 'aux' || el.type === 'module' || el.type === 'flow';
}

export function viewElementType(
  element: ViewElement,
): 'aux' | 'stock' | 'flow' | 'link' | 'module' | 'alias' | 'cloud' | 'group' {
  return element.type;
}

// View element fromJson functions

export function auxViewElementFromJson(json: JsonAuxiliaryViewElement, auxVar?: Variable | undefined): AuxViewElement {
  const ident = canonicalize(json.name);
  return {
    type: 'aux',
    uid: json.uid,
    name: json.name,
    ident,
    var: auxVar?.type === 'aux' ? auxVar : undefined,
    x: json.x,
    y: json.y,
    labelSide: (json.labelSide ?? 'right') as LabelSide,
    isZeroRadius: false,
  };
}

export function stockViewElementFromJson(
  json: JsonStockViewElement,
  stockVar?: Variable | undefined,
): StockViewElement {
  const ident = canonicalize(json.name);
  return {
    type: 'stock',
    uid: json.uid,
    name: json.name,
    ident,
    var: stockVar?.type === 'stock' ? stockVar : undefined,
    x: json.x,
    y: json.y,
    labelSide: (json.labelSide ?? 'center') as LabelSide,
    isZeroRadius: false,
    inflows: [],
    outflows: [],
  };
}

export function flowViewElementFromJson(json: JsonFlowViewElement, flowVar?: Variable | undefined): FlowViewElement {
  const ident = canonicalize(json.name);
  return {
    type: 'flow',
    uid: json.uid,
    name: json.name,
    ident,
    var: flowVar?.type === 'flow' ? flowVar : undefined,
    x: json.x,
    y: json.y,
    labelSide: (json.labelSide ?? 'center') as LabelSide,
    points: (json.points ?? []).map((p: JsonFlowPoint) => pointFromJson(p)),
    isZeroRadius: false,
  };
}

export function linkViewElementFromJson(json: JsonLinkViewElement): LinkViewElement {
  let arc: number | undefined = undefined;
  let isStraight = false;
  let multiPoint: readonly Point[] | undefined = undefined;

  if (json.arc !== undefined) {
    arc = json.arc;
  } else if (json.multiPoints && json.multiPoints.length > 0) {
    multiPoint = json.multiPoints.map((p: JsonLinkPoint) => ({ x: p.x, y: p.y, attachedToUid: undefined }));
  } else {
    isStraight = true;
  }

  return {
    type: 'link',
    uid: json.uid,
    fromUid: json.fromUid,
    toUid: json.toUid,
    arc,
    isStraight,
    multiPoint,
    polarity: json.polarity,
    x: NaN,
    y: NaN,
    isZeroRadius: false,
    ident: undefined,
  };
}

export function moduleViewElementFromJson(
  json: JsonModuleViewElement,
  moduleVar?: Variable | undefined,
): ModuleViewElement {
  const ident = canonicalize(json.name);
  return {
    type: 'module',
    uid: json.uid,
    name: json.name,
    ident,
    var: moduleVar?.type === 'module' ? moduleVar : undefined,
    x: json.x,
    y: json.y,
    labelSide: (json.labelSide ?? 'center') as LabelSide,
    isZeroRadius: false,
  };
}

export function aliasViewElementFromJson(json: JsonAliasViewElement): AliasViewElement {
  return {
    type: 'alias',
    uid: json.uid,
    aliasOfUid: json.aliasOfUid,
    x: json.x,
    y: json.y,
    labelSide: (json.labelSide ?? 'center') as LabelSide,
    isZeroRadius: false,
    ident: undefined,
  };
}

export function cloudViewElementFromJson(json: JsonCloudViewElement): CloudViewElement {
  return {
    type: 'cloud',
    uid: json.uid,
    flowUid: json.flowUid,
    x: json.x,
    y: json.y,
    isZeroRadius: false,
    ident: undefined,
  };
}

// XMILE stores groups with top-left x/y, but we normalize to center-based
// coordinates internally to match all other ViewElements.
export function groupViewElementFromJson(json: JsonGroupViewElement): GroupViewElement {
  return {
    type: 'group',
    uid: json.uid,
    name: json.name,
    x: json.x + json.width / 2,
    y: json.y + json.height / 2,
    width: json.width,
    height: json.height,
    isZeroRadius: false,
    ident: undefined,
  };
}

// View element toJson functions

function viewElementToJson(element: ViewElement): JsonViewElement {
  switch (element.type) {
    case 'aux':
      return {
        type: 'aux',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };
    case 'stock':
      return {
        type: 'stock',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };
    case 'flow':
      return {
        type: 'flow',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        points: element.points.map((p) => pointToJson(p)),
        labelSide: element.labelSide,
      };
    case 'link': {
      const result: JsonLinkViewElement = {
        type: 'link',
        uid: element.uid,
        fromUid: element.fromUid,
        toUid: element.toUid,
      };
      if (element.arc !== undefined) {
        result.arc = element.arc;
      } else if (element.multiPoint) {
        result.multiPoints = element.multiPoint.map((p) => ({ x: p.x, y: p.y }));
      }
      if (element.polarity !== undefined) {
        result.polarity = element.polarity;
      }
      return result;
    }
    case 'module':
      return {
        type: 'module',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };
    case 'alias':
      return {
        type: 'alias',
        uid: element.uid,
        aliasOfUid: element.aliasOfUid,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };
    case 'cloud':
      return {
        type: 'cloud',
        uid: element.uid,
        flowUid: element.flowUid,
        x: element.x,
        y: element.y,
      };
    case 'group':
      // Convert back to XMILE's top-left convention for serialization
      return {
        type: 'group',
        uid: element.uid,
        name: element.name,
        x: element.x - element.width / 2,
        y: element.y - element.height / 2,
        width: element.width,
        height: element.height,
      };
  }
}

// Rect

export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export function rectFromJson(json: JsonRect): Rect {
  return { x: json.x, y: json.y, width: json.width, height: json.height };
}

export function rectToJson(rect: Rect): JsonRect {
  return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
}

export function rectDefault(): Rect {
  return { x: 0, y: 0, width: 0, height: 0 };
}

// StockFlowView

export interface StockFlowView {
  readonly nextUid: number;
  readonly elements: readonly ViewElement[];
  readonly viewBox: Rect;
  readonly zoom: number;
  readonly useLetteredPolarity: boolean;
}

// Coerce a coordinate to a finite number, repairing null/undefined/NaN/Infinity
// (which can arrive via imported models, hand-edited files, or older bugs) to a
// safe default. A non-finite coordinate serializes to JSON `null`, which the
// engine's patch parser rejects, so a model carrying one would otherwise be
// permanently uneditable (issue #818). Repairing on load keeps such a model
// usable; the affected element lands at the fallback position (visibly flagging
// that something was off) instead of bricking the editor.
function finiteCoord(v: number, fallback = 0): number {
  return Number.isFinite(v) ? v : fallback;
}

// Repairs any non-finite coordinate on a freshly-deserialized view element,
// returning the element UNCHANGED (same reference) for the common all-finite
// case so this is a true no-op for well-formed data (it runs on every model
// load and engine round-trip). Only a corrupt element is rebuilt. See
// finiteCoord / issue #818.
function sanitizeElementCoords(el: ViewElement): ViewElement {
  const fin = (v: number): boolean => Number.isFinite(v);
  switch (el.type) {
    case 'aux':
    case 'stock':
    case 'module':
    case 'alias':
    case 'cloud':
      if (fin(el.x) && fin(el.y)) {
        return el;
      }
      return { ...el, x: finiteCoord(el.x), y: finiteCoord(el.y) };
    case 'flow':
      if (fin(el.x) && fin(el.y) && el.points.every((p) => fin(p.x) && fin(p.y))) {
        return el;
      }
      return {
        ...el,
        x: finiteCoord(el.x),
        y: finiteCoord(el.y),
        points: el.points.map((p) => ({ ...p, x: finiteCoord(p.x), y: finiteCoord(p.y) })),
      };
    case 'link': {
      const arcOk = el.arc === undefined || fin(el.arc);
      const multiOk = !el.multiPoint || el.multiPoint.every((p) => fin(p.x) && fin(p.y));
      if (arcOk && multiOk) {
        return el;
      }
      return {
        ...el,
        // A non-finite arc becomes a straight link rather than a broken curve.
        arc: arcOk ? el.arc : undefined,
        multiPoint: el.multiPoint?.map((p) => ({ ...p, x: finiteCoord(p.x), y: finiteCoord(p.y) })),
      };
    }
    case 'group':
      if (fin(el.x) && fin(el.y) && fin(el.width) && fin(el.height)) {
        return el;
      }
      return {
        ...el,
        x: finiteCoord(el.x),
        y: finiteCoord(el.y),
        width: finiteCoord(el.width),
        height: finiteCoord(el.height),
      };
  }
}

export function stockFlowViewFromJson(json: JsonView, variables: ReadonlyMap<string, Variable>): StockFlowView {
  let maxUid = -1;
  const namedElements = new Map<string, UID>();

  const rawElements: ViewElement[] = (json.elements ?? []).map((element: JsonViewElement) => {
    let e: ViewElement;
    const ident = 'name' in element ? canonicalize(element.name) : undefined;
    const variable = ident ? variables.get(ident) : undefined;

    switch (element.type) {
      case 'aux':
        e = auxViewElementFromJson(element as JsonAuxiliaryViewElement, variable);
        if (ident) namedElements.set(ident, e.uid);
        break;
      case 'stock':
        e = stockViewElementFromJson(element as JsonStockViewElement, variable);
        if (ident) namedElements.set(ident, e.uid);
        break;
      case 'flow':
        e = flowViewElementFromJson(element as JsonFlowViewElement, variable);
        if (ident) namedElements.set(ident, e.uid);
        break;
      case 'link':
        e = linkViewElementFromJson(element as JsonLinkViewElement);
        break;
      case 'module':
        e = moduleViewElementFromJson(element as JsonModuleViewElement, variable);
        if (ident) namedElements.set(ident, e.uid);
        break;
      case 'alias':
        e = aliasViewElementFromJson(element as JsonAliasViewElement);
        break;
      case 'cloud':
        e = cloudViewElementFromJson(element as JsonCloudViewElement);
        break;
      case 'group':
        e = groupViewElementFromJson(element as JsonGroupViewElement);
        break;
      default:
        throw new Error(`unknown view element type: ${(element as JsonViewElement).type}`);
    }
    maxUid = Math.max(e.uid, maxUid);
    return e;
  });

  const elements: readonly ViewElement[] = rawElements.map((rawElement) => {
    // Repair any non-finite coordinate before the element enters the live model
    // (issue #818); a no-op for well-formed data.
    const element = sanitizeElementCoords(rawElement);
    if (element.type === 'stock' && element.var) {
      const stock = element.var;
      const inflows: readonly UID[] = stock.inflows
        .filter((ident: string) => namedElements.has(ident))
        .map((ident: string) => defined(namedElements.get(ident)));
      const outflows: readonly UID[] = stock.outflows
        .filter((ident: string) => namedElements.has(ident))
        .map((ident: string) => defined(namedElements.get(ident)));
      return { ...element, inflows, outflows };
    }
    return element;
  });

  let nextUid = maxUid + 1;
  if (nextUid === 0) {
    nextUid = 1;
  }

  const rawViewBox = json.viewBox ? rectFromJson(json.viewBox) : rectDefault();
  const viewBox: Rect = {
    x: finiteCoord(rawViewBox.x),
    y: finiteCoord(rawViewBox.y),
    width: finiteCoord(rawViewBox.width),
    height: finiteCoord(rawViewBox.height),
  };

  return {
    nextUid,
    elements,
    viewBox,
    zoom: finiteCoord(json.zoom ?? 1, 1),
    useLetteredPolarity: json.useLetteredPolarity ?? false,
  };
}

export function stockFlowViewToJson(view: StockFlowView): JsonView {
  const elements: JsonViewElement[] = view.elements.map((element) => viewElementToJson(element));

  const result: JsonView = {
    elements,
  };

  if (view.viewBox && (view.viewBox.width > 0 || view.viewBox.height > 0)) {
    result.viewBox = rectToJson(view.viewBox);
  }

  if (view.zoom > 0) {
    result.zoom = view.zoom;
  }

  if (view.useLetteredPolarity) {
    result.useLetteredPolarity = true;
  }

  return result;
}

/**
 * Returns a human-readable description of the first non-finite coordinate found
 * in the view (an element x/y, a flow point, a link arc/multipoint, a group's
 * size, or the viewBox/zoom), or undefined if every coordinate is finite.
 *
 * A non-finite coordinate (NaN/Infinity) serializes to JSON `null`, which the
 * engine's patch parser rejects with "invalid type: null, expected f64" -- which
 * historically bricked a model (every subsequent edit failed). Callers use this
 * to refuse a corrupt view update before it reaches the engine (issue #818).
 */
export function findNonFiniteViewCoord(view: StockFlowView): string | undefined {
  const check = (label: string, ...values: number[]): string | undefined => {
    for (const v of values) {
      if (!Number.isFinite(v)) {
        return `${label}=${v}`;
      }
    }
    return undefined;
  };

  for (const el of view.elements) {
    let bad: string | undefined;
    switch (el.type) {
      case 'aux':
      case 'stock':
      case 'module':
      case 'alias':
      case 'cloud':
        bad = check(`${el.type} uid=${el.uid} x/y`, el.x, el.y);
        break;
      case 'flow':
        bad = check(`flow uid=${el.uid} valve`, el.x, el.y);
        for (let i = 0; !bad && i < el.points.length; i++) {
          const p = el.points[i];
          bad = check(`flow uid=${el.uid} point[${i}]`, p.x, p.y);
        }
        break;
      case 'link':
        if (el.arc !== undefined) {
          bad = check(`link uid=${el.uid} arc`, el.arc);
        }
        if (!bad && el.multiPoint) {
          for (let i = 0; !bad && i < el.multiPoint.length; i++) {
            const p = el.multiPoint[i];
            bad = check(`link uid=${el.uid} multiPoint[${i}]`, p.x, p.y);
          }
        }
        break;
      case 'group':
        bad = check(`group uid=${el.uid} x/y/w/h`, el.x, el.y, el.width, el.height);
        break;
    }
    if (bad) {
      return bad;
    }
  }

  return (
    check('viewBox', view.viewBox.x, view.viewBox.y, view.viewBox.width, view.viewBox.height) ??
    check('zoom', view.zoom)
  );
}

// LoopMetadata

export interface LoopMetadata {
  readonly uids: readonly number[];
  readonly deleted: boolean;
  readonly name: string;
  readonly description: string;
}

export function loopMetadataFromJson(json: JsonLoopMetadata): LoopMetadata {
  return {
    uids: json.uids,
    deleted: json.deleted ?? false,
    name: json.name,
    description: json.description ?? '',
  };
}

export function loopMetadataToJson(lm: LoopMetadata): JsonLoopMetadata {
  const result: JsonLoopMetadata = {
    uids: [...lm.uids],
    name: lm.name,
  };
  if (lm.deleted) {
    result.deleted = lm.deleted;
  }
  if (lm.description) {
    result.description = lm.description;
  }
  return result;
}

// MacroSpec

export interface MacroSpec {
  readonly parameters: readonly string[];
  readonly primaryOutput: string;
  readonly additionalOutputs: readonly string[];
}

export function macroSpecFromJson(json: JsonMacroSpec): MacroSpec {
  return {
    parameters: json.parameters,
    primaryOutput: json.primaryOutput,
    additionalOutputs: json.additionalOutputs ?? [],
  };
}

export function macroSpecToJson(spec: MacroSpec): JsonMacroSpec {
  const result: JsonMacroSpec = {
    parameters: [...spec.parameters],
    primaryOutput: spec.primaryOutput,
  };
  if (spec.additionalOutputs.length > 0) {
    result.additionalOutputs = [...spec.additionalOutputs];
  }
  return result;
}

// ModelGroup

export interface ModelGroup {
  readonly name: string;
  readonly doc: string | undefined;
  readonly parent: string | undefined;
  readonly members: readonly string[];
  readonly runEnabled: boolean;
}

export function modelGroupFromJson(json: JsonModelGroup): ModelGroup {
  return {
    name: json.name,
    doc: json.doc,
    parent: json.parent,
    members: json.members ?? [],
    runEnabled: json.runEnabled ?? false,
  };
}

export function modelGroupToJson(group: ModelGroup): JsonModelGroup {
  const result: JsonModelGroup = {
    name: group.name,
    members: [...group.members],
  };
  if (group.doc) {
    result.doc = group.doc;
  }
  if (group.parent) {
    result.parent = group.parent;
  }
  if (group.runEnabled) {
    result.runEnabled = group.runEnabled;
  }
  return result;
}

// Model

export interface Model {
  readonly name: string;
  readonly variables: ReadonlyMap<string, Variable>;
  readonly views: readonly StockFlowView[];
  readonly loopMetadata: readonly LoopMetadata[];
  readonly groups: readonly ModelGroup[];
  readonly macroSpec?: MacroSpec;
}

export function modelFromJson(json: JsonModel): Model {
  const variables = new Map<string, Variable>(
    [
      ...(json.stocks ?? []).map((s: JsonStock) => stockFromJson(s) as Variable),
      ...(json.flows ?? []).map((f: JsonFlow) => flowFromJson(f) as Variable),
      ...(json.auxiliaries ?? []).map((a: JsonAuxiliary) => auxFromJson(a) as Variable),
      ...(json.modules ?? []).map((m: JsonModule) => moduleFromJson(m) as Variable),
    ].map((v: Variable) => [v.ident, v] as [string, Variable]),
  );

  return {
    name: json.name,
    variables,
    views: (json.views ?? []).map((view: JsonView) => stockFlowViewFromJson(view, variables)),
    loopMetadata: (json.loopMetadata ?? []).map((lm: JsonLoopMetadata) => loopMetadataFromJson(lm)),
    groups: (json.groups ?? []).map((g: JsonModelGroup) => modelGroupFromJson(g)),
    macroSpec: json.macroSpec ? macroSpecFromJson(json.macroSpec) : undefined,
  };
}

export function modelToJson(model: Model): JsonModel {
  const stocks: JsonStock[] = [];
  const flows: JsonFlow[] = [];
  const auxiliaries: JsonAuxiliary[] = [];
  const modules: JsonModule[] = [];

  for (const variable of model.variables.values()) {
    const json = variableToJson(variable);
    switch (variable.type) {
      case 'stock':
        stocks.push(json as JsonStock);
        break;
      case 'flow':
        flows.push(json as JsonFlow);
        break;
      case 'aux':
        auxiliaries.push(json as JsonAuxiliary);
        break;
      case 'module':
        modules.push(json as JsonModule);
        break;
    }
  }

  const result: JsonModel = {
    name: model.name,
    stocks,
    flows,
    auxiliaries,
  };

  if (modules.length > 0) {
    result.modules = modules;
  }
  if (model.views.length > 0) {
    result.views = model.views.map((v: StockFlowView) => stockFlowViewToJson(v));
  }
  if (model.loopMetadata.length > 0) {
    result.loopMetadata = model.loopMetadata.map((lm: LoopMetadata) => loopMetadataToJson(lm));
  }
  if (model.groups.length > 0) {
    result.groups = model.groups.map((g: ModelGroup) => modelGroupToJson(g));
  }
  if (model.macroSpec) {
    result.macroSpec = macroSpecToJson(model.macroSpec);
  }

  return result;
}

// Dt

export interface Dt {
  readonly value: number;
  readonly isReciprocal: boolean;
}

export function dtFromJson(dt: string): Dt {
  if (dt.startsWith('1/')) {
    const value = parseFloat(dt.substring(2));
    return { value, isReciprocal: true };
  }
  return { value: parseFloat(dt), isReciprocal: false };
}

export function dtToJson(dt: Dt): string {
  if (dt.isReciprocal) {
    return `1/${dt.value}`;
  }
  return String(dt.value);
}

export function dtDefault(): Dt {
  return { value: 1, isReciprocal: false };
}

// SimSpecs

export type SimMethod = 'euler' | 'rk2' | 'rk4';

export interface SimSpecs {
  readonly start: number;
  readonly stop: number;
  readonly dt: Dt;
  readonly saveStep: Dt | undefined;
  readonly simMethod: SimMethod;
  readonly timeUnits: string | undefined;
}

export function simSpecsFromJson(json: JsonSimSpecs): SimSpecs {
  return {
    start: json.startTime,
    stop: json.endTime,
    dt: dtFromJson(json.dt ?? '1'),
    saveStep: json.saveStep ? { value: json.saveStep, isReciprocal: false } : undefined,
    simMethod: (json.method ?? 'euler') as SimMethod,
    timeUnits: json.timeUnits,
  };
}

export function simSpecsToJson(specs: SimSpecs): JsonSimSpecs {
  const result: JsonSimSpecs = {
    startTime: specs.start,
    endTime: specs.stop,
    dt: dtToJson(specs.dt),
  };
  if (specs.saveStep) {
    result.saveStep = specs.saveStep.isReciprocal ? 1 / specs.saveStep.value : specs.saveStep.value;
  }
  if (specs.simMethod && specs.simMethod !== 'euler') {
    result.method = specs.simMethod;
  }
  if (specs.timeUnits) {
    result.timeUnits = specs.timeUnits;
  }
  return result;
}

export function simSpecsDefault(): SimSpecs {
  return {
    start: 0,
    stop: 10,
    dt: dtDefault(),
    saveStep: undefined,
    simMethod: 'euler',
    timeUnits: undefined,
  };
}

// Dimension

export interface Dimension {
  readonly name: string;
  readonly subscripts: readonly string[];
}

export function dimensionFromJson(json: JsonDimension): Dimension {
  return {
    name: json.name,
    subscripts: json.elements ?? [],
  };
}

export function dimensionToJson(dim: Dimension): JsonDimension {
  const result: JsonDimension = {
    name: dim.name,
  };
  if (dim.subscripts.length > 0) {
    result.elements = [...dim.subscripts];
  }
  return result;
}

// Source

export type Extension = 'xmile' | 'vensim' | undefined;

export interface Source {
  readonly extension: Extension;
  readonly content: string;
}

export function sourceFromJson(json: JsonSource): Source {
  let extension: Extension;
  if (json.extension === 'xmile') {
    extension = 'xmile';
  } else if (json.extension === 'vensim') {
    extension = 'vensim';
  } else {
    extension = undefined;
  }
  return {
    extension,
    content: json.content ?? '',
  };
}

export function sourceToJson(source: Source): JsonSource {
  const result: JsonSource = {};
  if (source.extension) {
    result.extension = source.extension;
  }
  if (source.content) {
    result.content = source.content;
  }
  return result;
}

// Project

export interface Project {
  readonly name: string;
  readonly simSpecs: SimSpecs;
  readonly models: ReadonlyMap<string, Model>;
  readonly dimensions: ReadonlyMap<string, Dimension>;
  readonly hasNoEquations: boolean;
  readonly source: Source | undefined;
}

export function projectFromJson(json: JsonProject): Project {
  return {
    name: json.name,
    simSpecs: simSpecsFromJson(json.simSpecs),
    models: new Map<string, Model>(
      json.models.map((model: JsonModel) => [model.name, modelFromJson(model)] as [string, Model]),
    ),
    dimensions: new Map<string, Dimension>(
      (json.dimensions ?? []).map((dim: JsonDimension) => [dim.name, dimensionFromJson(dim)] as [string, Dimension]),
    ),
    hasNoEquations: false,
    source: json.source ? sourceFromJson(json.source) : undefined,
  };
}

export function projectToJson(project: Project): JsonProject {
  const result: JsonProject = {
    name: project.name,
    simSpecs: simSpecsToJson(project.simSpecs),
    models: [...project.models.values()].map((m: Model) => modelToJson(m)),
  };
  if (project.dimensions.size > 0) {
    result.dimensions = [...project.dimensions.values()].map((d: Dimension) => dimensionToJson(d));
  }
  if (project.source) {
    result.source = sourceToJson(project.source);
  }
  return result;
}

export function projectAttachData(project: Project, data: ReadonlyMap<string, Series>, modelName: string): Project {
  const model = defined(project.models.get(modelName));

  // Group every result series by its base variable ident. A scalar variable's
  // series is keyed by the bare canonical ident; an arrayed variable's
  // per-element series are keyed `ident[<canonical subscripts>]` for any
  // dimensionality (1-D `x[a]`, multi-D `x[a,b]`). Grouping by the ident before
  // the first `[` attaches every element series -- so multi-dimensional
  // variables are plotted too -- and matches whatever the simulation emitted
  // rather than reconstructing keys from a Dimension's (original-case)
  // subscripts, which avoids the element-name canonicalization mismatch
  // entirely.
  const seriesByIdent = new Map<string, Series[]>();
  for (const [key, s] of data) {
    const open = key.indexOf('[');
    const ident = open === -1 ? key : key.slice(0, open);
    const existing = seriesByIdent.get(ident);
    if (existing) {
      existing.push(s);
    } else {
      seriesByIdent.set(ident, [s]);
    }
  }

  const variables = mapValues(model.variables, (v: Variable) => {
    const series = seriesByIdent.get(v.ident);
    if (!series || series.length === 0) {
      return v;
    }
    return { ...v, data: series };
  });
  const updatedModel: Model = { ...model, variables };
  return { ...project, models: mapSet(project.models, modelName, updatedModel) };
}
