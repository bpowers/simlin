// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined, Series } from './common';

import { List, Map, Record } from 'immutable';

import { toUint8Array } from 'js-base64';

import {
  GraphicalFunction as PbGraphicalFunction,
  Variable as PbVariable,
  ViewElement as PbViewElement,
  View as PbView,
  Dt as PbDt,
  Project as PbProject,
  Model as PbModel,
  SimSpecs as PbSimSpecs,
  SimMethodMap as PbSimMethodMap,
  Dimension as PbDimension,
  Rect as PbRect,
  Source as PbSource,
  LoopMetadata as PbLoopMetadata,
} from './pb/project_io_pb';
import { canonicalize } from './canonicalize';

import {
  ErrorCode,
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

function getGraphicalFunctionKind(
  kind: PbGraphicalFunction.KindMap[keyof PbGraphicalFunction.KindMap],
): GraphicalFunctionKind {
  switch (kind) {
    case PbGraphicalFunction.Kind.CONTINUOUS:
      return 'continuous';
    case PbGraphicalFunction.Kind.EXTRAPOLATE:
      return 'extrapolate';
    case PbGraphicalFunction.Kind.DISCRETE:
      return 'discrete';
    default:
      return 'continuous';
  }
}

const graphicalFunctionScaleDefaults = {
  min: 0.0,
  max: 0.0,
};
export class GraphicalFunctionScale extends Record(graphicalFunctionScaleDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof graphicalFunctionScaleDefaults) {
    super(props);
  }
  static fromPb(scale: PbGraphicalFunction.Scale): GraphicalFunctionScale {
    return new GraphicalFunctionScale({
      min: scale.getMin(),
      max: scale.getMax(),
    });
  }
  toPb(): PbGraphicalFunction.Scale {
    const scale = new PbGraphicalFunction.Scale();
    scale.setMin(this.min);
    scale.setMax(this.max);
    return scale;
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

  static fromPb(gf: PbGraphicalFunction): GraphicalFunction {
    const xPoints = gf.getXPointsList();
    return new GraphicalFunction({
      kind: getGraphicalFunctionKind(gf.getKind()),
      xPoints: xPoints.length !== 0 ? List(xPoints) : undefined,
      yPoints: List(gf.getYPointsList()),
      xScale: GraphicalFunctionScale.fromPb(defined(gf.getXScale())),
      yScale: GraphicalFunctionScale.fromPb(defined(gf.getYScale())),
    });
  }
  toPb(): PbGraphicalFunction {
    const gf = new PbGraphicalFunction();
    if (this.kind) {
      switch (this.kind) {
        case 'continuous':
          gf.setKind(PbGraphicalFunction.Kind.CONTINUOUS);
          break;
        case 'discrete':
          gf.setKind(PbGraphicalFunction.Kind.DISCRETE);
          break;
        case 'extrapolate':
          gf.setKind(PbGraphicalFunction.Kind.EXTRAPOLATE);
          break;
      }
    }
    if (this.xPoints && this.xPoints.size > 0) {
      gf.setXPointsList(this.xPoints.toArray());
    }
    if (this.yPoints) {
      gf.setYPointsList(this.yPoints.toArray());
    }
    if (this.xScale) {
      gf.setXScale(this.xScale.toPb());
    }
    if (this.yScale) {
      gf.setYScale(this.yScale.toPb());
    }
    return gf;
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
  static fromPb(v: PbVariable.ScalarEquation): ScalarEquation {
    return new ScalarEquation({
      equation: v.getEquation(),
    });
  }
  toPb(): PbVariable.ScalarEquation {
    const eqn = new PbVariable.ScalarEquation();
    eqn.setEquation(this.equation);
    return eqn;
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
  static fromPb(v: PbVariable.ApplyToAllEquation): ApplyToAllEquation {
    return new ApplyToAllEquation({
      dimensionNames: List(v.getDimensionNamesList()),
      equation: v.getEquation(),
    });
  }
  toPb(): PbVariable.ApplyToAllEquation {
    const equation = new PbVariable.ApplyToAllEquation();
    equation.setDimensionNamesList(this.dimensionNames.toArray());
    equation.setEquation(this.equation);
    return equation;
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
  static fromPb(v: PbVariable.ArrayedEquation): ArrayedEquation {
    return new ArrayedEquation({
      dimensionNames: List(v.getDimensionNamesList()),
      elements: Map(v.getElementsList().map((el) => [el.getSubscript(), el.getEquation()])),
    });
  }
  toPb(): PbVariable.ArrayedEquation {
    const equation = new PbVariable.ArrayedEquation();
    equation.setDimensionNamesList(this.dimensionNames.toArray());
    equation.setElementsList(
      this.elements
        .map((name, eqn) => {
          const element = new PbVariable.ArrayedEquation.Element();
          element.setSubscript(name);
          element.setEquation(eqn);
          return element;
        })
        .valueSeq()
        .toArray(),
    );
    return equation;
  }
}

function equationFromPb(pbEquation: PbVariable.Equation | undefined): Equation {
  if (pbEquation?.hasApplyToAll()) {
    return ApplyToAllEquation.fromPb(defined(pbEquation?.getApplyToAll()));
  } else if (pbEquation?.hasArrayed()) {
    return ArrayedEquation.fromPb(defined(pbEquation?.getArrayed()));
  } else {
    return ScalarEquation.fromPb(defined(pbEquation?.getScalar()));
  }
}

function stockEquationFromJson(
  initialEquation: string | undefined,
  arrayedEquation: JsonArrayedEquation | undefined,
): Equation {
  if (arrayedEquation) {
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return new ArrayedEquation({
        dimensionNames: List(arrayedEquation.dimensions),
        elements: Map<string, string>(arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string])),
      });
    } else {
      return new ApplyToAllEquation({
        dimensionNames: List(arrayedEquation.dimensions),
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
    if (arrayedEquation.elements && arrayedEquation.elements.length > 0) {
      return {
        equation: new ArrayedEquation({
          dimensionNames: List(arrayedEquation.dimensions),
          elements: Map<string, string>(arrayedEquation.elements.map((el: JsonElementEquation) => [el.subscript, el.equation] as [string, string])),
        }),
        graphicalFunction,
      };
    } else {
      return {
        equation: new ApplyToAllEquation({
          dimensionNames: List(arrayedEquation.dimensions),
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
  static fromPb(stock: PbVariable.Stock): Stock {
    return new Stock({
      ident: stock.getIdent(),
      equation: equationFromPb(stock.getEquation()),
      documentation: stock.getDocumentation(),
      units: stock.getUnits(),
      inflows: List(stock.getInflowsList()),
      outflows: List(stock.getOutflowsList()),
      nonNegative: stock.getNonNegative(),
      data: undefined,
      errors: undefined as List<EquationError> | undefined,
      unitErrors: undefined as List<UnitError> | undefined,
      uid: stock.getUid() || undefined,
    });
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
  static fromPb(flow: PbVariable.Flow): Flow {
    const gf = flow.getGf();
    return new Flow({
      ident: flow.getIdent(),
      equation: equationFromPb(flow.getEquation()),
      documentation: flow.getDocumentation(),
      units: flow.getUnits(),
      gf: gf ? GraphicalFunction.fromPb(gf) : undefined,
      nonNegative: flow.getNonNegative(),
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: flow.getUid() || undefined,
    });
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
  static fromPb(aux: PbVariable.Aux): Aux {
    const gf = aux.getGf();
    return new Aux({
      ident: aux.getIdent(),
      equation: equationFromPb(aux.getEquation()),
      documentation: aux.getDocumentation(),
      units: aux.getUnits(),
      gf: gf ? GraphicalFunction.fromPb(gf) : undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: aux.getUid() || undefined,
    });
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
  static fromPb(modRef: PbVariable.Module.Reference): ModuleReference {
    return new ModuleReference({
      src: modRef.getSrc(),
      dst: modRef.getDst(),
    });
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
  static fromPb(module: PbVariable.Module): Module {
    return new Module({
      ident: module.getIdent(),
      modelName: module.getModelName(),
      documentation: module.getDocumentation(),
      units: module.getUnits(),
      references: List(module.getReferencesList().map((modRef) => ModuleReference.fromPb(modRef))),
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: module.getUid() || undefined,
    });
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

function getLabelSide(labelSide: PbViewElement.LabelSideMap[keyof PbViewElement.LabelSideMap]): LabelSide {
  switch (labelSide) {
    case PbViewElement.LabelSide.TOP:
      return 'top';
    case PbViewElement.LabelSide.LEFT:
      return 'left';
    case PbViewElement.LabelSide.CENTER:
      return 'center';
    case PbViewElement.LabelSide.BOTTOM:
      return 'bottom';
    case PbViewElement.LabelSide.RIGHT:
      return 'right';
    default:
      return 'top';
  }
}

function labelSideToPb(labelSide: LabelSide): PbViewElement.LabelSideMap[keyof PbViewElement.LabelSideMap] {
  switch (labelSide) {
    case 'top':
      return PbViewElement.LabelSide.TOP;
    case 'left':
      return PbViewElement.LabelSide.LEFT;
    case 'center':
      return PbViewElement.LabelSide.CENTER;
    case 'bottom':
      return PbViewElement.LabelSide.BOTTOM;
    case 'right':
      return PbViewElement.LabelSide.RIGHT;
  }
}

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
  static fromPb(aux: PbViewElement.Aux, ident: string, auxVar?: Variable | undefined): AuxViewElement {
    return new AuxViewElement({
      uid: aux.getUid(),
      name: aux.getName(),
      ident,
      var: auxVar instanceof Aux ? auxVar : undefined,
      x: aux.getX(),
      y: aux.getY(),
      labelSide: getLabelSide(aux.getLabelSide()),
      isZeroRadius: false,
    });
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
  toPb(): PbViewElement.Aux {
    const element = new PbViewElement.Aux();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  static fromPb(stock: PbViewElement.Stock, ident: string, stockVar: Variable | undefined): StockViewElement {
    return new StockViewElement({
      uid: stock.getUid(),
      name: stock.getName(),
      ident,
      var: stockVar instanceof Stock ? stockVar : undefined,
      x: stock.getX(),
      y: stock.getY(),
      labelSide: getLabelSide(stock.getLabelSide()),
      isZeroRadius: false,
      inflows: List<UID>(),
      outflows: List<UID>(),
    });
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
  toPb(): PbViewElement.Stock {
    const element = new PbViewElement.Stock();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  static fromPb(point: PbViewElement.FlowPoint): Point {
    const attachedToUid = point.getAttachedToUid();
    return new Point({
      x: point.getX(),
      y: point.getY(),
      attachedToUid: attachedToUid ? attachedToUid : undefined,
    });
  }
  static fromJson(json: JsonFlowPoint): Point {
    return new Point({
      x: json.x,
      y: json.y,
      attachedToUid: json.attachedToUid,
    });
  }
  toPb(): PbViewElement.FlowPoint {
    const element = new PbViewElement.FlowPoint();
    element.setX(this.x);
    element.setY(this.y);
    if (this.attachedToUid !== undefined) {
      element.setAttachedToUid(this.attachedToUid);
    }
    return element;
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
  static fromPb(flow: PbViewElement.Flow, ident: string, flowVar?: Variable): FlowViewElement {
    return new FlowViewElement({
      uid: flow.getUid(),
      name: flow.getName(),
      ident,
      var: flowVar instanceof Flow ? flowVar : undefined,
      x: flow.getX(),
      y: flow.getY(),
      labelSide: getLabelSide(flow.getLabelSide()),
      points: List(flow.getPointsList().map((point) => Point.fromPb(point))),
      isZeroRadius: false,
    });
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
  toPb(): PbViewElement.Flow {
    const element = new PbViewElement.Flow();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setPointsList(this.points.map((p) => p.toPb()).toArray());
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  static fromPb(link: PbViewElement.Link): LinkViewElement {
    let arc: number | undefined = undefined;
    let isStraight = true;
    let multiPoint: List<Point> | undefined = undefined;
    switch (link.getShapeCase()) {
      case PbViewElement.Link.ShapeCase.ARC:
        arc = link.getArc();
        isStraight = false;
        multiPoint = undefined;
        break;
      case PbViewElement.Link.ShapeCase.IS_STRAIGHT:
        arc = undefined;
        isStraight = link.getIsStraight();
        multiPoint = undefined;
        break;
      case PbViewElement.Link.ShapeCase.MULTI_POINT:
        arc = undefined;
        isStraight = false;
        multiPoint = List(
          defined(link.getMultiPoint())
            .getPointsList()
            .map((point) => Point.fromPb(point)),
        );
        break;
    }
    return new LinkViewElement({
      uid: link.getUid(),
      fromUid: link.getFromUid(),
      toUid: link.getToUid(),
      arc,
      isStraight,
      multiPoint,
    });
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
  toPb(): PbViewElement.Link {
    const element = new PbViewElement.Link();
    element.setUid(this.uid);
    element.setFromUid(this.fromUid);
    element.setToUid(this.toUid);
    if (this.arc !== undefined) {
      element.setArc(this.arc);
    } else if (this.multiPoint) {
      const linkPoints = new PbViewElement.Link.LinkPoints();
      linkPoints.setPointsList(this.multiPoint.map((p) => p.toPb()).toArray());
      element.setMultiPoint(linkPoints);
    } else {
      element.setIsStraight(this.isStraight);
    }
    return element;
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
  static fromPb(module: PbViewElement.Module, ident: string, moduleVar?: Variable): ModuleViewElement {
    return new ModuleViewElement({
      uid: module.getUid(),
      name: module.getName(),
      ident,
      var: moduleVar instanceof Module ? moduleVar : undefined,
      x: module.getX(),
      y: module.getY(),
      labelSide: getLabelSide(module.getLabelSide()),
      isZeroRadius: false,
    });
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
  toPb(): PbViewElement.Module {
    const element = new PbViewElement.Module();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  static fromPb(alias: PbViewElement.Alias): AliasViewElement {
    return new AliasViewElement({
      uid: alias.getUid(),
      aliasOfUid: alias.getAliasOfUid(),
      x: alias.getX(),
      y: alias.getY(),
      labelSide: getLabelSide(alias.getLabelSide()),
      isZeroRadius: false,
    });
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
  toPb(): PbViewElement.Alias {
    const element = new PbViewElement.Alias();
    element.setUid(this.uid);
    element.setAliasOfUid(this.aliasOfUid);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  static fromPb(cloud: PbViewElement.Cloud): CloudViewElement {
    return new CloudViewElement({
      uid: cloud.getUid(),
      flowUid: cloud.getFlowUid(),
      x: cloud.getX(),
      y: cloud.getY(),
      isZeroRadius: false,
    });
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
  toPb(): PbViewElement.Cloud {
    const element = new PbViewElement.Cloud();
    element.setUid(this.uid);
    element.setFlowUid(this.flowUid);
    element.setX(this.x);
    element.setY(this.y);
    return element;
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
  static fromPb(rect: PbRect): Rect {
    return new Rect({
      x: rect.getX(),
      y: rect.getY(),
      width: rect.getWidth(),
      height: rect.getHeight(),
    });
  }
  static fromJson(json: JsonRect): Rect {
    return new Rect({
      x: json.x,
      y: json.y,
      width: json.width,
      height: json.height,
    });
  }
  toPb(): PbRect {
    const rect = new PbRect();
    rect.setX(this.x);
    rect.setY(this.y);
    rect.setWidth(this.width);
    rect.setHeight(this.height);
    return rect;
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
  static fromPb(view: PbView, variables: Map<string, Variable>): StockFlowView {
    let maxUid = -1;
    let namedElements = Map<string, UID>();
    const elements = List(
      view.getElementsList().map((element) => {
        let e: ViewElement;
        switch (element.getElementCase()) {
          case PbViewElement.ElementCase.AUX: {
            const aux = defined(element.getAux());
            const ident = canonicalize(aux.getName());
            e = AuxViewElement.fromPb(aux, ident, variables.get(ident));
            namedElements = namedElements.set(ident, e.uid);
            break;
          }
          case PbViewElement.ElementCase.STOCK: {
            const stock = defined(element.getStock());
            const ident = canonicalize(stock.getName());
            e = StockViewElement.fromPb(stock, ident, variables.get(ident));
            namedElements = namedElements.set(ident, e.uid);
            break;
          }
          case PbViewElement.ElementCase.FLOW: {
            const flow = defined(element.getFlow());
            const ident = canonicalize(flow.getName());
            e = FlowViewElement.fromPb(flow, ident, variables.get(ident));
            namedElements = namedElements.set(ident, e.uid);
            break;
          }
          case PbViewElement.ElementCase.LINK: {
            e = LinkViewElement.fromPb(defined(element.getLink()));
            break;
          }
          case PbViewElement.ElementCase.MODULE: {
            const module = defined(element.getModule());
            const ident = canonicalize(module.getName());
            e = ModuleViewElement.fromPb(module, ident, variables.get(ident));
            namedElements = namedElements.set(ident, e.uid);
            break;
          }
          case PbViewElement.ElementCase.ALIAS: {
            e = AliasViewElement.fromPb(defined(element.getAlias()));
            break;
          }
          case PbViewElement.ElementCase.CLOUD: {
            e = CloudViewElement.fromPb(defined(element.getCloud()));
            break;
          }
          default: {
            throw new Error('invariant broken: protobuf variable with empty oneof');
          }
        }
        maxUid = Math.max(e.uid, maxUid);
        return e;
      }),
    ).map((element: ViewElement) => {
      if (element instanceof StockViewElement && element.var) {
        const stock = element.var;
        const inflows = List<UID>(
          stock.inflows.filter((ident) => namedElements.has(ident)).map((ident) => defined(namedElements.get(ident))),
        );
        const outflows = List<UID>(
          stock.outflows.filter((ident) => namedElements.has(ident)).map((ident) => defined(namedElements.get(ident))),
        );
        return element.merge({
          inflows,
          outflows,
        });
      }
      return element;
    });
    let nextUid = maxUid + 1;
    // if this is an empty view, start the numbering at 1
    if (nextUid === 0) {
      nextUid = 1;
    }

    const pbViewBox = view.getViewbox();
    const viewBox = pbViewBox ? Rect.fromPb(pbViewBox) : Rect.default();

    return new StockFlowView({
      elements,
      nextUid,
      viewBox,
      zoom: view.getZoom() || 1,
    });
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
  toPb(): PbView {
    const view = new PbView();
    view.setKind(PbView.ViewType.STOCK_FLOW);
    const elements = this.elements
      .map((element) => {
        const e = new PbViewElement();
        if (element instanceof AuxViewElement) {
          e.setAux(element.toPb());
        } else if (element instanceof StockViewElement) {
          e.setStock(element.toPb());
        } else if (element instanceof FlowViewElement) {
          e.setFlow(element.toPb());
        } else if (element instanceof LinkViewElement) {
          e.setLink(element.toPb());
        } else if (element instanceof ModuleViewElement) {
          e.setModule(element.toPb());
        } else if (element instanceof AliasViewElement) {
          e.setAlias(element.toPb());
        } else if (element instanceof CloudViewElement) {
          e.setCloud(element.toPb());
        } else {
          throw new Error('unknown view element variant');
        }
        return e;
      })
      .toArray();

    view.setElementsList(elements);
    view.setViewbox(this.viewBox.toPb());
    view.setZoom(this.zoom);

    return view;
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
): 'aux' | 'stock' | 'flow' | 'link' | 'module' | 'alias' | 'cloud' {
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
  static fromPb(loopMetadata: PbLoopMetadata): LoopMetadata {
    return new LoopMetadata({
      uids: List(loopMetadata.getUidsList()),
      deleted: loopMetadata.getDeleted(),
      name: loopMetadata.getName(),
      description: loopMetadata.getDescription(),
    });
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
  static fromPb(model: PbModel): Model {
    const variables = Map(
      model
        .getVariablesList()
        .map((v: PbVariable) => {
          switch (v.getVCase()) {
            case PbVariable.VCase.STOCK:
              return Stock.fromPb(defined(v.getStock())) as Variable;
            case PbVariable.VCase.FLOW:
              return Flow.fromPb(defined(v.getFlow())) as Variable;
            case PbVariable.VCase.AUX:
              return Aux.fromPb(defined(v.getAux())) as Variable;
            case PbVariable.VCase.MODULE:
              return Module.fromPb(defined(v.getModule())) as Variable;
            default:
              throw new Error('invariant broken: protobuf variable with empty oneof');
          }
        })
        .map((v: Variable) => [v.ident, v]),
    );
    return new Model({
      name: model.getName(),
      variables,
      views: List(
        model.getViewsList().map((view) => {
          switch (view.getKind()) {
            case PbView.ViewType.STOCK_FLOW:
              return StockFlowView.fromPb(view, variables);
            default:
              throw new Error('invariant broken: protobuf view with unknown kind');
          }
        }),
      ),
      loopMetadata: List(model.getLoopMetadataList().map((lm) => LoopMetadata.fromPb(lm))),
    });
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
  static fromPb(dt: PbDt): Dt {
    return new Dt({
      value: dt.getValue(),
      isReciprocal: dt.getIsReciprocal(),
    });
  }
  static fromJson(dt: string): Dt {
    if (dt.startsWith('1/')) {
      const value = parseFloat(dt.substring(2));
      return new Dt({ value, isReciprocal: true });
    }
    return new Dt({ value: parseFloat(dt), isReciprocal: false });
  }
  toPb(): PbDt {
    const dt = new PbDt();
    dt.setValue(this.value);
    dt.setIsReciprocal(this.isReciprocal);
    return dt;
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

function getSimMethod(method: PbSimMethodMap[keyof PbSimMethodMap]): SimMethod {
  switch (method) {
    case 0:
      return 'euler';
    case 1:
      return 'rk4';
    default:
      return 'euler';
  }
}

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
  static fromPb(simSpecs: PbSimSpecs): SimSpecs {
    const saveStep = simSpecs.getSaveStep();
    return new SimSpecs({
      start: simSpecs.getStart(),
      stop: simSpecs.getStop(),
      dt: Dt.fromPb(defined(simSpecs.getDt())),
      saveStep: saveStep ? Dt.fromPb(saveStep) : undefined,
      simMethod: getSimMethod(simSpecs.getSimMethod()),
      timeUnits: simSpecs.getTimeUnits(),
    });
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
  static fromPb(dim: PbDimension): Dimension {
    return new Dimension({
      name: dim.getName(),
      subscripts: List(dim.getObsoleteElementsList()),
    });
  }
  static fromJson(json: JsonDimension): Dimension {
    return new Dimension({
      name: json.name,
      subscripts: List(json.elements ?? []),
    });
  }
  toPb(): PbDimension {
    const dim = new PbDimension();
    dim.setName(this.name);
    dim.setObsoleteElementsList(this.subscripts.toArray());
    return dim;
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

function getExtension(ext: PbSource.ExtensionMap[keyof PbSource.ExtensionMap]): Extension {
  switch (ext) {
    case PbSource.Extension.UNSPECIFIED:
      return undefined;
    case PbSource.Extension.XMILE:
      return 'xmile';
    case PbSource.Extension.VENSIM:
      return 'vensim';
    default:
      return undefined;
  }
}

function extensionToPb(ext: Extension): PbSource.ExtensionMap[keyof PbSource.ExtensionMap] {
  switch (ext) {
    case 'xmile':
      return PbSource.Extension.XMILE;
    case 'vensim':
      return PbSource.Extension.VENSIM;
    default:
      return PbSource.Extension.UNSPECIFIED;
  }
}

const sourceDefaults = {
  extension: undefined as Extension,
  content: '',
};
export class Source extends Record(sourceDefaults) {
  // this isn't useless, as it ensures we specify the full object

  constructor(props: typeof sourceDefaults) {
    super(props);
  }
  static fromPb(source: PbSource): Source {
    return new Source({
      extension: getExtension(source.getExtension$()),
      content: source.getContent(),
    });
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
  toPb(): PbSource {
    const source = new PbSource();
    source.setExtension$(extensionToPb(this.extension));
    source.setContent(this.content);
    return source;
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
  static fromPb(project: PbProject): Project {
    const source = project.getSource();
    return new Project({
      name: project.getName(),
      simSpecs: SimSpecs.fromPb(defined(project.getSimSpecs())),
      models: Map(project.getModelsList().map((model) => [model.getName(), Model.fromPb(model)])),
      dimensions: Map(project.getDimensionsList().map((dim) => [dim.getName(), Dimension.fromPb(dim)])),
      hasNoEquations: false,
      source: source ? Source.fromPb(source) : undefined,
    });
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
  static deserializeBinary(serializedPb: Readonly<Uint8Array>): Project {
    const project = PbProject.deserializeBinary(serializedPb as Uint8Array);
    return Project.fromPb(project);
  }
  static deserializeBase64(serializedPb: string): Project {
    return Project.deserializeBinary(toUint8Array(serializedPb));
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
