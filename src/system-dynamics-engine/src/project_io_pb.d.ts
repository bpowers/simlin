// package: project_io
// file: src/system-dynamics-engine/src/project_io.proto

import * as jspb from 'google-protobuf';

export class GraphicalFunction extends jspb.Message {
  getKind(): GraphicalFunction.KindMap[keyof GraphicalFunction.KindMap];
  setKind(value: GraphicalFunction.KindMap[keyof GraphicalFunction.KindMap]): void;

  clearXpointsList(): void;
  getXpointsList(): Array<number>;
  setXpointsList(value: Array<number>): void;
  addXpoints(value: number, index?: number): number;

  clearYpointsList(): void;
  getYpointsList(): Array<number>;
  setYpointsList(value: Array<number>): void;
  addYpoints(value: number, index?: number): number;

  hasXscale(): boolean;
  clearXscale(): void;
  getXscale(): GraphicalFunction.Scale | undefined;
  setXscale(value?: GraphicalFunction.Scale): void;

  hasYscale(): boolean;
  clearYscale(): void;
  getYscale(): GraphicalFunction.Scale | undefined;
  setYscale(value?: GraphicalFunction.Scale): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): GraphicalFunction.AsObject;
  static toObject(includeInstance: boolean, msg: GraphicalFunction): GraphicalFunction.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: GraphicalFunction, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): GraphicalFunction;
  static deserializeBinaryFromReader(message: GraphicalFunction, reader: jspb.BinaryReader): GraphicalFunction;
}

export namespace GraphicalFunction {
  export type AsObject = {
    kind: GraphicalFunction.KindMap[keyof GraphicalFunction.KindMap];
    xpointsList: Array<number>;
    ypointsList: Array<number>;
    xscale?: GraphicalFunction.Scale.AsObject;
    yscale?: GraphicalFunction.Scale.AsObject;
  };

  export class Scale extends jspb.Message {
    getMin(): number;
    setMin(value: number): void;

    getMax(): number;
    setMax(value: number): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Scale.AsObject;
    static toObject(includeInstance: boolean, msg: Scale): Scale.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Scale, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Scale;
    static deserializeBinaryFromReader(message: Scale, reader: jspb.BinaryReader): Scale;
  }

  export namespace Scale {
    export type AsObject = {
      min: number;
      max: number;
    };
  }

  export interface KindMap {
    CONTINUOUS: 0;
    DISCRETE: 1;
    EXTRAPOLATE: 2;
  }

  export const Kind: KindMap;
}

export class Variable extends jspb.Message {
  hasStock(): boolean;
  clearStock(): void;
  getStock(): Variable.Stock | undefined;
  setStock(value?: Variable.Stock): void;

  hasFlow(): boolean;
  clearFlow(): void;
  getFlow(): Variable.Flow | undefined;
  setFlow(value?: Variable.Flow): void;

  hasAux(): boolean;
  clearAux(): void;
  getAux(): Variable.Aux | undefined;
  setAux(value?: Variable.Aux): void;

  hasModule(): boolean;
  clearModule(): void;
  getModule(): Variable.Module | undefined;
  setModule(value?: Variable.Module): void;

  getVCase(): Variable.VCase;
  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Variable.AsObject;
  static toObject(includeInstance: boolean, msg: Variable): Variable.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Variable, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Variable;
  static deserializeBinaryFromReader(message: Variable, reader: jspb.BinaryReader): Variable;
}

export namespace Variable {
  export type AsObject = {
    stock?: Variable.Stock.AsObject;
    flow?: Variable.Flow.AsObject;
    aux?: Variable.Aux.AsObject;
    module?: Variable.Module.AsObject;
  };

  export class Stock extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    getEquation(): string;
    setEquation(value: string): void;

    getDocumentation(): string;
    setDocumentation(value: string): void;

    getUnits(): string;
    setUnits(value: string): void;

    clearInflowsList(): void;
    getInflowsList(): Array<string>;
    setInflowsList(value: Array<string>): void;
    addInflows(value: string, index?: number): string;

    clearOutflowsList(): void;
    getOutflowsList(): Array<string>;
    setOutflowsList(value: Array<string>): void;
    addOutflows(value: string, index?: number): string;

