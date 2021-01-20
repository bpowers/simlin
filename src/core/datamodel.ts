// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined, Series } from './common';

import { List, Map, Record } from 'immutable';

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
} from './pb/project_io_pb';
import { canonicalize } from './canonicalize';

export type UID = number;

export enum ErrorCode {
  NoError,
  DoesNotExist,
  XmlDeserialization,
  VensimConversion,
  ProtobufDecode,
  InvalidToken,
  UnrecognizedEOF,
  UnrecognizedToken,
  ExtraToken,
  UnclosedComment,
  UnclosedQuotedIdent,
  ExpectedNumber,
  UnknownBuiltin,
  BadBuiltinArgs,
  EmptyEquation,
  BadModuleInputDst,
  BadModuleInputSrc,
  NotSimulatable,
  BadTable,
  BadSimSpecs,
  NoAbsoluteReferences,
  CircularDependency,
  ArraysNotImplemented,
  MultiDimensionalArraysNotImplemented,
  BadDimensionName,
  BadModelName,
  MismatchedDimensions,
  ArrayReferenceNeedsExplicitSubscripts,
  DuplicateVariable,
  UnknownDependency,
  VariablesHaveErrors,
  Generic,
}

const equationErrorDefaults = {
  code: ErrorCode.NoError,
  start: 0.0,
  end: 0.0,
};
export class EquationError extends Record(equationErrorDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(props: typeof equationErrorDefaults) {
    super(props);
  }
}

const simErrorDefaults = {
  code: ErrorCode.NoError,
  details: undefined as (string | undefined),
};
export class SimError extends Record(simErrorDefaults) {
}

const modelErrorDefaults = {
  code: ErrorCode.NoError,
  details: undefined as (string | undefined),
};
export class ModelError extends Record(modelErrorDefaults) {
}

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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
}

export type Equation = ScalarEquation | ApplyToAllEquation | ArrayedEquation;

const scalarEquationDefaults = {
  equation: '',
};
export class ScalarEquation extends Record(scalarEquationDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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

export interface Variable {
  readonly ident: string;
  readonly equation: Equation | undefined;
  readonly gf: GraphicalFunction | undefined;
  readonly isArrayed: boolean;
  readonly hasError: boolean;
  readonly errors: List<EquationError> | undefined;
  readonly data: Readonly<Array<Series>> | undefined;
  set(prop: 'errors', errors: List<EquationError> | undefined): Variable;
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
  data: undefined as (Readonly<Array<Series>> | undefined),
  errors: undefined as (List<EquationError> | undefined),
};
export class Stock extends Record(stockDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
      errors: undefined as (List<EquationError> | undefined),
    });
  }
  get gf(): undefined {
    return undefined;
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined;
  }
}

const flowDefaults = {
  ident: '',
  equation: ScalarEquation.default() as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  nonNegative: false,
  data: undefined as (Readonly<Array<Series>> | undefined),
  errors: undefined as (List<EquationError> | undefined),
};
export class Flow extends Record(flowDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
    });
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined;
  }
}

const auxDefaults = {
  ident: '',
  equation: ScalarEquation.default() as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  data: undefined as (Readonly<Array<Series>> | undefined),
  errors: undefined as (List<EquationError> | undefined),
};
export class Aux extends Record(auxDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
    });
  }
  get isArrayed(): boolean {
    return this.equation instanceof ApplyToAllEquation || this.equation instanceof ArrayedEquation;
  }
  get hasError(): boolean {
    return this.errors !== undefined;
  }
}

const moduleReferenceDefaults = {
  src: '',
  dst: '',
};
export class ModuleReference extends Record(moduleReferenceDefaults) {
  constructor(modRef: PbVariable.Module.Reference) {
    super({
      src: modRef.getSrc(),
      dst: modRef.getDst(),
    });
  }
}

