// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined, Series } from './common';

import { List, Map, Record } from 'immutable';

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
  type JsonSource,
} from '@system-dynamics/engine2';

export { ErrorCode };

export type UID = number;

const equationErrorDefaults = {
  code: ErrorCode.NoError,
  start: 0.0,
  end: 0.0,
};
export class EquationError extends Record(equationErrorDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof equationErrorDefaults) {
    super(props);
  }
}

const unitErrorDefaults = {
  code: ErrorCode.NoError,
  start: 0.0,
  end: 0.0,
  isConsistencyError: false,
  details: undefined as string | undefined,
};
export class UnitError extends Record(unitErrorDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof unitErrorDefaults) {
    super(props);
  }
}

const simErrorDefaults = {
  code: ErrorCode.NoError,
  details: undefined as string | undefined,
};
export class SimError extends Record(simErrorDefaults) {}

const modelErrorDefaults = {
  code: ErrorCode.NoError,
  details: undefined as string | undefined,
};
export class ModelError extends Record(modelErrorDefaults) {}

export type GraphicalFunctionKind = 'continuous' | 'extrapolate' | 'discrete';

const graphicalFunctionScaleDefaults = {
  min: 0.0,
  max: 0.0,
};
export class GraphicalFunctionScale extends Record(graphicalFunctionScaleDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof graphicalFunctionScaleDefaults) {
    super(props);
  }
  static default(): GraphicalFunctionScale {
    return new GraphicalFunctionScale(graphicalFunctionScaleDefaults);
  }
  static fromJson(json: JsonGraphicalFunctionScale): GraphicalFunctionScale {
    return new GraphicalFunctionScale({
      min: json.min,
      max: json.max,
    });
  }
  toJson(): JsonGraphicalFunctionScale {
    return {
      min: this.min,
      max: this.max,
    };
  }
}

const graphicalFunctionDefaults = {
  kind: 'continuous' as GraphicalFunctionKind,
  xPoints: undefined as List<number> | undefined,
  yPoints: List<number>(),
  xScale: GraphicalFunctionScale.default(),
  yScale: GraphicalFunctionScale.default(),
};

export class GraphicalFunction extends Record(graphicalFunctionDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof graphicalFunctionDefaults) {
    super(props);
  }

  static fromJson(json: JsonGraphicalFunction): GraphicalFunction {
    let xPoints: List<number> | undefined;
    let yPoints: List<number>;

    if (json.points && json.points.length > 0) {
      xPoints = List(json.points.map((p: [number, number]) => p[0]));
      yPoints = List(json.points.map((p: [number, number]) => p[1]));
    } else {
      xPoints = undefined;
      yPoints = List(json.yPoints ?? []);
    }

    const xScale = json.xScale
      ? GraphicalFunctionScale.fromJson(json.xScale)
      : new GraphicalFunctionScale({ min: 0, max: Math.max(0, yPoints.size - 1) });
    const yScale = json.yScale
      ? GraphicalFunctionScale.fromJson(json.yScale)
      : GraphicalFunctionScale.default();

    let kind: GraphicalFunctionKind = 'continuous';
    if (json.kind === 'discrete') {
      kind = 'discrete';
    } else if (json.kind === 'extrapolate') {
      kind = 'extrapolate';
    }

    return new GraphicalFunction({
      kind,
      xPoints,
      yPoints,
      xScale,
      yScale,
    });
  }
  toJson(): JsonGraphicalFunction {
    const result: JsonGraphicalFunction = {};

    if (this.xPoints && this.xPoints.size > 0) {
      result.points = this.xPoints
        .zip(this.yPoints)
        .map(([x, y]) => [x, y] as [number, number])
        .toArray();
    } else {
      result.yPoints = this.yPoints.toArray();
    }

    if (this.kind && this.kind !== 'continuous') {
      result.kind = this.kind;
    }

    if (this.xScale) {
      result.xScale = this.xScale.toJson();
    }
    if (this.yScale) {
      result.yScale = this.yScale.toJson();
    }

    return result;
  }
}

export type Equation = ScalarEquation | ApplyToAllEquation | ArrayedEquation;

const scalarEquationDefaults = {
  equation: '',
};
export class ScalarEquation extends Record(scalarEquationDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof scalarEquationDefaults) {
    super(props);
  }
  static default(): ScalarEquation {
    return new ScalarEquation({
      equation: '',
    });
  }
}

const applyToAllEquationDefaults = {
  dimensionNames: List<string>(),
  equation: '',
};
export class ApplyToAllEquation extends Record(applyToAllEquationDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof applyToAllEquationDefaults) {
    super(props);
  }
}

const arrayedEquationDefaults = {
  dimensionNames: List<string>(),
  elements: Map<string, string>(),
};
export class ArrayedEquation extends Record(arrayedEquationDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof arrayedEquationDefaults) {
    super(props);
  }
}