    getNonNegative(): boolean;
    setNonNegative(value: boolean): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Stock.AsObject;
    static toObject(includeInstance: boolean, msg: Stock): Stock.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Stock, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Stock;
    static deserializeBinaryFromReader(message: Stock, reader: jspb.BinaryReader): Stock;
  }

  export namespace Stock {
    export type AsObject = {
      ident: string;
      equation: string;
      documentation: string;
      units: string;
      inflowsList: Array<string>;
      outflowsList: Array<string>;
      nonNegative: boolean;
    };
  }

  export class Flow extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    getEquation(): string;
    setEquation(value: string): void;

    getDocumentation(): string;
    setDocumentation(value: string): void;

    getUnits(): string;
    setUnits(value: string): void;

    hasGf(): boolean;
    clearGf(): void;
    getGf(): GraphicalFunction | undefined;
    setGf(value?: GraphicalFunction): void;

    getNonNegative(): boolean;
    setNonNegative(value: boolean): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Flow.AsObject;
    static toObject(includeInstance: boolean, msg: Flow): Flow.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Flow, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Flow;
    static deserializeBinaryFromReader(message: Flow, reader: jspb.BinaryReader): Flow;
  }

  export namespace Flow {
    export type AsObject = {
      ident: string;
      equation: string;
      documentation: string;
      units: string;
      gf?: GraphicalFunction.AsObject;
      nonNegative: boolean;
    };
  }

  export class Aux extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    getEquation(): string;
    setEquation(value: string): void;

    getDocumentation(): string;
    setDocumentation(value: string): void;

    getUnits(): string;
    setUnits(value: string): void;

    hasGf(): boolean;
    clearGf(): void;
    getGf(): GraphicalFunction | undefined;
    setGf(value?: GraphicalFunction): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Aux.AsObject;
    static toObject(includeInstance: boolean, msg: Aux): Aux.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Aux, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Aux;
    static deserializeBinaryFromReader(message: Aux, reader: jspb.BinaryReader): Aux;
  }

  export namespace Aux {
    export type AsObject = {
      ident: string;
      equation: string;
      documentation: string;
      units: string;
      gf?: GraphicalFunction.AsObject;
    };
  }

  export class Module extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    getModelName(): string;
    setModelName(value: string): void;

    getDocumentation(): string;
    setDocumentation(value: string): void;

    getUnits(): string;
    setUnits(value: string): void;

    clearReferencesList(): void;
    getReferencesList(): Array<Variable.Module.Reference>;
    setReferencesList(value: Array<Variable.Module.Reference>): void;
    addReferences(value?: Variable.Module.Reference, index?: number): Variable.Module.Reference;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Module.AsObject;
    static toObject(includeInstance: boolean, msg: Module): Module.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Module, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Module;
    static deserializeBinaryFromReader(message: Module, reader: jspb.BinaryReader): Module;
  }

  export namespace Module {
    export type AsObject = {
      ident: string;
      modelName: string;
      documentation: string;
      units: string;
      referencesList: Array<Variable.Module.Reference.AsObject>;
    };

    export class Reference extends jspb.Message {
      getSrc(): string;
      setSrc(value: string): void;

      getDst(): string;
      setDst(value: string): void;

      serializeBinary(): Uint8Array;
      toObject(includeInstance?: boolean): Reference.AsObject;
      static toObject(includeInstance: boolean, msg: Reference): Reference.AsObject;
      static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
      static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
      static serializeBinaryToWriter(message: Reference, writer: jspb.BinaryWriter): void;
      static deserializeBinary(bytes: Uint8Array): Reference;
      static deserializeBinaryFromReader(message: Reference, reader: jspb.BinaryReader): Reference;
    }

    export namespace Reference {
      export type AsObject = {
        src: string;
        dst: string;
      };
    }
  }

  export enum VCase {
    V_NOT_SET = 0,
    STOCK = 1,
    FLOW = 2,
    AUX = 3,
    MODULE = 4,
  }
}

export class ViewElement extends jspb.Message {
  hasAux(): boolean;
  clearAux(): void;
  getAux(): ViewElement.Aux | undefined;
  setAux(value?: ViewElement.Aux): void;

  hasStock(): boolean;
  clearStock(): void;
  getStock(): ViewElement.Stock | undefined;
  setStock(value?: ViewElement.Stock): void;

