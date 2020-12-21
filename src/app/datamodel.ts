// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined } from './common';

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
  SimMethod as PbSimMethod,
  SimMethodMap as PbSimMethodMap,
} from '../system-dynamics-engine/src/project_io_pb';
import { canonicalize } from '../canonicalize';

export type UID = number;

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
  constructor(scale: PbGraphicalFunction.Scale) {
    super({
      min: scale.getMin(),
      max: scale.getMax(),
    });
  }
  static from(props: typeof graphicalFunctionScaleDefaults): GraphicalFunctionScale {
    return new GraphicalFunctionScale(GraphicalFunctionScale.toPb(props));
  }
  static toPb(props: typeof graphicalFunctionScaleDefaults): PbGraphicalFunction.Scale {
    const scale = new PbGraphicalFunction.Scale();
    scale.setMin(props.min);
    scale.setMax(props.max);
    return scale;
  }
}

const graphicalFunctionDefaults = {
  kind: 'continuous' as GraphicalFunctionKind,
  xPoints: undefined as List<number> | undefined,
  yPoints: List<number>(),
  xScale: new GraphicalFunctionScale(new PbGraphicalFunction.Scale()),
  yScale: new GraphicalFunctionScale(new PbGraphicalFunction.Scale()),
};
export class GraphicalFunction extends Record(graphicalFunctionDefaults) {
  constructor(gf: PbGraphicalFunction) {
    const xPoints = gf.getXPointsList();
    super({
      kind: getGraphicalFunctionKind(gf.getKind()),
      xPoints: xPoints.length !== 0 ? List(xPoints) : undefined,
      yPoints: List(gf.getYPointsList()),
      xScale: new GraphicalFunctionScale(defined(gf.getXScale())),
      yScale: new GraphicalFunctionScale(defined(gf.getYScale())),
    });
  }
  static toPb(props: typeof graphicalFunctionDefaults): PbGraphicalFunction {
    const gf = new PbGraphicalFunction();
    if (props.kind) {
      switch (props.kind) {
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
    if (props.xPoints && props.xPoints.size > 0) {
      gf.setXPointsList(props.xPoints.toArray());
    }
    if (props.yPoints) {
      gf.setYPointsList(props.yPoints.toArray());
    }
    if (props.xScale) {
      gf.setXScale(GraphicalFunctionScale.toPb(props.xScale));
    }
    if (props.yScale) {
      gf.setYScale(GraphicalFunctionScale.toPb(props.yScale));
    }
    return gf;
  }
  static from(props: typeof graphicalFunctionDefaults): GraphicalFunction {
    return new GraphicalFunction(GraphicalFunction.toPb(props));
  }
}

export type Equation = ScalarEquation | ApplyToAllEquation | ArrayedEquation;

const scalarEquationDefaults = {
  equation: '',
};
export class ScalarEquation extends Record(scalarEquationDefaults) {
  constructor(v: PbVariable.ScalarEquation) {
    super({
      equation: v.getEquation(),
    });
  }
  static toPb(props: typeof scalarEquationDefaults): PbVariable.ScalarEquation {
    const eqn = new PbVariable.ScalarEquation();
    eqn.setEquation(props.equation);
    return eqn;
  }
  static from(props: typeof scalarEquationDefaults): ScalarEquation {
    return new ScalarEquation(ScalarEquation.toPb(props));
  }
}

const applyToAllEquationDefaults = {
  dimensionNames: List<string>(),
  equation: '',
};
export class ApplyToAllEquation extends Record(applyToAllEquationDefaults) {
  constructor(v: PbVariable.ApplyToAllEquation) {
    super({
      dimensionNames: List(v.getDimensionNamesList()),
      equation: v.getEquation(),
    });
  }
}

const arrayedEquationDefaults = {
  dimensionNames: List<string>(),
  elements: Map<string, string>(),
};
export class ArrayedEquation extends Record(arrayedEquationDefaults) {
  constructor(v: PbVariable.ArrayedEquation) {
    super({
      dimensionNames: List(v.getDimensionNamesList()),
      elements: Map(v.getElementsList().map((el) => [el.getSubscript(), el.getEquation()])),
    });
  }
}

export interface Variable {
  readonly ident: string;
  readonly equation: Equation | undefined;
  readonly gf: GraphicalFunction | undefined;
}

const stockDefaults = {
  ident: '',
  equation: ScalarEquation.from({ equation: '' }) as Equation,
  documentation: '',
  units: '',
  inflows: List<string>(),
  outflows: List<string>(),
  nonNegative: false,
};
export class Stock extends Record(stockDefaults) implements Variable {
  constructor(stock: PbVariable.Stock) {
    const pbEquation = stock.getEquation();
    let equation: Equation = ScalarEquation.from({ equation: '' });
    if (pbEquation?.hasApplyToAll()) {
      equation = new ApplyToAllEquation(defined(pbEquation?.getApplyToAll()));
    } else if (pbEquation?.hasArrayed()) {
      equation = new ArrayedEquation(defined(pbEquation?.getArrayed()));
    } else if (pbEquation?.hasScalar()) {
      equation = new ScalarEquation(defined(pbEquation?.getScalar()));
    }
    super({
      ident: stock.getIdent(),
      equation,
      documentation: stock.getDocumentation(),
      units: stock.getUnits(),
      inflows: List(stock.getInflowsList()),
      outflows: List(stock.getOutflowsList()),
      nonNegative: stock.getNonNegative(),
    });
  }
  get gf(): undefined {
    return undefined;
  }
}

const flowDefaults = {
  ident: '',
  equation: ScalarEquation.from({ equation: '' }) as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  nonNegative: false,
};
export class Flow extends Record(flowDefaults) implements Variable {
  constructor(flow: PbVariable.Flow) {
    const pbEquation = flow.getEquation();
    let equation: Equation = ScalarEquation.from({ equation: '' });
    if (pbEquation?.hasApplyToAll()) {
      equation = new ApplyToAllEquation(defined(pbEquation?.getApplyToAll()));
    } else if (pbEquation?.hasArrayed()) {
      equation = new ArrayedEquation(defined(pbEquation?.getArrayed()));
    } else if (pbEquation?.hasScalar()) {
      equation = new ScalarEquation(defined(pbEquation?.getScalar()));
    }
    const gf = flow.getGf();
    super({
      ident: flow.getIdent(),
      equation,
      documentation: flow.getDocumentation(),
      units: flow.getUnits(),
      gf: gf ? new GraphicalFunction(gf) : undefined,
      nonNegative: flow.getNonNegative(),
    });
  }
}

const auxDefaults = {
  ident: '',
  equation: ScalarEquation.from({ equation: '' }) as Equation,
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
};
export class Aux extends Record(auxDefaults) implements Variable {
  constructor(aux: PbVariable.Aux) {
    const pbEquation = aux.getEquation();
    let equation: Equation = ScalarEquation.from({ equation: '' });
    if (pbEquation?.hasApplyToAll()) {
      equation = new ApplyToAllEquation(defined(pbEquation?.getApplyToAll()));
    } else if (pbEquation?.hasArrayed()) {
      equation = new ArrayedEquation(defined(pbEquation?.getArrayed()));
    } else if (pbEquation?.hasScalar()) {
      equation = new ScalarEquation(defined(pbEquation?.getScalar()));
    }
    const gf = aux.getGf();
    super({
      ident: aux.getIdent(),
      equation,
      documentation: aux.getDocumentation(),
      units: aux.getUnits(),
      gf: gf ? new GraphicalFunction(gf) : undefined,
    });
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
};
export class Module extends Record(moduleDefaults) implements Variable {
  constructor(module: PbVariable.Module) {
    super({
      ident: module.getIdent(),
      modelName: module.getModelName(),
      documentation: module.getDocumentation(),
      units: module.getUnits(),
      references: List(module.getReferencesList().map((modRef) => new ModuleReference(modRef))),
    });
  }
  get equation(): undefined {
    return undefined;
  }
  get gf(): undefined {
    return undefined;
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
  isNamed(): boolean;
  ident(): string | undefined;
  set(prop: 'uid', uid: number): ViewElement;
  set(prop: 'x', x: number): ViewElement;
  set(prop: 'y', x: number): ViewElement;
}

const auxViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'right' as LabelSide,
  isZeroRadius: false,
};
export class AuxViewElement extends Record(auxViewElementDefaults) implements ViewElement {
  constructor(aux: PbViewElement.Aux) {
    super({
      uid: aux.getUid(),
      name: aux.getName(),
      x: aux.getX(),
      y: aux.getY(),
      labelSide: getLabelSide(aux.getLabelSide()),
    });
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
  ident(): string {
    return canonicalize(this.name);
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

  static from(props: typeof auxViewElementDefaults): AuxViewElement {
    const element = new PbViewElement.Aux();
    element.setUid(props.uid);
    element.setName(props.name);
    element.setX(props.x);
    element.setY(props.y);
    element.setLabelSide(labelSideToPb(props.labelSide));

    const aux = new AuxViewElement(element);
    if (props.isZeroRadius) {
      return aux.set('isZeroRadius', true);
    } else {
      return aux;
    }
  }
}

const stockViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
};
export class StockViewElement extends Record(stockViewElementDefaults) implements ViewElement {
  constructor(stock: PbViewElement.Stock) {
    super({
      uid: stock.getUid(),
      name: stock.getName(),
      x: stock.getX(),
      y: stock.getY(),
      labelSide: getLabelSide(stock.getLabelSide()),
    });
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
  ident(): string {
    return canonicalize(this.name);
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

  static from(props: typeof stockViewElementDefaults): StockViewElement {
    const element = new PbViewElement.Stock();
    element.setUid(props.uid);
    element.setName(props.name);
    element.setX(props.x);
    element.setY(props.y);
    element.setLabelSide(labelSideToPb(props.labelSide));

    const stock = new StockViewElement(element);
    if (props.isZeroRadius) {
      return stock.set('isZeroRadius', true);
    } else {
      return stock;
    }
  }
}

const pointDefaults = {
  x: -1,
  y: -1,
  attachedToUid: undefined as number | undefined,
};
export class Point extends Record(pointDefaults) {
  constructor(point: PbViewElement.FlowPoint) {
    const attachedToUid = point.getAttachedToUid();
    super({
      x: point.getX(),
      y: point.getY(),
      attachedToUid: attachedToUid ? attachedToUid : undefined,
    });
  }
  static from(props: typeof pointDefaults): Point {
    const point = new PbViewElement.FlowPoint();
    point.setX(props.x);
    point.setY(props.y);
    if (props.attachedToUid) {
      point.setAttachedToUid(props.attachedToUid);
    }
    return new Point(point);
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
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  points: List<Point>(),
  isZeroRadius: false,
};
export class FlowViewElement extends Record(flowViewElementDefaults) implements ViewElement {
  constructor(flow: PbViewElement.Flow) {
    super({
      uid: flow.getUid(),
      name: flow.getName(),
      x: flow.getX(),
      y: flow.getY(),
      labelSide: getLabelSide(flow.getLabelSide()),
      points: List(flow.getPointsList().map((point) => new Point(point))),
    });
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
  ident(): string {
    return canonicalize(this.name);
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

  static from(props: typeof flowViewElementDefaults): FlowViewElement {
    const element = new PbViewElement.Flow();
    element.setUid(props.uid);
    element.setName(props.name);
    element.setX(props.x);
    element.setY(props.y);
    element.setPointsList(props.points.map((p) => p.toPb()).toArray());
    element.setLabelSide(labelSideToPb(props.labelSide));
    return new FlowViewElement(element);
  }
}

const linkViewElementDefaults = {
  uid: -1,
  fromUid: -1,
  toUid: -1,
  arc: 0.0 as number | undefined,
  isStraight: false,
  multiPoint: undefined as List<Point> | undefined,
  isZeroRadius: false,
};
export class LinkViewElement extends Record(linkViewElementDefaults) implements ViewElement {
  constructor(link: PbViewElement.Link) {
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
            .map((point) => new Point(point)),
        );
        break;
    }
    super({
      uid: link.getUid(),
      fromUid: link.getFromUid(),
      toUid: link.getToUid(),
      arc,
      isStraight,
      multiPoint,
    });
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
  ident(): undefined {
    return undefined;
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

  static from(props: typeof linkViewElementDefaults): LinkViewElement {
    const element = new PbViewElement.Link();
    element.setUid(props.uid);
    element.setFromUid(props.fromUid);
    element.setToUid(props.toUid);
    if (props.arc !== undefined) {
      element.setArc(props.arc);
    } else if (props.multiPoint) {
      const linkPoints = new PbViewElement.Link.LinkPoints();
      linkPoints.setPointsList(props.multiPoint.map((p) => p.toPb()).toArray());
      element.setMultiPoint(linkPoints);
    } else {
      element.setIsStraight(props.isStraight);
    }
    return new LinkViewElement(element);
  }
}

const moduleViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
};
export class ModuleViewElement extends Record(moduleViewElementDefaults) implements ViewElement {
  constructor(module: PbViewElement.Module) {
    super({
      uid: module.getUid(),
      name: module.getName(),
      x: module.getX(),
      y: module.getY(),
      labelSide: getLabelSide(module.getLabelSide()),
    });
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
  ident(): string {
    return canonicalize(this.name);
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
  constructor(alias: PbViewElement.Alias) {
    super({
      uid: alias.getUid(),
      aliasOfUid: alias.getAliasOfUid(),
      x: alias.getX(),
      y: alias.getY(),
      labelSide: getLabelSide(alias.getLabelSide()),
    });
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
  ident(): undefined {
    return undefined;
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
}

const cloudViewElementDefaults = {
  uid: -1,
  flowUid: -1,
  x: -1,
  y: -1,
  isZeroRadius: false,
};
export class CloudViewElement extends Record(cloudViewElementDefaults) implements ViewElement {
  constructor(cloud: PbViewElement.Cloud) {
    super({
      uid: cloud.getUid(),
      flowUid: cloud.getFlowUid(),
      x: cloud.getX(),
      y: cloud.getY(),
    });
  }
  static from(props: typeof cloudViewElementDefaults): CloudViewElement {
    const element = new PbViewElement.Cloud();
    element.setUid(props.uid);
    element.setFlowUid(props.flowUid);
    element.setX(props.x);
    element.setY(props.y);
    const cloud = new CloudViewElement(element);
    if (props.isZeroRadius) {
      return cloud.set('isZeroRadius', true);
    } else {
      return cloud;
    }
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
  ident(): undefined {
    return undefined;
  }

  toPb(): PbViewElement.Cloud {
    const element = new PbViewElement.Cloud();
    element.setUid(this.uid);
    element.setFlowUid(this.flowUid);
    element.setX(this.x);
    element.setY(this.y);
    return element;
  }
}

export type NamedViewElement = StockViewElement | AuxViewElement | ModuleViewElement | FlowViewElement;

const stockFlowViewDefaults = {
  nextUid: -1,
  elements: List<ViewElement>(),
};
export class StockFlowView extends Record(stockFlowViewDefaults) {
  constructor(view: PbView) {
    let maxUid = -1;
    const elements = List(
      view.getElementsList().map((element) => {
        let e: ViewElement;
        switch (element.getElementCase()) {
          case PbViewElement.ElementCase.AUX:
            e = new AuxViewElement(defined(element.getAux()));
            break;
          case PbViewElement.ElementCase.STOCK:
            e = new StockViewElement(defined(element.getStock()));
            break;
          case PbViewElement.ElementCase.FLOW:
            e = new FlowViewElement(defined(element.getFlow()));
            break;
          case PbViewElement.ElementCase.LINK:
            e = new LinkViewElement(defined(element.getLink()));
            break;
          case PbViewElement.ElementCase.MODULE:
            e = new ModuleViewElement(defined(element.getModule()));
            break;
          case PbViewElement.ElementCase.ALIAS:
            e = new AliasViewElement(defined(element.getAlias()));
            break;
          case PbViewElement.ElementCase.CLOUD:
            e = new CloudViewElement(defined(element.getCloud()));
            break;
          default:
            throw new Error('invariant broken: protobuf variable with empty oneof');
        }
        maxUid = Math.max(e.uid, maxUid);
        return e;
      }),
    );
    let nextUid = maxUid + 1;
    // if this is an empty view, start the numbering at 1
    if (nextUid === 0) {
      nextUid = 1;
    }
    super({
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
  constructor(model: PbModel) {
    super({
      name: model.getName(),
      variables: Map(
        model
          .getVariablesList()
          .map((v: PbVariable) => {
            switch (v.getVCase()) {
              case PbVariable.VCase.STOCK:
                return new Stock(defined(v.getStock())) as Variable;
              case PbVariable.VCase.FLOW:
                return new Flow(defined(v.getFlow())) as Variable;
              case PbVariable.VCase.AUX:
                return new Aux(defined(v.getAux())) as Variable;
              case PbVariable.VCase.MODULE:
                return new Module(defined(v.getModule())) as Variable;
              default:
                throw new Error('invariant broken: protobuf variable with empty oneof');
            }
          })
          .map((v: Variable) => [v.ident, v]),
      ),
      views: List(
        model.getViewsList().map((view) => {
          switch (view.getKind()) {
            case PbView.ViewType.STOCK_FLOW:
              return new StockFlowView(view);
            default:
              throw new Error('invariant broken: protobuf view with unknown kind');
          }
        }),
      ),
    });
  }
}

const dtDefaults = {
  dt: -1,
  isReciprocal: false,
};
export class Dt extends Record(dtDefaults) {
  constructor(dt: PbDt) {
    super({
      dt: dt.getValue(),
      isReciprocal: dt.getIsReciprocal(),
    });
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
  start: -1,
  stop: -1,
  dt: new Dt(new PbDt()),
  saveStep: undefined as Dt | undefined,
  simMethod: 'euler' as SimMethod,
  timeUnits: undefined as string | undefined,
};
export class SimSpecs extends Record(simSpecsDefaults) {
  constructor(simSpecs: PbSimSpecs) {
    const saveStep = simSpecs.getSaveStep();
    super({
      start: simSpecs.getStart(),
      stop: simSpecs.getStop(),
      dt: new Dt(defined(simSpecs.getDt())),
      saveStep: saveStep ? new Dt(saveStep) : undefined,
      simMethod: getSimMethod(simSpecs.getSimMethod()),
      timeUnits: simSpecs.getTimeUnits(),
    });
  }

  static default(): SimSpecs {
    const dt = new PbDt();
    dt.setValue(1);
    const specs = new PbSimSpecs();
    specs.setStart(0);
    specs.setStop(10);
    specs.setDt(dt);
    specs.setSimMethod(PbSimMethod.EULER);
    return new SimSpecs(specs);
  }
}

const projectDefaults = {
  name: '',
  simSpecs: SimSpecs.default(),
  models: Map<string, Model>(),
};
export class Project extends Record(projectDefaults) {
  constructor(project: PbProject) {
    super({
      name: project.getName(),
      simSpecs: new SimSpecs(defined(project.getSimSpecs())),
      models: Map(project.getModelsList().map((model) => [model.getName(), new Model(model)])),
    });
  }

  static deserializeBinary(serializedPb: Readonly<Uint8Array>): Project {
    const project = PbProject.deserializeBinary(serializedPb as Uint8Array);
    return new Project(project);
  }
}