function stockEquationFromJson(
  initialEquation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
): Equation {
  if (arrayedEquation) {
    const dimensionNames = List(arrayedEquation.dimensions ?? []);
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return new ArrayedEquation({
        dimensionNames,
        elements: Map<string, string>(arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string])),
      });
    } else {
      return new ApplyToAllEquation({
        dimensionNames,
        equation: arrayedEquation.equation ?? '',
      });
    }
  }
  return new ScalarEquation({ equation: initialEquation ?? '' });
}

function auxEquationFromJson(
  equation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
  gf: JsonGraphicalFunction | undefined,
): { equation: Equation; graphicalFunction: GraphicalFunction | undefined } {
  let graphicalFunction: GraphicalFunction | undefined;
  if (gf) {
    graphicalFunction = GraphicalFunction.fromJson(gf);
  }

  if (arrayedEquation) {
    const dimensionNames = List(arrayedEquation.dimensions ?? []);
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return {
        equation: new ArrayedEquation({
          dimensionNames,
          elements: Map<string, string>(arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string])),
        }),
        graphicalFunction,
      };
    } else {
      return {
        equation: new ApplyToAllEquation({
          dimensionNames,
          equation: arrayedEquation.equation ?? '',
        }),
        graphicalFunction,
      };
    }
  }
  return {
    equation: new ScalarEquation({ equation: equation ?? '' }),
    graphicalFunction,
  };
}

function stockEquationToJson(equation: Equation): { initialEquation?: string; arrayedEquation?: JsonArrayedEquation } {
  if (equation instanceof ScalarEquation) {
    return { initialEquation: equation.equation || undefined };
  } else if (equation instanceof ApplyToAllEquation) {
    return {
      arrayedEquation: {
        dimensions: equation.dimensionNames.toArray(),
        equation: equation.equation || undefined,
      },
    };
  } else if (equation instanceof ArrayedEquation) {
    return {
      arrayedEquation: {
        dimensions: equation.dimensionNames.toArray(),
        elements: equation.elements
          .map((eqn, subscript) => ({ subscript, equation: eqn }))
          .valueSeq()
          .toArray(),
      },
    };
  }
  return {};
}

function auxEquationToJson(equation: Equation): { equation?: string; arrayedEquation?: JsonArrayedEquation } {
  if (equation instanceof ScalarEquation) {
    return { equation: equation.equation || undefined };
  } else if (equation instanceof ApplyToAllEquation) {
    return {
      arrayedEquation: {
        dimensions: equation.dimensionNames.toArray(),
        equation: equation.equation || undefined,
      },
    };
  } else if (equation instanceof ArrayedEquation) {
    return {
      arrayedEquation: {
        dimensions: equation.dimensionNames.toArray(),
        elements: equation.elements
          .map((eqn, subscript) => ({ subscript, equation: eqn }))
          .valueSeq()
          .toArray(),
      },
    };
  }
  return {};
}

export interface Variable {
  readonly ident: string;
  readonly equation: Equation | undefined;
  readonly gf: GraphicalFunction | undefined;
  readonly units: string;
  readonly documentation: string;
  readonly isArrayed: boolean;
  readonly hasError: boolean;
  readonly errors: List<EquationError> | undefined;
  readonly unitErrors: List<UnitError> | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  set(prop: 'errors', errors: List<EquationError> | undefined): Variable;
  set(prop: 'unitErrors', errors: List<UnitError> | undefined): Variable;
  set(prop: 'data', data: Readonly<Array<Series>> | undefined): Variable;
}

const stockDefaults = {
  ident: '',
  equation: ScalarEquation.default() as Equation,
  documentation: '',
  units: '',
  inflows: List<string>(),
  outflows: List<string>(),
  nonNegative: false,
  data: undefined as Readonly<Array<Series>> | undefined,
  errors: undefined as List<EquationError> | undefined,
  unitErrors: undefined as List<UnitError> | undefined,
  uid: undefined as number | undefined,
};
export class Stock extends Record(stockDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof stockDefaults) {
    super(props);
  }
  static fromJson(json: JsonStock): Stock {
    return new Stock({
      ident: canonicalize(json.name),
      equation: stockEquationFromJson(json.initialEquation, json.arrayedEquation),
      documentation: json.documentation ?? '',
      units: json.units ?? '',
      inflows: List(json.inflows ?? []),
      outflows: List(json.outflows ?? []),
      nonNegative: json.nonNegative ?? false,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: json.uid,
    });
  }
  toJson(): JsonStock {
    const eqJson = stockEquationToJson(this.equation);
    const result: JsonStock = {
      name: this.ident,
      inflows: this.inflows.toArray(),
      outflows: this.outflows.toArray(),
    };
    if (this.uid !== undefined) {
      result.uid = this.uid;
    }
    if (eqJson.initialEquation) {
      result.initialEquation = eqJson.initialEquation;
    }
    if (eqJson.arrayedEquation) {
      result.arrayedEquation = eqJson.arrayedEquation;
    }
    if (this.units) {
      result.units = this.units;
    }
    if (this.nonNegative) {
      result.nonNegative = this.nonNegative;
    }
    if (this.documentation) {
      result.documentation = this.documentation;
    }
    return result;
  }
  get gf(): undefined {
    return undefined;
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined || this.unitErrors !== undefined;
  }
}

