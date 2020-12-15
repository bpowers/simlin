// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { defined } from './common';

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

export class GraphicalFunctionScale {
  readonly min: number;
  readonly max: number;

  constructor(scale: pb.GraphicalFunction.Scale) {
    this.min = scale.getMin();
    this.max = scale.getMax();
  }
}

export class GraphicalFunction {
  readonly kind: GraphicalFunctionKind;
  readonly xPoints?: number[];
  readonly yPoints: number[];
  readonly xScale: GraphicalFunctionScale;
  readonly yScale: GraphicalFunctionScale;

  constructor(gf: pb.GraphicalFunction) {
    this.kind = getGraphicalFunctionKind(gf.getKind());
    const xPoints = gf.getXPointsList();
    this.xPoints = xPoints.length !== 0 ? xPoints : undefined;
    this.yPoints = gf.getYPointsList();
    this.xScale = new GraphicalFunctionScale(defined(gf.getXScale()));
    this.yScale = new GraphicalFunctionScale(defined(gf.getYScale()));
  }
}

export class Variable {
  readonly ident: string;

  constructor(ident: string) {
    this.ident = ident;
  }
}

export class Stock extends Variable {
  readonly equation: string;
  readonly documentation: string;
  readonly units: string;
  readonly inflows: string[];
  readonly outflows: string[];
  readonly nonNegative: boolean;

  constructor(stock: pb.Variable.Stock) {
    super(stock.getIdent());
    this.equation = stock.getEquation();
    this.documentation = stock.getDocumentation();
    this.units = stock.getUnits();
    this.inflows = stock.getInflowsList();
    this.outflows = stock.getOutflowsList();
    this.nonNegative = stock.getNonNegative();
  }
}

export class Flow extends Variable {
  readonly equation: string;
  readonly documentation: string;
  readonly units: string;
  readonly gf?: GraphicalFunction;
  readonly nonNegative: boolean;

  constructor(flow: pb.Variable.Flow) {
    super(flow.getIdent());
    this.equation = flow.getEquation();
    this.documentation = flow.getDocumentation();
    this.units = flow.getUnits();
    const gf = flow.getGf();
    this.gf = gf ? new GraphicalFunction(gf) : undefined;
    this.nonNegative = flow.getNonNegative();
  }
}

export class Aux extends Variable {
  readonly equation: string;
  readonly documentation: string;
  readonly units: string;
  readonly gf?: GraphicalFunction;

  constructor(aux: pb.Variable.Aux) {
    super(aux.getIdent());
    this.equation = aux.getEquation();
    this.documentation = aux.getDocumentation();
    this.units = aux.getUnits();
    const gf = aux.getGf();
    this.gf = gf ? new GraphicalFunction(gf) : undefined;
  }
}

export class ModuleReference {
  readonly src: string;
  readonly dst: string;

  constructor(modRef: pb.Variable.Module.Reference) {
    this.src = modRef.getSrc();
    this.dst = modRef.getDst();
  }
}

export class Module extends Variable {
  readonly modelName: string;
  readonly documentation: string;
  readonly units: string;
  readonly references: ModuleReference[];

