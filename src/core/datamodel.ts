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

export interface ArrayedEquation {
  readonly type: 'arrayed';
  readonly dimensionNames: readonly string[];
  readonly elements: ReadonlyMap<string, string>;
}

export type Equation = ScalarEquation | ApplyToAllEquation | ArrayedEquation;

function stockEquationFromJson(
  initialEquation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
): Equation {
  if (arrayedEquation) {
    const dimensionNames: readonly string[] = arrayedEquation.dimensions ?? [];
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return {
        type: 'arrayed',
        dimensionNames,
        elements: new Map<string, string>(
          arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string]),
        ),
      };
    } else {
      return {
        type: 'applyToAll',
        dimensionNames,
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
    const dimensionNames: readonly string[] = arrayedEquation.dimensions ?? [];
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return {
        equation: {
          type: 'arrayed',
          dimensionNames,
          elements: new Map<string, string>(
            arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string]),
          ),
        },
        graphicalFunction,
      };
    } else {
      return {
        equation: {
          type: 'applyToAll',
          dimensionNames,
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
    return {
      arrayedEquation: {
        dimensions: [...equation.dimensionNames],
        elements: [...equation.elements].map(([subscript, eqn]) => ({ subscript, equation: eqn })),
      },
    };
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
    return {
      arrayedEquation: {
        dimensions: [...equation.dimensionNames],
        elements: [...equation.elements].map(([subscript, eqn]) => ({ subscript, equation: eqn })),
      },
    };
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
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
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
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  readonly uid: number | undefined;
}

export interface Aux {
  readonly type: 'aux';
  readonly ident: string;
  readonly equation: Equation;
  readonly documentation: string;
  readonly units: string;
  readonly gf: GraphicalFunction | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
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
  readonly data: Readonly<Array<Series>> | undefined;
  readonly errors: readonly EquationError[] | undefined;
  readonly unitErrors: readonly UnitError[] | undefined;
  readonly uid: number | undefined;
}

export type Variable = Stock | Flow | Aux | Module;

export function variableIsArrayed(v: Variable): boolean {
  if (v.type === 'module') return false;
  return v.equation.type === 'applyToAll' || v.equation.type === 'arrayed';
}

export function variableHasError(v: Variable): boolean {
  return v.errors !== undefined || v.unitErrors !== undefined;
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
    nonNegative: json.nonNegative ?? false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
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
    result.nonNegative = stock.nonNegative;
  }
  if (stock.documentation) {
    result.documentation = stock.documentation;
  }
  return result;
}

export function flowFromJson(json: JsonFlow): Flow {
  const { equation, graphicalFunction } = auxEquationFromJson(json.equation, json.arrayedEquation, json.graphicalFunction);
  return {
    type: 'flow',
    ident: canonicalize(json.name),
    equation,
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    gf: graphicalFunction,
    nonNegative: json.nonNegative ?? false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
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
    result.nonNegative = flow.nonNegative;
  }
  if (flow.documentation) {
    result.documentation = flow.documentation;
  }
  return result;
}

export function auxFromJson(json: JsonAuxiliary): Aux {
  const { equation, graphicalFunction } = auxEquationFromJson(json.equation, json.arrayedEquation, json.graphicalFunction);
  return {
    type: 'aux',
    ident: canonicalize(json.name),
    equation,
    documentation: json.documentation ?? '',
    units: json.units ?? '',
    gf: graphicalFunction,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
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
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
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

export function flowViewElementFromJson(
  json: JsonFlowViewElement,
  flowVar?: Variable | undefined,
): FlowViewElement {
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

  const elements: readonly ViewElement[] = rawElements.map((element) => {
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

  const viewBox = json.viewBox ? rectFromJson(json.viewBox) : rectDefault();

  return {
    nextUid,
    elements,
    viewBox,
    zoom: json.zoom ?? 1,
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
    models: new Map<string, Model>(json.models.map((model: JsonModel) => [model.name, modelFromJson(model)] as [string, Model])),
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
  const variables = mapValues(model.variables, (v: Variable) => {
    if (data.has(v.ident)) {
      return { ...v, data: [defined(data.get(v.ident))] };
    }
    if (!variableIsArrayed(v)) {
      return v;
    }
    const eqn = variableEquation(v);
    if (!eqn || (eqn.type !== 'applyToAll' && eqn.type !== 'arrayed')) {
      return v;
    }
    const dimNames = eqn.dimensionNames;
    if (dimNames.length !== 1) {
      return v;
    }
    const ident = v.ident;
    const dim = defined(project.dimensions.get(dimNames[0]));
    const series = dim.subscripts
      .map((element) => data.get(`${ident}[${element}]`))
      .filter((d) => d !== undefined)
      .map((d) => defined(d));

    return { ...v, data: series };
  });
  const updatedModel: Model = { ...model, variables };
  return { ...project, models: mapSet(project.models, modelName, updatedModel) };
}