const flowDefaults = {
  ident: '',
  equation: ScalarEquation.default() as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  nonNegative: false,
  data: undefined as Readonly<Array<Series>> | undefined,
  errors: undefined as List<EquationError> | undefined,
  unitErrors: undefined as List<UnitError> | undefined,
  uid: undefined as number | undefined,
};
export class Flow extends Record(flowDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof flowDefaults) {
    super(props);
  }
  static fromJson(json: JsonFlow): Flow {
    const { equation, graphicalFunction } = auxEquationFromJson(
      json.equation,
      json.arrayedEquation,
      json.graphicalFunction,
    );
    return new Flow({
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
    });
  }
  toJson(): JsonFlow {
    const eqJson = auxEquationToJson(this.equation);
    const result: JsonFlow = {
      name: this.ident,
    };
    if (this.uid !== undefined) {
      result.uid = this.uid;
    }
    if (eqJson.equation) {
      result.equation = eqJson.equation;
    }
    if (eqJson.arrayedEquation) {
      result.arrayedEquation = eqJson.arrayedEquation;
    }
    if (this.gf) {
      result.graphicalFunction = this.gf.toJson();
    }
    if (this.units) {
      result.units = this.units;
    }
    if (this.nonNegative) {
      result.nonNegative = this.nonNegative;
    }
    if (this.documentation) {
      result.documentation = this.documentation;
    }
    return result;
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined || this.unitErrors !== undefined;
  }
}

const auxDefaults = {
  ident: '',
  equation: ScalarEquation.default() as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  data: undefined as Readonly<Array<Series>> | undefined,
  errors: undefined as List<EquationError> | undefined,
  unitErrors: undefined as List<UnitError> | undefined,
  uid: undefined as number | undefined,
};
export class Aux extends Record(auxDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof auxDefaults) {
    super(props);
  }
  static fromJson(json: JsonAuxiliary): Aux {
    const { equation, graphicalFunction } = auxEquationFromJson(
      json.equation,
      json.arrayedEquation,
      json.graphicalFunction,
    );
    return new Aux({
      ident: canonicalize(json.name),
      equation,
      documentation: json.documentation ?? '',
      units: json.units ?? '',
      gf: graphicalFunction,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: json.uid,
    });
  }
  toJson(): JsonAuxiliary {
    const eqJson = auxEquationToJson(this.equation);
    const result: JsonAuxiliary = {
      name: this.ident,
    };
    if (this.uid !== undefined) {
      result.uid = this.uid;
    }
    if (eqJson.equation) {
      result.equation = eqJson.equation;
    }
    if (eqJson.arrayedEquation) {
      result.arrayedEquation = eqJson.arrayedEquation;
    }
    if (this.gf) {
      result.graphicalFunction = this.gf.toJson();
    }
    if (this.units) {
      result.units = this.units;
    }
    if (this.documentation) {
      result.documentation = this.documentation;
    }
    return result;
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined || this.unitErrors !== undefined;
  }
}

const moduleReferenceDefaults = {
  src: '',
  dst: '',
};
export class ModuleReference extends Record(moduleReferenceDefaults) {
  constructor(props: typeof moduleReferenceDefaults) {
    super(props);
  }
  static fromJson(json: JsonModuleReference): ModuleReference {
    return new ModuleReference({
      src: json.src,
      dst: json.dst,
    });
  }
  toJson(): JsonModuleReference {
    return {
      src: this.src,
      dst: this.dst,
    };
  }
}

const moduleDefaults = {
  ident: '',
  modelName: '',
  documentation: '',
  units: '',
  references: List<ModuleReference>(),
  data: undefined as Readonly<Array<Series>> | undefined,
  errors: undefined as List<EquationError> | undefined,
  unitErrors: undefined as List<UnitError> | undefined,
  uid: undefined as number | undefined,
};
export class Module extends Record(moduleDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof moduleDefaults) {
    super(props);
  }
  static fromJson(json: JsonModule): Module {
    return new Module({
      ident: canonicalize(json.name),
      modelName: json.modelName,
      documentation: json.documentation ?? '',
      units: json.units ?? '',
      references: List((json.references ?? []).map((ref: JsonModuleReference) => ModuleReference.fromJson(ref))),
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: json.uid,
    });
  }
  toJson(): JsonModule {
    const result: JsonModule = {
      name: this.ident,
      modelName: this.modelName,
    };
    if (this.uid !== undefined) {
      result.uid = this.uid;
    }
    if (this.references.size > 0) {
      result.references = this.references.map((ref) => ref.toJson()).toArray();
    }
    if (this.units) {
      result.units = this.units;
    }
    if (this.documentation) {
      result.documentation = this.documentation;
    }
    return result;
  }
  get equation(): undefined {
    return undefined;
  }
  get gf(): undefined {
    return undefined;
  }
  get isArrayed(): boolean {
    return false;
  }
  get hasError(): boolean {
    return this.errors !== undefined;
  }
}