  hasFlow(): boolean;
  clearFlow(): void;
  getFlow(): ViewElement.Flow | undefined;
  setFlow(value?: ViewElement.Flow): void;

  hasLink(): boolean;
  clearLink(): void;
  getLink(): ViewElement.Link | undefined;
  setLink(value?: ViewElement.Link): void;

  hasModule(): boolean;
  clearModule(): void;
  getModule(): ViewElement.Module | undefined;
  setModule(value?: ViewElement.Module): void;

  hasAlias(): boolean;
  clearAlias(): void;
  getAlias(): ViewElement.Alias | undefined;
  setAlias(value?: ViewElement.Alias): void;

  hasCloud(): boolean;
  clearCloud(): void;
  getCloud(): ViewElement.Cloud | undefined;
  setCloud(value?: ViewElement.Cloud): void;

  getElementCase(): ViewElement.ElementCase;
  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): ViewElement.AsObject;
  static toObject(includeInstance: boolean, msg: ViewElement): ViewElement.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: ViewElement, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): ViewElement;
  static deserializeBinaryFromReader(message: ViewElement, reader: jspb.BinaryReader): ViewElement;
}

export namespace ViewElement {
  export type AsObject = {
    aux?: ViewElement.Aux.AsObject;
    stock?: ViewElement.Stock.AsObject;
    flow?: ViewElement.Flow.AsObject;
    link?: ViewElement.Link.AsObject;
    module?: ViewElement.Module.AsObject;
    alias?: ViewElement.Alias.AsObject;
    cloud?: ViewElement.Cloud.AsObject;
  };

  export class Aux extends jspb.Message {
    getName(): string;
    setName(value: string): void;

