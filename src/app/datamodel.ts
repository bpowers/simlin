// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined } from './common';

import { List, Map, Record } from 'immutable';

import * as pb from '../system-dynamics-engine/src/project_io_pb';
import { canonicalize } from '../canonicalize';

export type UID = number;

export type GraphicalFunctionKind = 'continuous' | 'extrapolate' | 'discrete';

function getGraphicalFunctionKind(
  kind: pb.GraphicalFunction.KindMap[keyof pb.GraphicalFunction.KindMap],
): GraphicalFunctionKind {
  switch (kind) {
    case pb.GraphicalFunction.Kind.CONTINUOUS:
      return 'continuous';
    case pb.GraphicalFunction.Kind.EXTRAPOLATE:
      return 'extrapolate';
    case pb.GraphicalFunction.Kind.DISCRETE:
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
  constructor(scale: pb.GraphicalFunction.Scale) {
    super({
      min: scale.getMin(),
      max: scale.getMax(),
    });
  }
  static from(props: typeof graphicalFunctionScaleDefaults): GraphicalFunctionScale {
    return new GraphicalFunctionScale(GraphicalFunctionScale.toPb(props));
  }
  static toPb(props: typeof graphicalFunctionScaleDefaults): pb.GraphicalFunction.Scale {
    const scale = new pb.GraphicalFunction.Scale();
    scale.setMin(props.min);
    scale.setMax(props.max);
    return scale;
  }
}

const graphicalFunctionDefaults = {
  kind: 'continuous' as GraphicalFunctionKind,
  xPoints: undefined as List<number> | undefined,
  yPoints: List<number>(),
  xScale: new GraphicalFunctionScale(new pb.GraphicalFunction.Scale()),
  yScale: new GraphicalFunctionScale(new pb.GraphicalFunction.Scale()),
};
export class GraphicalFunction extends Record(graphicalFunctionDefaults) {
  constructor(gf: pb.GraphicalFunction) {
    const xPoints = gf.getXPointsList();
    super({
      kind: getGraphicalFunctionKind(gf.getKind()),
      xPoints: xPoints.length !== 0 ? List(xPoints) : undefined,
      yPoints: List(gf.getYPointsList()),
      xScale: new GraphicalFunctionScale(defined(gf.getXScale())),
      yScale: new GraphicalFunctionScale(defined(gf.getYScale())),
    });
  }
  static toPb(props: typeof graphicalFunctionDefaults): pb.GraphicalFunction {
    const gf = new pb.GraphicalFunction();
    if (props.kind) {
      switch (props.kind) {
        case 'continuous':
          gf.setKind(pb.GraphicalFunction.Kind.CONTINUOUS);
          break;
        case 'discrete':
          gf.setKind(pb.GraphicalFunction.Kind.DISCRETE);
          break;
        case 'extrapolate':
          gf.setKind(pb.GraphicalFunction.Kind.EXTRAPOLATE);
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

export interface Variable {
  readonly ident: string;
  readonly equation: string | undefined;
  readonly gf: GraphicalFunction | undefined;
}

const stockDefaults = {
  ident: '',
  equation: '',
  documentation: '',
  units: '',
  inflows: List<string>(),
  outflows: List<string>(),
  nonNegative: false,
};
export class Stock extends Record(stockDefaults) implements Variable {
  constructor(stock: pb.Variable.Stock) {
    super({
      ident: stock.getIdent(),
      equation: stock.getEquation(),
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
  equation: '',
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
  nonNegative: false,
};
export class Flow extends Record(flowDefaults) implements Variable {
  constructor(flow: pb.Variable.Flow) {
    const gf = flow.getGf();
    super({
      ident: flow.getIdent(),
      equation: flow.getEquation(),
      documentation: flow.getDocumentation(),
      units: flow.getUnits(),
      gf: gf ? new GraphicalFunction(gf) : undefined,
      nonNegative: flow.getNonNegative(),
    });
  }
}

const auxDefaults = {
  ident: '',
  equation: '',
  documentation: '',
  units: '',
  gf: undefined as GraphicalFunction | undefined,
};
export class Aux extends Record(auxDefaults) implements Variable {
  constructor(aux: pb.Variable.Aux) {
    const gf = aux.getGf();
    super({
      ident: aux.getIdent(),
      equation: aux.getEquation(),
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
  constructor(modRef: pb.Variable.Module.Reference) {
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
  constructor(module: pb.Variable.Module) {
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

function getLabelSide(labelSide: pb.ViewElement.LabelSideMap[keyof pb.ViewElement.LabelSideMap]): LabelSide {
  switch (labelSide) {
    case 0:
      return 'top';
    case 1:
      return 'left';
    case 2:
      return 'center';
    case 3:
      return 'bottom';
    case 4:
      return 'right';
    default:
      return 'top';
  }
}

export interface ViewElement {
  readonly isZeroRadius: boolean;
  readonly uid: number;
  readonly cx: number;
  readonly cy: number;
  isNamed(): boolean;
  ident(): string | undefined;
}

const auxViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  isZeroRadius: false,
};
export class AuxViewElement extends Record(auxViewElementDefaults) implements ViewElement {
  constructor(aux: pb.ViewElement.Aux) {
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
  constructor(stock: pb.ViewElement.Stock) {
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
}

const pointDefaults = {
  x: -1,
  y: -1,
  attachedToUid: undefined as number | undefined,
};
export class Point extends Record(pointDefaults) {
  constructor(point: pb.ViewElement.FlowPoint) {
    const attachedToUid = point.getAttachedToUid();
    super({
      x: point.getX(),
      y: point.getY(),
      attachedToUid: attachedToUid ? attachedToUid : undefined,
    });
  }
  static from(props: typeof pointDefaults): Point {
    const point = new pb.ViewElement.FlowPoint();
    point.setX(props.x);
    point.setY(props.y);
    if (props.attachedToUid) {
      point.setAttachedToUid(props.attachedToUid);
    }
    return new Point(point);
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
  constructor(flow: pb.ViewElement.Flow) {
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
  constructor(link: pb.ViewElement.Link) {
    let arc: number | undefined = undefined;
    let isStraight = true;
    let multiPoint: List<Point> | undefined = undefined;
    switch (link.getShapeCase()) {
      case pb.ViewElement.Link.ShapeCase.ARC:
        arc = link.getArc();
        isStraight = false;
        multiPoint = undefined;
        break;
      case pb.ViewElement.Link.ShapeCase.IS_STRAIGHT:
        arc = undefined;
        isStraight = link.getIsStraight();
        multiPoint = undefined;
        break;
      case pb.ViewElement.Link.ShapeCase.MULTI_POINT:
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
  constructor(module: pb.ViewElement.Module) {
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
  constructor(alias: pb.ViewElement.Alias) {
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
}

const cloudViewElementDefaults = {
  uid: -1,
  flowUid: -1,
  x: -1,
  y: -1,
  isZeroRadius: false,
};
export class CloudViewElement extends Record(cloudViewElementDefaults) implements ViewElement {
  constructor(cloud: pb.ViewElement.Cloud) {
    super({
      uid: cloud.getUid(),
      flowUid: cloud.getFlowUid(),
      x: cloud.getX(),
      y: cloud.getY(),
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
}

export type NamedViewElement = StockViewElement | AuxViewElement | ModuleViewElement | FlowViewElement;

const stockFlowViewDefaults = {
  elements: List<ViewElement>(),
};
export class StockFlowView extends Record(stockFlowViewDefaults) {
  constructor(view: pb.View) {
    const elements = List(
      view.getElementsList().map((element) => {
        switch (element.getElementCase()) {
          case pb.ViewElement.ElementCase.AUX:
            return new AuxViewElement(defined(element.getAux()));
          case pb.ViewElement.ElementCase.STOCK:
            return new StockViewElement(defined(element.getStock()));
          case pb.ViewElement.ElementCase.FLOW:
            return new FlowViewElement(defined(element.getFlow()));
          case pb.ViewElement.ElementCase.LINK:
            return new LinkViewElement(defined(element.getLink()));
          case pb.ViewElement.ElementCase.MODULE:
            return new ModuleViewElement(defined(element.getModule()));
          case pb.ViewElement.ElementCase.ALIAS:
            return new AliasViewElement(defined(element.getAlias()));
          case pb.ViewElement.ElementCase.CLOUD:
            return new CloudViewElement(defined(element.getCloud()));
          default:
            throw new Error('invariant broken: protobuf variable with empty oneof');
        }
      }),
    );
    super({
      elements,
    });
  }
}

const modelDefaults = {
  name: '',
  variables: Map<string, Variable>(),
  views: List<StockFlowView>(),
};
export class Model extends Record(modelDefaults) {
  constructor(model: pb.Model) {
    super({
      name: model.getName(),
      variables: Map(
        model
          .getVariablesList()
          .map((v: pb.Variable) => {
            switch (v.getVCase()) {
              case pb.Variable.VCase.STOCK:
                return new Stock(defined(v.getStock())) as Variable;
              case pb.Variable.VCase.FLOW:
                return new Flow(defined(v.getFlow())) as Variable;
              case pb.Variable.VCase.AUX:
                return new Aux(defined(v.getAux())) as Variable;
              case pb.Variable.VCase.MODULE:
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
            case pb.View.ViewType.STOCK_FLOW:
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
  constructor(dt: pb.Dt) {
    super({
      dt: dt.getValue(),
      isReciprocal: dt.getIsReciprocal(),
    });
  }
}

export type SimMethod = 'euler' | 'rk4';

function getSimMethod(method: pb.SimMethodMap[keyof pb.SimMethodMap]): SimMethod {
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
  dt: new Dt(new pb.Dt()),
  saveStep: undefined as Dt | undefined,
  simMethod: 'euler' as SimMethod,
  timeUnits: undefined as string | undefined,
};
export class SimSpecs extends Record(simSpecsDefaults) {
  constructor(simSpecs: pb.SimSpecs) {
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
    const dt = new pb.Dt();
    dt.setValue(1);
    const specs = new pb.SimSpecs();
    specs.setStart(0);
    specs.setStop(10);
    specs.setDt(dt);
    specs.setSimMethod(pb.SimMethod.EULER);
    return new SimSpecs(specs);
  }
}

const projectDefaults = {
  name: '',
  simSpecs: SimSpecs.default(),
  models: Map<string, Model>(),
};
export class Project extends Record(projectDefaults) {
  constructor(project: pb.Project) {
    super({
      name: project.getName(),
      simSpecs: new SimSpecs(defined(project.getSimSpecs())),
      models: Map(project.getModelsList().map((model) => [model.getName(), new Model(model)])),
    });
  }
}