export type LabelSide = 'top' | 'left' | 'center' | 'bottom' | 'right';

export interface ViewElement {
  readonly isZeroRadius: boolean;
  readonly uid: number;
  readonly cx: number;
  readonly cy: number;
  readonly ident: string | undefined;
  isNamed(): boolean;
  set(prop: 'uid', uid: number): ViewElement;
  set(prop: 'x', x: number): ViewElement;
  set(prop: 'y', x: number): ViewElement;
}

const auxViewElementDefaults = {
  uid: -1,
  name: '',
  ident: '',
  var: undefined as Aux | undefined,
  x: -1,
  y: -1,
  labelSide: 'right' as LabelSide,
  isZeroRadius: false,
};
export class AuxViewElement extends Record(auxViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof auxViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonAuxiliaryViewElement, auxVar?: Variable | undefined): AuxViewElement {
    const ident = canonicalize(json.name);
    return new AuxViewElement({
      uid: json.uid,
      name: json.name,
      ident,
      var: auxVar instanceof Aux ? auxVar : undefined,
      x: json.x,
      y: json.y,
      labelSide: (json.labelSide ?? 'right') as LabelSide,
      isZeroRadius: false,
    });
  }
  toJson(): JsonAuxiliaryViewElement {
    return {
      type: 'aux',
      uid: this.uid,
      name: this.name,
      x: this.x,
      y: this.y,
      labelSide: this.labelSide,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
}

const stockViewElementDefaults = {
  uid: -1,
  name: '',
  ident: '',
  var: undefined as Stock | undefined,
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
  inflows: List<UID>(),
  outflows: List<UID>(),
};
export class StockViewElement extends Record(stockViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof stockViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonStockViewElement, stockVar?: Variable | undefined): StockViewElement {
    const ident = canonicalize(json.name);
    return new StockViewElement({
      uid: json.uid,
      name: json.name,
      ident,
      var: stockVar instanceof Stock ? stockVar : undefined,
      x: json.x,
      y: json.y,
      labelSide: (json.labelSide ?? 'center') as LabelSide,
      isZeroRadius: false,
      inflows: List<UID>(),
      outflows: List<UID>(),
    });
  }
  toJson(): JsonStockViewElement {
    return {
      type: 'stock',
      uid: this.uid,
      name: this.name,
      x: this.x,
      y: this.y,
      labelSide: this.labelSide,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
}

const pointDefaults = {
  x: -1,
  y: -1,
  attachedToUid: undefined as number | undefined,
};
export class Point extends Record(pointDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof pointDefaults) {
    super(props);
  }
  static fromJson(json: JsonFlowPoint): Point {
    return new Point({
      x: json.x,
      y: json.y,
      attachedToUid: json.attachedToUid,
    });
  }
  toJson(): JsonFlowPoint {
    const result: JsonFlowPoint = {
      x: this.x,
      y: this.y,
    };
    if (this.attachedToUid !== undefined) {
      result.attachedToUid = this.attachedToUid;
    }
    return result;
  }
}

const flowViewElementDefaults = {
  uid: -1,
  name: '',
  ident: '',
  var: undefined as Flow | undefined,
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  points: List<Point>(),
  isZeroRadius: false,
};
export class FlowViewElement extends Record(flowViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof flowViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonFlowViewElement, flowVar?: Variable | undefined): FlowViewElement {
    const ident = canonicalize(json.name);
    return new FlowViewElement({
      uid: json.uid,
      name: json.name,
      ident,
      var: flowVar instanceof Flow ? flowVar : undefined,
      x: json.x,
      y: json.y,
      labelSide: (json.labelSide ?? 'center') as LabelSide,
      points: List((json.points ?? []).map((p: JsonFlowPoint) => Point.fromJson(p))),
      isZeroRadius: false,
    });
  }
  toJson(): JsonFlowViewElement {
    return {
      type: 'flow',
      uid: this.uid,
      name: this.name,
      x: this.x,
      y: this.y,
      points: this.points.map((p) => p.toJson()).toArray(),
      labelSide: this.labelSide,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
}

const linkViewElementDefaults = {
  uid: -1,
  fromUid: -1,
  toUid: -1,
  arc: undefined as number | undefined,
  isStraight: false,
  multiPoint: undefined as List<Point> | undefined,
};
export class LinkViewElement extends Record(linkViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof linkViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonLinkViewElement): LinkViewElement {
    let arc: number | undefined = undefined;
    let isStraight = false;
    let multiPoint: List<Point> | undefined = undefined;

    if (json.arc !== undefined) {
      arc = json.arc;
    } else if (json.multiPoints && json.multiPoints.length > 0) {
      multiPoint = List(json.multiPoints.map((p: JsonLinkPoint) => new Point({ x: p.x, y: p.y, attachedToUid: undefined })));
    } else {
      isStraight = true;
    }

    return new LinkViewElement({
      uid: json.uid,
      fromUid: json.fromUid,
      toUid: json.toUid,
      arc,
      isStraight,
      multiPoint,
    });
  }
  toJson(): JsonLinkViewElement {
    const result: JsonLinkViewElement = {
      type: 'link',
      uid: this.uid,
      fromUid: this.fromUid,
      toUid: this.toUid,
    };
    if (this.arc !== undefined) {
      result.arc = this.arc;
    } else if (this.multiPoint) {
      result.multiPoints = this.multiPoint.map((p) => ({ x: p.x, y: p.y })).toArray();
    }
    return result;
  }
  get cx(): number {
    return NaN;
  }
  get cy(): number {
    return NaN;
  }
  isNamed(): boolean {
    return false;
  }
  get ident(): undefined {
    return undefined;
  }
  get isZeroRadius(): boolean {
    return false;
  }
}

const moduleViewElementDefaults = {
  uid: -1,
  name: '',
  ident: '',
  var: undefined as Module | undefined,
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
};
export class ModuleViewElement extends Record(moduleViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof moduleViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonModuleViewElement, moduleVar?: Variable | undefined): ModuleViewElement {
    const ident = canonicalize(json.name);
    return new ModuleViewElement({
      uid: json.uid,
      name: json.name,
      ident,
      var: moduleVar instanceof Module ? moduleVar : undefined,
      x: json.x,
      y: json.y,
      labelSide: (json.labelSide ?? 'center') as LabelSide,
      isZeroRadius: false,
    });
  }
  toJson(): JsonModuleViewElement {
    return {
      type: 'module',
      uid: this.uid,
      name: this.name,
      x: this.x,
      y: this.y,
      labelSide: this.labelSide,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
}

const aliasViewElementDefaults = {
  uid: -1,
  aliasOfUid: -1,
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
};
export class AliasViewElement extends Record(aliasViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof aliasViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonAliasViewElement): AliasViewElement {
    return new AliasViewElement({
      uid: json.uid,
      aliasOfUid: json.aliasOfUid,
      x: json.x,
      y: json.y,
      labelSide: (json.labelSide ?? 'center') as LabelSide,
      isZeroRadius: false,
    });
  }
  toJson(): JsonAliasViewElement {
    return {
      type: 'alias',
      uid: this.uid,
      aliasOfUid: this.aliasOfUid,
      x: this.x,
      y: this.y,
      labelSide: this.labelSide,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return false;
  }
  get ident(): undefined {
    return undefined;
  }
}

const cloudViewElementDefaults = {
  uid: -1,
  flowUid: -1,
  x: -1,
  y: -1,
  isZeroRadius: false,
};
export class CloudViewElement extends Record(cloudViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof cloudViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonCloudViewElement): CloudViewElement {
    return new CloudViewElement({
      uid: json.uid,
      flowUid: json.flowUid,
      x: json.x,
      y: json.y,
      isZeroRadius: false,
    });
  }
  toJson(): JsonCloudViewElement {
    return {
      type: 'cloud',
      uid: this.uid,
      flowUid: this.flowUid,
      x: this.x,
      y: this.y,
    };
  }
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return false;
  }
  get ident(): undefined {
    return undefined;
  }
}

const groupViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  width: 100,
  height: 80,
  isZeroRadius: false,
};
export class GroupViewElement extends Record(groupViewElementDefaults) implements ViewElement {
  constructor(props: typeof groupViewElementDefaults) {
    super(props);
  }
  static fromJson(json: JsonGroupViewElement): GroupViewElement {
    return new GroupViewElement({
      uid: json.uid,
      name: json.name,
      x: json.x,
      y: json.y,
      width: json.width,
      height: json.height,
      isZeroRadius: false,
    });
  }
  toJson(): JsonGroupViewElement {
    return {
      type: 'group',
      uid: this.uid,
      name: this.name,
      x: this.x,
      y: this.y,
      width: this.width,
      height: this.height,
    };
  }
  // Return x/y directly (not the center) because selection move logic
  // computes new positions as element.cx - delta. Since groups store x/y
  // as the top-left corner (per XMILE spec), returning the center would
  // cause groups to jump by half their dimensions when dragged.
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
  get ident(): string {
    return this.name;
  }
}

export type NamedViewElement = StockViewElement | AuxViewElement | ModuleViewElement | FlowViewElement;

const rectDefaults = {
  x: -1,
  y: -1,
  width: -1,
  height: -1,
};
export class Rect extends Record(rectDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof rectDefaults) {
    super(props);
  }
  static fromJson(json: JsonRect): Rect {
    return new Rect({
      x: json.x,
      y: json.y,
      width: json.width,
      height: json.height,
    });
  }
  toJson(): JsonRect {
    return {
      x: this.x,
      y: this.y,
      width: this.width,
      height: this.height,
    };
  }

  static default(): Rect {
    return new Rect({
      x: 0,
      y: 0,
      width: 0,
      height: 0,
    });
  }
}