  constructor(module: pb.Variable.Module) {
    super(module.getIdent());
    this.modelName = module.getModelName();
    this.documentation = module.getDocumentation();
    this.units = module.getUnits();
    this.references = module.getReferencesList().map((modRef) => new ModuleReference(modRef));
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

export class ViewElement {
  readonly uid: number;

  constructor(uid: number) {
    this.uid = uid;
  }
}

export class AuxViewElement extends ViewElement {
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;

  constructor(aux: pb.ViewElement.Aux) {
    super(aux.getUid());
    this.name = aux.getName();
    this.x = aux.getX();
    this.y = aux.getY();
    this.labelSide = getLabelSide(aux.getLabelSide());
  }
}

export class StockViewElement extends ViewElement {
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;

  constructor(stock: pb.ViewElement.Stock) {
    super(stock.getUid());
    this.name = stock.getName();
    this.x = stock.getX();
    this.y = stock.getY();
    this.labelSide = getLabelSide(stock.getLabelSide());
  }
}

export class Point {
  readonly x: number;
  readonly y: number;
  readonly attachedToUid: number;

  constructor(point: pb.ViewElement.FlowPoint) {
    this.x = point.getX();
    this.y = point.getY();
    this.attachedToUid = point.getAttachedToUid();
  }
}

export class FlowViewElement extends ViewElement {
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;
  readonly points: Point[];

  constructor(flow: pb.ViewElement.Flow) {
    super(flow.getUid());
    this.name = flow.getName();
    this.x = flow.getX();
    this.y = flow.getY();
    this.labelSide = getLabelSide(flow.getLabelSide());
    this.points = flow.getPointsList().map((point) => new Point(point));
  }
}

export class LinkViewElement extends ViewElement {
  readonly fromUid: number;
  readonly toUid: number;
  readonly arc?: number;
  readonly isStraight: boolean;
  readonly multiPoint?: Point[];

  constructor(link: pb.ViewElement.Link) {
    super(link.getUid());
    this.fromUid = link.getFromUid();
    this.toUid = link.getToUid();
    switch (link.getShapeCase()) {
      case pb.ViewElement.Link.ShapeCase.ARC:
        this.arc = link.getArc();
        this.isStraight = false;
        this.multiPoint = undefined;
        break;
      case pb.ViewElement.Link.ShapeCase.IS_STRAIGHT:
        this.arc = undefined;
        this.isStraight = link.getIsStraight();
        this.multiPoint = undefined;
        break;
      case pb.ViewElement.Link.ShapeCase.MULTI_POINT:
        this.arc = undefined;
        this.isStraight = false;
        this.multiPoint = defined(link.getMultiPoint())
          .getPointsList()
          .map((point) => new Point(point));
        break;
      default:
        throw new Error('invariant broken: protobuf link with empty shape');
    }
  }
}

export class ModuleViewElement extends ViewElement {
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;

  constructor(module: pb.ViewElement.Module) {
    super(module.getUid());
    this.name = module.getName();
    this.x = module.getX();
    this.y = module.getY();
    this.labelSide = getLabelSide(module.getLabelSide());
  }
}

export class AliasViewElement extends ViewElement {
  readonly aliasOfUid: number;
  readonly x: number;
  readonly y: number;
  readonly labelSide: LabelSide;

  constructor(alias: pb.ViewElement.Alias) {
    super(alias.getUid());
    this.aliasOfUid = alias.getAliasOfUid();
    this.x = alias.getX();
    this.y = alias.getY();
    this.labelSide = getLabelSide(alias.getLabelSide());
  }
}

export class CloudViewElement extends ViewElement {
  readonly flowUid: number;
  readonly x: number;
  readonly y: number;

  constructor(cloud: pb.ViewElement.Cloud) {
    super(cloud.getUid());
    this.flowUid = cloud.getFlowUid();
    this.x = cloud.getX();
    this.y = cloud.getY();
  }
}

export class View {}

export class StockFlowView extends View {
  readonly elements: ViewElement[];

  constructor(view: pb.View) {
    super();
    this.elements = view.getElementsList().map((element) => {
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
    });
  }
}

export class Model {
  readonly name: string;
  readonly variables: Map<string, Variable>;
  readonly views: View[];

  constructor(model: pb.Model) {
    this.name = model.getName();
    this.variables = new Map(
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
    );
    this.views = model.getViewsList().map((view) => {
      switch (view.getKind()) {
        case pb.View.ViewType.STOCK_FLOW:
          return new StockFlowView(view);
        default:
          throw new Error('invariant broken: protobuf view with unknown kind');
      }
    });
  }
}

export class Dt {
  readonly dt: number;
  readonly isReciprocal: boolean;

  constructor(dt: pb.Dt) {
    this.dt = dt.getValue();
    this.isReciprocal = dt.getIsReciprocal();
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

export class SimSpecs {
  readonly start: number;
  readonly stop: number;
  readonly dt: Dt;
  readonly saveStep?: Dt;
  readonly simMethod: SimMethod;
  readonly timeUnits?: string;

  constructor(simSpecs: pb.SimSpecs) {
    this.start = simSpecs.getStart();
    this.stop = simSpecs.getStop();
    this.dt = new Dt(defined(simSpecs.getDt()));
    const saveStep = simSpecs.getSaveStep();
    this.saveStep = saveStep ? new Dt(saveStep) : undefined;
    this.simMethod = getSimMethod(simSpecs.getSimMethod());
    this.timeUnits = simSpecs.getTimeUnits();
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

export class Project {
  readonly name: string;
  readonly simSpecs: SimSpecs;
  readonly models: Map<string, Model>;

  constructor(project: pb.Project) {
    this.name = project.getName();
    this.simSpecs = new SimSpecs(defined(project.getSimSpecs()));
    this.models = new Map(project.getModelsList().map((model) => [model.getName(), new Model(model)]));
  }
}
