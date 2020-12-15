// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined } from './common';

import { List, Map, Record } from 'immutable';

import * as pb from '../system-dynamics-engine/src/project_io_pb';

export type GraphicalFunctionKind = 'continuous' | 'extrapolate' | 'discrete';

function getGraphicalFunctionKind(
  kind: pb.GraphicalFunction.KindMap[keyof pb.GraphicalFunction.KindMap],
): GraphicalFunctionKind {
  switch (kind) {
    case 0:
      return 'continuous';
    case 1:
      return 'extrapolate';
    case 2:
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
}

export interface Variable {
  readonly ident: string;
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
  readonly uid: number;
}

const auxViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
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
}

const stockViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
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
}

const flowViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
  points: List<Point>(),
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
  constructor(link: pb.ViewElement.Link) {
    let arc: number | undefined;
    let isStraight: boolean;
    let multiPoint: List<Point> | undefined;
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
      default:
        throw new Error('invariant broken: protobuf link with empty shape');
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
}

const moduleViewElementDefaults = {
  uid: -1,
  name: '',
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
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
}

const aliasViewElementDefaults = {
  uid: -1,
  aliasOfUid: -1,
  x: -1,
  y: -1,
  labelSide: 'center' as LabelSide,
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
}

const cloudViewElementDefaults = {
  uid: -1,
  flowUid: -1,
  x: -1,
  y: -1,
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
}

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