const stockFlowViewDefaults = {
  nextUid: -1,
  elements: List<ViewElement>(),
  viewBox: Rect.default(),
  zoom: -1,
};
export class StockFlowView extends Record(stockFlowViewDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof stockFlowViewDefaults) {
    super(props);
  }
  static fromJson(json: JsonView, variables: Map<string, Variable>): StockFlowView {
    let maxUid = -1;
    let namedElements = Map<string, UID>();

    const elements = List<ViewElement>(
      (json.elements ?? []).map((element: JsonViewElement) => {
        let e: ViewElement;
        const ident = 'name' in element ? canonicalize(element.name) : undefined;
        const variable = ident ? variables.get(ident) : undefined;

        switch (element.type) {
          case 'aux':
            e = AuxViewElement.fromJson(element as JsonAuxiliaryViewElement, variable);
            if (ident) namedElements = namedElements.set(ident, e.uid);
            break;
          case 'stock':
            e = StockViewElement.fromJson(element as JsonStockViewElement, variable);
            if (ident) namedElements = namedElements.set(ident, e.uid);
            break;
          case 'flow':
            e = FlowViewElement.fromJson(element as JsonFlowViewElement, variable);
            if (ident) namedElements = namedElements.set(ident, e.uid);
            break;
          case 'link':
            e = LinkViewElement.fromJson(element as JsonLinkViewElement);
            break;
          case 'module':
            e = ModuleViewElement.fromJson(element as JsonModuleViewElement, variable);
            if (ident) namedElements = namedElements.set(ident, e.uid);
            break;
          case 'alias':
            e = AliasViewElement.fromJson(element as JsonAliasViewElement);
            break;
          case 'cloud':
            e = CloudViewElement.fromJson(element as JsonCloudViewElement);
            break;
          case 'group':
            e = GroupViewElement.fromJson(element as JsonGroupViewElement);
            break;
          default:
            throw new Error(`unknown view element type: ${(element as JsonViewElement).type}`);
        }
        maxUid = Math.max(e.uid, maxUid);
        return e;
      }),
    ).map((element) => {
      if (element instanceof StockViewElement && element.var) {
        const stock = element.var;
        const inflows = List<UID>(
          stock.inflows.filter((ident: string) => namedElements.has(ident)).map((ident: string) => defined(namedElements.get(ident))),
        );
        const outflows = List<UID>(
          stock.outflows.filter((ident: string) => namedElements.has(ident)).map((ident: string) => defined(namedElements.get(ident))),
        );
        return element.merge({
          inflows,
          outflows,
        });
      }
      return element;
    });

    let nextUid = maxUid + 1;
    if (nextUid === 0) {
      nextUid = 1;
    }

    const viewBox = json.viewBox ? Rect.fromJson(json.viewBox) : Rect.default();

    return new StockFlowView({
      elements,
      nextUid,
      viewBox,
      zoom: json.zoom ?? 1,
    });
  }
  toJson(): JsonView {
    const elements: JsonViewElement[] = this.elements
      .map((element) => {
        if (element instanceof AuxViewElement) {
          return element.toJson();
        } else if (element instanceof StockViewElement) {
          return element.toJson();
        } else if (element instanceof FlowViewElement) {
          return element.toJson();
        } else if (element instanceof LinkViewElement) {
          return element.toJson();
        } else if (element instanceof ModuleViewElement) {
          return element.toJson();
        } else if (element instanceof AliasViewElement) {
          return element.toJson();
        } else if (element instanceof CloudViewElement) {
          return element.toJson();
        } else if (element instanceof GroupViewElement) {
          return element.toJson();
        } else {
          throw new Error('unknown view element variant');
        }
      })
      .toArray();

    const result: JsonView = {
      elements,
    };

    if (this.viewBox && (this.viewBox.width > 0 || this.viewBox.height > 0)) {
      result.viewBox = this.viewBox.toJson();
    }

    if (this.zoom > 0) {
      result.zoom = this.zoom;
    }

    return result;
  }
}