const moduleDefaults = {
  ident: '',
  modelName: '',
  documentation: '',
  units: '',
  references: List<ModuleReference>(),
  data: undefined as (Readonly<Array<Series>> | undefined),
  errors: undefined as (List<EquationError> | undefined),
};
export class Module extends Record(moduleDefaults) implements Variable {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(props: typeof moduleDefaults) {
    super(props);
  }
  static fromPb(module: PbVariable.Module): Module {
    return new Module({
      ident: module.getIdent(),
      modelName: module.getModelName(),
      documentation: module.getDocumentation(),
      units: module.getUnits(),
      references: List(module.getReferencesList().map((modRef) => new ModuleReference(modRef))),
      data: undefined,
      errors: undefined,
    });
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.Aux {
    const element = new PbViewElement.Aux();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.Stock {
    const element = new PbViewElement.Stock();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.FlowPoint {
    const element = new PbViewElement.FlowPoint();
    element.setX(this.x);
    element.setY(this.y);
    if (this.attachedToUid !== undefined) {
      element.setAttachedToUid(this.attachedToUid);
    }
    return element;
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
};
export class FlowViewElement extends Record(flowViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  get cx(): number {
    return this.x;
  }
  get cy(): number {
    return this.y;
  }
  isNamed(): boolean {
    return true;
  }
  get isZeroRadius(): boolean {
    return false;
  }
}

const linkViewElementDefaults = {
  uid: -1,
  fromUid: -1,
  toUid: -1,
  arc: 0.0 as number | undefined,
  isStraight: false,
  multiPoint: undefined as List<Point> | undefined,
};
export class LinkViewElement extends Record(linkViewElementDefaults) implements ViewElement {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.Module {
    const element = new PbViewElement.Module();
    element.setUid(this.uid);
    element.setName(this.name);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.Alias {
    const element = new PbViewElement.Alias();
    element.setUid(this.uid);
    element.setAliasOfUid(this.aliasOfUid);
    element.setX(this.x);
    element.setY(this.y);
    element.setLabelSide(labelSideToPb(this.labelSide));
    return element;
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  toPb(): PbViewElement.Cloud {
    const element = new PbViewElement.Cloud();
    element.setUid(this.uid);
    element.setFlowUid(this.flowUid);
    element.setX(this.x);
    element.setY(this.y);
    return element;
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

const stockFlowViewDefaults = {
  nextUid: -1,
  elements: List<ViewElement>(),
};
export class StockFlowView extends Record(stockFlowViewDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
        const inflows = List<UID>(stock.inflows.filter((ident) => namedElements.has(ident)).map((ident) => defined(namedElements.get(ident))));
        const outflows = List<UID>(stock.outflows.filter((ident) => namedElements.has(ident)).map((ident) => defined(namedElements.get(ident))));
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

    return new StockFlowView({
      elements,
      nextUid,
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

    return view;
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

const modelDefaults = {
  name: '',
  variables: Map<string, Variable>(),
  views: List<StockFlowView>(),
};
export class Model extends Record(modelDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
    });
  }
}

const dtDefaults = {
  value: 1,
  isReciprocal: false,
};
export class Dt extends Record(dtDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(props: typeof dtDefaults) {
    super(props);
  }
  static fromPb(dt: PbDt): Dt {
    return new Dt({
      value: dt.getValue(),
      isReciprocal: dt.getIsReciprocal(),
    });
  }
  toPb(): PbDt {
    const dt = new PbDt();
    dt.setValue(this.value);
    dt.setIsReciprocal(this.isReciprocal);
    return dt;
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
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
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(props: typeof dimensionDefaults) {
    super(props);
  }
  static fromPb(dim: PbDimension): Dimension {
    return new Dimension({
      name: dim.getName(),
      subscripts: List(dim.getElementsList()),
    });
  }
  toPb(): PbDimension {
    const dim = new PbDimension();
    dim.setName(this.name);
    dim.setElementsList(this.subscripts.toArray());
    return dim;
  }
}

const projectDefaults = {
  name: '',
  simSpecs: SimSpecs.default(),
  models: Map<string, Model>(),
  dimensions: Map<string, Dimension>(),
  hasNoEquations: false,
};
export class Project extends Record(projectDefaults) {
  // this isn't useless, as it ensures we specify the full object
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(props: typeof projectDefaults) {
    super(props);
  }
  static fromPb(project: PbProject): Project {
    return new Project({
      name: project.getName(),
      simSpecs: SimSpecs.fromPb(defined(project.getSimSpecs())),
      models: Map(project.getModelsList().map((model) => [model.getName(), Model.fromPb(model)])),
      dimensions: Map(project.getDimensionsList().map((dim) => [dim.getName(), Dimension.fromPb(dim)])),
      hasNoEquations: false,
    });
  }
  static deserializeBinary(serializedPb: Readonly<Uint8Array>): Project {
    const project = PbProject.deserializeBinary(serializedPb as Uint8Array);
    return Project.fromPb(project);
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