    getUid(): number;
    setUid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getLabelSide(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelSide(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Aux.AsObject;
    static toObject(includeInstance: boolean, msg: Aux): Aux.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Aux, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Aux;
    static deserializeBinaryFromReader(message: Aux, reader: jspb.BinaryReader): Aux;
  }

  export namespace Aux {
    export type AsObject = {
      name: string;
      uid: number;
      x: number;
      y: number;
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    };
  }

  export class Stock extends jspb.Message {
    getName(): string;
    setName(value: string): void;

    getUid(): number;
    setUid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getLabelSide(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelSide(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Stock.AsObject;
    static toObject(includeInstance: boolean, msg: Stock): Stock.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Stock, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Stock;
    static deserializeBinaryFromReader(message: Stock, reader: jspb.BinaryReader): Stock;
  }

  export namespace Stock {
    export type AsObject = {
      name: string;
      uid: number;
      x: number;
      y: number;
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    };
  }

  export class FlowPoint extends jspb.Message {
    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getAttachedtouid(): number;
    setAttachedtouid(value: number): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): FlowPoint.AsObject;
    static toObject(includeInstance: boolean, msg: FlowPoint): FlowPoint.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: FlowPoint, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): FlowPoint;
    static deserializeBinaryFromReader(message: FlowPoint, reader: jspb.BinaryReader): FlowPoint;
  }

  export namespace FlowPoint {
    export type AsObject = {
      x: number;
      y: number;
      attachedtouid: number;
    };
  }

  export class Flow extends jspb.Message {
    getName(): string;
    setName(value: string): void;

    getUid(): number;
    setUid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getLabelside(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelside(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

    clearPointsList(): void;
    getPointsList(): Array<ViewElement.FlowPoint>;
    setPointsList(value: Array<ViewElement.FlowPoint>): void;
    addPoints(value?: ViewElement.FlowPoint, index?: number): ViewElement.FlowPoint;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Flow.AsObject;
    static toObject(includeInstance: boolean, msg: Flow): Flow.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Flow, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Flow;
    static deserializeBinaryFromReader(message: Flow, reader: jspb.BinaryReader): Flow;
  }

  export namespace Flow {
    export type AsObject = {
      name: string;
      uid: number;
      x: number;
      y: number;
      labelside: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
      pointsList: Array<ViewElement.FlowPoint.AsObject>;
    };
  }

  export class Link extends jspb.Message {
    getUid(): number;
    setUid(value: number): void;

    getFromuid(): number;
    setFromuid(value: number): void;

    getTouid(): number;
    setTouid(value: number): void;

    hasArc(): boolean;
    clearArc(): void;
    getArc(): number;
    setArc(value: number): void;

    hasIsstraight(): boolean;
    clearIsstraight(): void;
    getIsstraight(): boolean;
    setIsstraight(value: boolean): void;

    hasMultipoint(): boolean;
    clearMultipoint(): void;
    getMultipoint(): ViewElement.Link.LinkPoints | undefined;
    setMultipoint(value?: ViewElement.Link.LinkPoints): void;

    getShapeCase(): Link.ShapeCase;
    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Link.AsObject;
    static toObject(includeInstance: boolean, msg: Link): Link.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Link, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Link;
    static deserializeBinaryFromReader(message: Link, reader: jspb.BinaryReader): Link;
  }

  export namespace Link {
    export type AsObject = {
      uid: number;
      fromuid: number;
      touid: number;
      arc: number;
      isstraight: boolean;
      multipoint?: ViewElement.Link.LinkPoints.AsObject;
    };

    export class LinkPoints extends jspb.Message {
      clearPointsList(): void;
      getPointsList(): Array<ViewElement.FlowPoint>;
      setPointsList(value: Array<ViewElement.FlowPoint>): void;
      addPoints(value?: ViewElement.FlowPoint, index?: number): ViewElement.FlowPoint;

      serializeBinary(): Uint8Array;
      toObject(includeInstance?: boolean): LinkPoints.AsObject;
      static toObject(includeInstance: boolean, msg: LinkPoints): LinkPoints.AsObject;
      static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
      static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
      static serializeBinaryToWriter(message: LinkPoints, writer: jspb.BinaryWriter): void;
      static deserializeBinary(bytes: Uint8Array): LinkPoints;
      static deserializeBinaryFromReader(message: LinkPoints, reader: jspb.BinaryReader): LinkPoints;
    }

    export namespace LinkPoints {
      export type AsObject = {
        pointsList: Array<ViewElement.FlowPoint.AsObject>;
      };
    }

    export enum ShapeCase {
      SHAPE_NOT_SET = 0,
      ARC = 4,
      ISSTRAIGHT = 5,
      MULTIPOINT = 6,
    }
  }

  export class Module extends jspb.Message {
    getName(): string;
    setName(value: string): void;

    getUid(): number;
    setUid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getLabelSide(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelSide(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Module.AsObject;
    static toObject(includeInstance: boolean, msg: Module): Module.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Module, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Module;
    static deserializeBinaryFromReader(message: Module, reader: jspb.BinaryReader): Module;
  }

  export namespace Module {
    export type AsObject = {
      name: string;
      uid: number;
      x: number;
      y: number;
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    };
  }

  export class Alias extends jspb.Message {
    getUid(): number;
    setUid(value: number): void;

    getAliasofuid(): number;
    setAliasofuid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    getLabelSide(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelSide(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Alias.AsObject;
    static toObject(includeInstance: boolean, msg: Alias): Alias.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Alias, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Alias;
    static deserializeBinaryFromReader(message: Alias, reader: jspb.BinaryReader): Alias;
  }

  export namespace Alias {
    export type AsObject = {
      uid: number;
      aliasofuid: number;
      x: number;
      y: number;
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    };
  }

  export class Cloud extends jspb.Message {
    getUid(): number;
    setUid(value: number): void;

    getFlowuid(): number;
    setFlowuid(value: number): void;

    getX(): number;
    setX(value: number): void;

    getY(): number;
    setY(value: number): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Cloud.AsObject;
    static toObject(includeInstance: boolean, msg: Cloud): Cloud.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Cloud, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Cloud;
    static deserializeBinaryFromReader(message: Cloud, reader: jspb.BinaryReader): Cloud;
  }

  export namespace Cloud {
    export type AsObject = {
      uid: number;
      flowuid: number;
      x: number;
      y: number;
    };
  }

  export interface LabelSideMap {
    TOP: 0;
    LEFT: 1;
    CENTER: 2;
    BOTTOM: 3;
    RIGHT: 4;
  }

  export const LabelSide: LabelSideMap;

  export enum ElementCase {
    ELEMENT_NOT_SET = 0,
    AUX = 1,
    STOCK = 2,
    FLOW = 3,
    LINK = 4,
    MODULE = 5,
    ALIAS = 6,
    CLOUD = 7,
  }
}

export class View extends jspb.Message {
  getKind(): View.ViewTypeMap[keyof View.ViewTypeMap];
  setKind(value: View.ViewTypeMap[keyof View.ViewTypeMap]): void;

  clearElementsList(): void;
  getElementsList(): Array<ViewElement>;
  setElementsList(value: Array<ViewElement>): void;
  addElements(value?: ViewElement, index?: number): ViewElement;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): View.AsObject;
  static toObject(includeInstance: boolean, msg: View): View.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: View, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): View;
  static deserializeBinaryFromReader(message: View, reader: jspb.BinaryReader): View;
}

export namespace View {
  export type AsObject = {
    kind: View.ViewTypeMap[keyof View.ViewTypeMap];
    elementsList: Array<ViewElement.AsObject>;
  };

  export interface ViewTypeMap {
    STOCK_FLOW: 0;
  }

  export const ViewType: ViewTypeMap;
}

export class Model extends jspb.Message {
  getName(): string;
  setName(value: string): void;

  clearVariablesList(): void;
  getVariablesList(): Array<Variable>;
  setVariablesList(value: Array<Variable>): void;
  addVariables(value?: Variable, index?: number): Variable;

  clearViewsList(): void;
  getViewsList(): Array<View>;
  setViewsList(value: Array<View>): void;
  addViews(value?: View, index?: number): View;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Model.AsObject;
  static toObject(includeInstance: boolean, msg: Model): Model.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Model, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Model;
  static deserializeBinaryFromReader(message: Model, reader: jspb.BinaryReader): Model;
}

export namespace Model {
  export type AsObject = {
    name: string;
    variablesList: Array<Variable.AsObject>;
    viewsList: Array<View.AsObject>;
  };
}

export class Dt extends jspb.Message {
  getValue(): number;
  setValue(value: number): void;

  getIsreciprocal(): boolean;
  setIsreciprocal(value: boolean): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Dt.AsObject;
  static toObject(includeInstance: boolean, msg: Dt): Dt.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Dt, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Dt;
  static deserializeBinaryFromReader(message: Dt, reader: jspb.BinaryReader): Dt;
}

export namespace Dt {
  export type AsObject = {
    value: number;
    isreciprocal: boolean;
  };
}

export class SimSpecs extends jspb.Message {
  getStart(): number;
  setStart(value: number): void;

  getStop(): number;
  setStop(value: number): void;

  hasDt(): boolean;
  clearDt(): void;
  getDt(): Dt | undefined;
  setDt(value?: Dt): void;

  hasSavestep(): boolean;
  clearSavestep(): void;
  getSavestep(): Dt | undefined;
  setSavestep(value?: Dt): void;

  getSimmethod(): SimMethodMap[keyof SimMethodMap];
  setSimmethod(value: SimMethodMap[keyof SimMethodMap]): void;

  getTimeunits(): string;
  setTimeunits(value: string): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): SimSpecs.AsObject;
  static toObject(includeInstance: boolean, msg: SimSpecs): SimSpecs.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: SimSpecs, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): SimSpecs;
  static deserializeBinaryFromReader(message: SimSpecs, reader: jspb.BinaryReader): SimSpecs;
}

export namespace SimSpecs {
  export type AsObject = {
    start: number;
    stop: number;
    dt?: Dt.AsObject;
    savestep?: Dt.AsObject;
    simmethod: SimMethodMap[keyof SimMethodMap];
    timeunits: string;
  };
}

export class Project extends jspb.Message {
  getName(): string;
  setName(value: string): void;

  hasSimspecs(): boolean;
  clearSimspecs(): void;
  getSimspecs(): SimSpecs | undefined;
  setSimspecs(value?: SimSpecs): void;

  clearModelsList(): void;
  getModelsList(): Array<Model>;
  setModelsList(value: Array<Model>): void;
  addModels(value?: Model, index?: number): Model;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Project.AsObject;
  static toObject(includeInstance: boolean, msg: Project): Project.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Project, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Project;
  static deserializeBinaryFromReader(message: Project, reader: jspb.BinaryReader): Project;
}

export namespace Project {
  export type AsObject = {
    name: string;
    simspecs?: SimSpecs.AsObject;
    modelsList: Array<Model.AsObject>;
  };
}

export interface SimMethodMap {
  EULER: 0;
  RUNGE_KUTTA_4: 1;
}

export const SimMethod: SimMethodMap;