export function viewElementType(
  element: ViewElement,
): 'aux' | 'stock' | 'flow' | 'link' | 'module' | 'alias' | 'cloud' | 'group' {
  if (element instanceof AuxViewElement) {
    return 'aux';
  } else if (element instanceof StockViewElement) {
    return 'stock';
  } else if (element instanceof FlowViewElement) {
    return 'flow';
  } else if (element instanceof LinkViewElement) {
    return 'link';
  } else if (element instanceof ModuleViewElement) {
    return 'module';
  } else if (element instanceof AliasViewElement) {
    return 'alias';
  } else if (element instanceof CloudViewElement) {
    return 'cloud';
  } else if (element instanceof GroupViewElement) {
    return 'group';
  } else {
    throw new Error('unknown view element variant');
  }
}

const loopMetadataDefaults = {
  uids: List<number>(),
  deleted: false,
  name: '',
  description: '',
};
export class LoopMetadata extends Record(loopMetadataDefaults) {
  constructor(props: typeof loopMetadataDefaults) {
    super(props);
  }
  static fromJson(json: JsonLoopMetadata): LoopMetadata {
    return new LoopMetadata({
      uids: List(json.uids),
      deleted: json.deleted ?? false,
      name: json.name,
      description: json.description ?? '',
    });
  }
  toJson(): JsonLoopMetadata {
    const result: JsonLoopMetadata = {
      uids: this.uids.toArray(),
      name: this.name,
    };
    if (this.deleted) {
      result.deleted = this.deleted;
    }
    if (this.description) {
      result.description = this.description;
    }
    return result;
  }
}

const modelDefaults = {
  name: '',
  variables: Map<string, Variable>(),
  views: List<StockFlowView>(),
  loopMetadata: List<LoopMetadata>(),
};
export class Model extends Record(modelDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof modelDefaults) {
    super(props);
  }
  static fromJson(json: JsonModel): Model {
    const variables = Map<string, Variable>(
      [
        ...(json.stocks ?? []).map((s: JsonStock) => Stock.fromJson(s) as Variable),
        ...(json.flows ?? []).map((f: JsonFlow) => Flow.fromJson(f) as Variable),
        ...(json.auxiliaries ?? []).map((a: JsonAuxiliary) => Aux.fromJson(a) as Variable),
        ...(json.modules ?? []).map((m: JsonModule) => Module.fromJson(m) as Variable),
      ].map((v: Variable) => [v.ident, v] as [string, Variable]),
    );

    return new Model({
      name: json.name,
      variables,
      views: List((json.views ?? []).map((view: JsonView) => StockFlowView.fromJson(view, variables))),
      loopMetadata: List((json.loopMetadata ?? []).map((lm: JsonLoopMetadata) => LoopMetadata.fromJson(lm))),
    });
  }
  toJson(): JsonModel {
    const stocks: JsonStock[] = [];
    const flows: JsonFlow[] = [];
    const auxiliaries: JsonAuxiliary[] = [];
    const modules: JsonModule[] = [];

    for (const variable of this.variables.values()) {
      if (variable instanceof Stock) {
        stocks.push(variable.toJson());
      } else if (variable instanceof Flow) {
        flows.push(variable.toJson());
      } else if (variable instanceof Aux) {
        auxiliaries.push(variable.toJson());
      } else if (variable instanceof Module) {
        modules.push(variable.toJson());
      }
    }

    const result: JsonModel = {
      name: this.name,
      stocks,
      flows,
      auxiliaries,
    };

    if (modules.length > 0) {
      result.modules = modules;
    }
    if (this.views.size > 0) {
      result.views = this.views.map((v: StockFlowView) => v.toJson()).toArray();
    }
    if (this.loopMetadata.size > 0) {
      result.loopMetadata = this.loopMetadata.map((lm: LoopMetadata) => lm.toJson()).toArray();
    }

    return result;
  }
}

const dtDefaults = {
  value: 1,
  isReciprocal: false,
};
export class Dt extends Record(dtDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof dtDefaults) {
    super(props);
  }
  static fromJson(dt: string): Dt {
    if (dt.startsWith('1/')) {
      const value = parseFloat(dt.substring(2));
      return new Dt({ value, isReciprocal: true });
    }
    return new Dt({ value: parseFloat(dt), isReciprocal: false });
  }
  toJson(): string {
    if (this.isReciprocal) {
      return `1/${this.value}`;
    }
    return String(this.value);
  }
  static default(): Dt {
    return new Dt(dtDefaults);
  }
}

export type SimMethod = 'euler' | 'rk4';

const simSpecsDefaults = {
  start: 0,
  stop: 10,
  dt: Dt.default(),
  saveStep: undefined as Dt | undefined,
  simMethod: 'euler' as SimMethod,
  timeUnits: undefined as string | undefined,
};
export class SimSpecs extends Record(simSpecsDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof simSpecsDefaults) {
    super(props);
  }
  static fromJson(json: JsonSimSpecs): SimSpecs {
    return new SimSpecs({
      start: json.startTime,
      stop: json.endTime,
      dt: Dt.fromJson(json.dt ?? '1'),
      saveStep: json.saveStep ? new Dt({ value: json.saveStep, isReciprocal: false }) : undefined,
      simMethod: (json.method ?? 'euler') as SimMethod,
      timeUnits: json.timeUnits,
    });
  }
  toJson(): JsonSimSpecs {
    const result: JsonSimSpecs = {
      startTime: this.start,
      endTime: this.stop,
      dt: this.dt.toJson(),
    };
    if (this.saveStep) {
      result.saveStep = this.saveStep.isReciprocal ? 1 / this.saveStep.value : this.saveStep.value;
    }
    if (this.simMethod && this.simMethod !== 'euler') {
      result.method = this.simMethod;
    }
    if (this.timeUnits) {
      result.timeUnits = this.timeUnits;
    }
    return result;
  }
  static default(): SimSpecs {
    return new SimSpecs(simSpecsDefaults);
  }
}

const dimensionDefaults = {
  name: '',
  subscripts: List<string>(),
};
export class Dimension extends Record(dimensionDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof dimensionDefaults) {
    super(props);
  }
  static fromJson(json: JsonDimension): Dimension {
    return new Dimension({
      name: json.name,
      subscripts: List(json.elements ?? []),
    });
  }
  toJson(): JsonDimension {
    const result: JsonDimension = {
      name: this.name,
    };
    if (this.subscripts.size > 0) {
      result.elements = this.subscripts.toArray();
    }
    return result;
  }
}

export type Extension = 'xmile' | 'vensim' | undefined;

const sourceDefaults = {
  extension: undefined as Extension,
  content: '',
};
export class Source extends Record(sourceDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof sourceDefaults) {
    super(props);
  }
  static fromJson(json: JsonSource): Source {
    let extension: Extension;
    if (json.extension === 'xmile') {
      extension = 'xmile';
    } else if (json.extension === 'vensim') {
      extension = 'vensim';
    } else {
      extension = undefined;
    }
    return new Source({
      extension,
      content: json.content ?? '',
    });
  }
  toJson(): JsonSource {
    const result: JsonSource = {};
    if (this.extension) {
      result.extension = this.extension;
    }
    if (this.content) {
      result.content = this.content;
    }
    return result;
  }
}

const projectDefaults = {
  name: '',
  simSpecs: SimSpecs.default(),
  models: Map<string, Model>(),
  dimensions: Map<string, Dimension>(),
  hasNoEquations: false,
  source: undefined as Source | undefined,
};
export class Project extends Record(projectDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof projectDefaults) {
    super(props);
  }
  static fromJson(json: JsonProject): Project {
    return new Project({
      name: json.name,
      simSpecs: SimSpecs.fromJson(json.simSpecs),
      models: Map<string, Model>(json.models.map((model: JsonModel) => [model.name, Model.fromJson(model)] as [string, Model])),
      dimensions: Map<string, Dimension>((json.dimensions ?? []).map((dim: JsonDimension) => [dim.name, Dimension.fromJson(dim)] as [string, Dimension])),
      hasNoEquations: false,
      source: json.source ? Source.fromJson(json.source) : undefined,
    });
  }
  toJson(): JsonProject {
    const result: JsonProject = {
      name: this.name,
      simSpecs: this.simSpecs.toJson(),
      models: this.models
        .valueSeq()
        .map((m: Model) => m.toJson())
        .toArray(),
    };
    if (this.dimensions.size > 0) {
      result.dimensions = this.dimensions
        .valueSeq()
        .map((d: Dimension) => d.toJson())
        .toArray();
    }
    if (this.source) {
      result.source = this.source.toJson();
    }
    return result;
  }
  attachData(data: Map<string, Series>, modelName: string): Project {
    let model = defined(this.models.get(modelName));
    const variables = model.variables.map((v: Variable) => {
      if (data.has(v.ident)) {
        return v.set('data', [defined(data.get(v.ident))]);
      }
      if (!v.isArrayed) {
        return v;
      }
      const eqn = defined(v.equation);
      if (!(eqn instanceof ApplyToAllEquation || eqn instanceof ArrayedEquation)) {
        return v;
      }
      const dimNames = eqn.dimensionNames;
      if (dimNames.size !== 1) {
        return v;
      }
      const ident = v.ident;
      const dim = defined(this.dimensions.get(defined(dimNames.get(0))));
      const series = dim.subscripts
        .map((element) => data.get(`${ident}[${element}]`))
        .filter((data) => data !== undefined)
        .map((data) => defined(data))
        .toArray();

      return v.set('data', series);
    });
    model = model.set('variables', variables);

    return this.set('models', this.models.set(modelName, model));
  }
}
