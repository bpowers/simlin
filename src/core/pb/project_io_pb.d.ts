// package: project_io
// file: src/simlin-engine/src/project_io.proto

import * as jspb from 'google-protobuf';

export class GraphicalFunction extends jspb.Message {
  getKind(): GraphicalFunction.KindMap[keyof GraphicalFunction.KindMap];
  setKind(value: GraphicalFunction.KindMap[keyof GraphicalFunction.KindMap]): void;

  clearXPointsList(): void;
  getXPointsList(): Array<number>;
  setXPointsList(value: Array<number>): void;
  addXPoints(value: number, index?: number): number;

  clearYPointsList(): void;
  getYPointsList(): Array<number>;
  setYPointsList(value: Array<number>): void;
  addYPoints(value: number, index?: number): number;

  hasXScale(): boolean;
  clearXScale(): void;
  getXScale(): GraphicalFunction.Scale | undefined;
  setXScale(value?: GraphicalFunction.Scale): void;

  hasYScale(): boolean;
  clearYScale(): void;
  getYScale(): GraphicalFunction.Scale | undefined;
  setYScale(value?: GraphicalFunction.Scale): void;

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
    xPointsList: Array<number>;
    yPointsList: Array<number>;
    xScale?: GraphicalFunction.Scale.AsObject;
    yScale?: GraphicalFunction.Scale.AsObject;
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

  export class ScalarEquation extends jspb.Message {
    getEquation(): string;
    setEquation(value: string): void;

    hasInitialEquation(): boolean;
    clearInitialEquation(): void;
    getInitialEquation(): string;
    setInitialEquation(value: string): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): ScalarEquation.AsObject;
    static toObject(includeInstance: boolean, msg: ScalarEquation): ScalarEquation.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: ScalarEquation, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): ScalarEquation;
    static deserializeBinaryFromReader(message: ScalarEquation, reader: jspb.BinaryReader): ScalarEquation;
  }

  export namespace ScalarEquation {
    export type AsObject = {
      equation: string;
      initialEquation: string;
    };
  }

  export class ApplyToAllEquation extends jspb.Message {
    clearDimensionNamesList(): void;
    getDimensionNamesList(): Array<string>;
    setDimensionNamesList(value: Array<string>): void;
    addDimensionNames(value: string, index?: number): string;

    getEquation(): string;
    setEquation(value: string): void;

    hasInitialEquation(): boolean;
    clearInitialEquation(): void;
    getInitialEquation(): string;
    setInitialEquation(value: string): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): ApplyToAllEquation.AsObject;
    static toObject(includeInstance: boolean, msg: ApplyToAllEquation): ApplyToAllEquation.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: ApplyToAllEquation, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): ApplyToAllEquation;
    static deserializeBinaryFromReader(message: ApplyToAllEquation, reader: jspb.BinaryReader): ApplyToAllEquation;
  }

  export namespace ApplyToAllEquation {
    export type AsObject = {
      dimensionNamesList: Array<string>;
      equation: string;
      initialEquation: string;
    };
  }

  export class ArrayedEquation extends jspb.Message {
    clearDimensionNamesList(): void;
    getDimensionNamesList(): Array<string>;
    setDimensionNamesList(value: Array<string>): void;
    addDimensionNames(value: string, index?: number): string;

    clearElementsList(): void;
    getElementsList(): Array<Variable.ArrayedEquation.Element>;
    setElementsList(value: Array<Variable.ArrayedEquation.Element>): void;
    addElements(value?: Variable.ArrayedEquation.Element, index?: number): Variable.ArrayedEquation.Element;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): ArrayedEquation.AsObject;
    static toObject(includeInstance: boolean, msg: ArrayedEquation): ArrayedEquation.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: ArrayedEquation, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): ArrayedEquation;
    static deserializeBinaryFromReader(message: ArrayedEquation, reader: jspb.BinaryReader): ArrayedEquation;
  }

  export namespace ArrayedEquation {
    export type AsObject = {
      dimensionNamesList: Array<string>;
      elementsList: Array<Variable.ArrayedEquation.Element.AsObject>;
    };

    export class Element extends jspb.Message {
      getSubscript(): string;
      setSubscript(value: string): void;

      getEquation(): string;
      setEquation(value: string): void;

      hasInitialEquation(): boolean;
      clearInitialEquation(): void;
      getInitialEquation(): string;
      setInitialEquation(value: string): void;

      serializeBinary(): Uint8Array;
      toObject(includeInstance?: boolean): Element.AsObject;
      static toObject(includeInstance: boolean, msg: Element): Element.AsObject;
      static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
      static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
      static serializeBinaryToWriter(message: Element, writer: jspb.BinaryWriter): void;
      static deserializeBinary(bytes: Uint8Array): Element;
      static deserializeBinaryFromReader(message: Element, reader: jspb.BinaryReader): Element;
    }

    export namespace Element {
      export type AsObject = {
        subscript: string;
        equation: string;
        initialEquation: string;
      };
    }
  }

  export class Equation extends jspb.Message {
    hasScalar(): boolean;
    clearScalar(): void;
    getScalar(): Variable.ScalarEquation | undefined;
    setScalar(value?: Variable.ScalarEquation): void;

    hasApplyToAll(): boolean;
    clearApplyToAll(): void;
    getApplyToAll(): Variable.ApplyToAllEquation | undefined;
    setApplyToAll(value?: Variable.ApplyToAllEquation): void;

    hasArrayed(): boolean;
    clearArrayed(): void;
    getArrayed(): Variable.ArrayedEquation | undefined;
    setArrayed(value?: Variable.ArrayedEquation): void;

    getEquationCase(): Equation.EquationCase;
    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Equation.AsObject;
    static toObject(includeInstance: boolean, msg: Equation): Equation.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: Equation, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Equation;
    static deserializeBinaryFromReader(message: Equation, reader: jspb.BinaryReader): Equation;
  }

  export namespace Equation {
    export type AsObject = {
      scalar?: Variable.ScalarEquation.AsObject;
      applyToAll?: Variable.ApplyToAllEquation.AsObject;
      arrayed?: Variable.ArrayedEquation.AsObject;
    };

    export enum EquationCase {
      EQUATION_NOT_SET = 0,
      SCALAR = 1,
      APPLY_TO_ALL = 2,
      ARRAYED = 3,
    }
  }

  export class Stock extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    hasEquation(): boolean;
    clearEquation(): void;
    getEquation(): Variable.Equation | undefined;
    setEquation(value?: Variable.Equation): void;

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

    getCanBeModuleInput(): boolean;
    setCanBeModuleInput(value: boolean): void;

    getVisibility(): Variable.VisibilityMap[keyof Variable.VisibilityMap];
    setVisibility(value: Variable.VisibilityMap[keyof Variable.VisibilityMap]): void;

    getUid(): number;
    setUid(value: number): void;

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
      equation?: Variable.Equation.AsObject;
      documentation: string;
      units: string;
      inflowsList: Array<string>;
      outflowsList: Array<string>;
      nonNegative: boolean;
      canBeModuleInput: boolean;
      visibility: Variable.VisibilityMap[keyof Variable.VisibilityMap];
      uid: number;
    };
  }

  export class Flow extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    hasEquation(): boolean;
    clearEquation(): void;
    getEquation(): Variable.Equation | undefined;
    setEquation(value?: Variable.Equation): void;

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

    getCanBeModuleInput(): boolean;
    setCanBeModuleInput(value: boolean): void;

    getVisibility(): Variable.VisibilityMap[keyof Variable.VisibilityMap];
    setVisibility(value: Variable.VisibilityMap[keyof Variable.VisibilityMap]): void;

    getUid(): number;
    setUid(value: number): void;

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
      equation?: Variable.Equation.AsObject;
      documentation: string;
      units: string;
      gf?: GraphicalFunction.AsObject;
      nonNegative: boolean;
      canBeModuleInput: boolean;
      visibility: Variable.VisibilityMap[keyof Variable.VisibilityMap];
      uid: number;
    };
  }

  export class Aux extends jspb.Message {
    getIdent(): string;
    setIdent(value: string): void;

    hasEquation(): boolean;
    clearEquation(): void;
    getEquation(): Variable.Equation | undefined;
    setEquation(value?: Variable.Equation): void;

    getDocumentation(): string;
    setDocumentation(value: string): void;

    getUnits(): string;
    setUnits(value: string): void;

    hasGf(): boolean;
    clearGf(): void;
    getGf(): GraphicalFunction | undefined;
    setGf(value?: GraphicalFunction): void;

    getCanBeModuleInput(): boolean;
    setCanBeModuleInput(value: boolean): void;

    getVisibility(): Variable.VisibilityMap[keyof Variable.VisibilityMap];
    setVisibility(value: Variable.VisibilityMap[keyof Variable.VisibilityMap]): void;

    getUid(): number;
    setUid(value: number): void;

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
      equation?: Variable.Equation.AsObject;
      documentation: string;
      units: string;
      gf?: GraphicalFunction.AsObject;
      canBeModuleInput: boolean;
      visibility: Variable.VisibilityMap[keyof Variable.VisibilityMap];
      uid: number;
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

    getCanBeModuleInput(): boolean;
    setCanBeModuleInput(value: boolean): void;

    getVisibility(): Variable.VisibilityMap[keyof Variable.VisibilityMap];
    setVisibility(value: Variable.VisibilityMap[keyof Variable.VisibilityMap]): void;

    getUid(): number;
    setUid(value: number): void;

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
      canBeModuleInput: boolean;
      visibility: Variable.VisibilityMap[keyof Variable.VisibilityMap];
      uid: number;
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

  export interface VisibilityMap {
    PRIVATE: 0;
    PUBLIC: 1;
  }

  export const Visibility: VisibilityMap;

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

    getAttachedToUid(): number;
    setAttachedToUid(value: number): void;

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
      attachedToUid: number;
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

    getLabelSide(): ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    setLabelSide(value: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap]): void;

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
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
      pointsList: Array<ViewElement.FlowPoint.AsObject>;
    };
  }

  export class Link extends jspb.Message {
    getUid(): number;
    setUid(value: number): void;

    getFromUid(): number;
    setFromUid(value: number): void;

    getToUid(): number;
    setToUid(value: number): void;

    hasArc(): boolean;
    clearArc(): void;
    getArc(): number;
    setArc(value: number): void;

    hasIsStraight(): boolean;
    clearIsStraight(): void;
    getIsStraight(): boolean;
    setIsStraight(value: boolean): void;

    hasMultiPoint(): boolean;
    clearMultiPoint(): void;
    getMultiPoint(): ViewElement.Link.LinkPoints | undefined;
    setMultiPoint(value?: ViewElement.Link.LinkPoints): void;

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
      fromUid: number;
      toUid: number;
      arc: number;
      isStraight: boolean;
      multiPoint?: ViewElement.Link.LinkPoints.AsObject;
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
      IS_STRAIGHT = 5,
      MULTI_POINT = 6,
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

    getAliasOfUid(): number;
    setAliasOfUid(value: number): void;

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
      aliasOfUid: number;
      x: number;
      y: number;
      labelSide: ViewElement.LabelSideMap[keyof ViewElement.LabelSideMap];
    };
  }

  export class Cloud extends jspb.Message {
    getUid(): number;
    setUid(value: number): void;

    getFlowUid(): number;
    setFlowUid(value: number): void;

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
      flowUid: number;
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

export class Rect extends jspb.Message {
  getX(): number;
  setX(value: number): void;

  getY(): number;
  setY(value: number): void;

  getWidth(): number;
  setWidth(value: number): void;

  getHeight(): number;
  setHeight(value: number): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Rect.AsObject;
  static toObject(includeInstance: boolean, msg: Rect): Rect.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Rect, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Rect;
  static deserializeBinaryFromReader(message: Rect, reader: jspb.BinaryReader): Rect;
}

export namespace Rect {
  export type AsObject = {
    x: number;
    y: number;
    width: number;
    height: number;
  };
}

export class View extends jspb.Message {
  getKind(): View.ViewTypeMap[keyof View.ViewTypeMap];
  setKind(value: View.ViewTypeMap[keyof View.ViewTypeMap]): void;

  clearElementsList(): void;
  getElementsList(): Array<ViewElement>;
  setElementsList(value: Array<ViewElement>): void;
  addElements(value?: ViewElement, index?: number): ViewElement;

  hasViewbox(): boolean;
  clearViewbox(): void;
  getViewbox(): Rect | undefined;
  setViewbox(value?: Rect): void;

  getZoom(): number;
  setZoom(value: number): void;

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
    viewbox?: Rect.AsObject;
    zoom: number;
  };

  export interface ViewTypeMap {
    STOCK_FLOW: 0;
  }

  export const ViewType: ViewTypeMap;
}

export class LoopMetadata extends jspb.Message {
  clearUidsList(): void;
  getUidsList(): Array<number>;
  setUidsList(value: Array<number>): void;
  addUids(value: number, index?: number): number;

  getDeleted(): boolean;
  setDeleted(value: boolean): void;

  getName(): string;
  setName(value: string): void;

  getDescription(): string;
  setDescription(value: string): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): LoopMetadata.AsObject;
  static toObject(includeInstance: boolean, msg: LoopMetadata): LoopMetadata.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: LoopMetadata, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): LoopMetadata;
  static deserializeBinaryFromReader(message: LoopMetadata, reader: jspb.BinaryReader): LoopMetadata;
}

export namespace LoopMetadata {
  export type AsObject = {
    uidsList: Array<number>;
    deleted: boolean;
    name: string;
    description: string;
  };
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

  clearLoopMetadataList(): void;
  getLoopMetadataList(): Array<LoopMetadata>;
  setLoopMetadataList(value: Array<LoopMetadata>): void;
  addLoopMetadata(value?: LoopMetadata, index?: number): LoopMetadata;

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
    loopMetadataList: Array<LoopMetadata.AsObject>;
  };
}

export class Dt extends jspb.Message {
  getValue(): number;
  setValue(value: number): void;

  getIsReciprocal(): boolean;
  setIsReciprocal(value: boolean): void;

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
    isReciprocal: boolean;
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

  hasSaveStep(): boolean;
  clearSaveStep(): void;
  getSaveStep(): Dt | undefined;
  setSaveStep(value?: Dt): void;

  getSimMethod(): SimMethodMap[keyof SimMethodMap];
  setSimMethod(value: SimMethodMap[keyof SimMethodMap]): void;

  getTimeUnits(): string;
  setTimeUnits(value: string): void;

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
    saveStep?: Dt.AsObject;
    simMethod: SimMethodMap[keyof SimMethodMap];
    timeUnits: string;
  };
}

export class Dimension extends jspb.Message {
  getName(): string;
  setName(value: string): void;

  clearObsoleteElementsList(): void;
  getObsoleteElementsList(): Array<string>;
  setObsoleteElementsList(value: Array<string>): void;
  addObsoleteElements(value: string, index?: number): string;

  hasElements(): boolean;
  clearElements(): void;
  getElements(): Dimension.DimensionElements | undefined;
  setElements(value?: Dimension.DimensionElements): void;

  hasSize(): boolean;
  clearSize(): void;
  getSize(): Dimension.DimensionSize | undefined;
  setSize(value?: Dimension.DimensionSize): void;

  getDimensionCase(): Dimension.DimensionCase;
  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Dimension.AsObject;
  static toObject(includeInstance: boolean, msg: Dimension): Dimension.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Dimension, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Dimension;
  static deserializeBinaryFromReader(message: Dimension, reader: jspb.BinaryReader): Dimension;
}

export namespace Dimension {
  export type AsObject = {
    name: string;
    obsoleteElementsList: Array<string>;
    elements?: Dimension.DimensionElements.AsObject;
    size?: Dimension.DimensionSize.AsObject;
  };

  export class DimensionElements extends jspb.Message {
    clearElementsList(): void;
    getElementsList(): Array<string>;
    setElementsList(value: Array<string>): void;
    addElements(value: string, index?: number): string;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): DimensionElements.AsObject;
    static toObject(includeInstance: boolean, msg: DimensionElements): DimensionElements.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: DimensionElements, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): DimensionElements;
    static deserializeBinaryFromReader(message: DimensionElements, reader: jspb.BinaryReader): DimensionElements;
  }

  export namespace DimensionElements {
    export type AsObject = {
      elementsList: Array<string>;
    };
  }

  export class DimensionSize extends jspb.Message {
    getSize(): number;
    setSize(value: number): void;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): DimensionSize.AsObject;
    static toObject(includeInstance: boolean, msg: DimensionSize): DimensionSize.AsObject;
    static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
    static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
    static serializeBinaryToWriter(message: DimensionSize, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): DimensionSize;
    static deserializeBinaryFromReader(message: DimensionSize, reader: jspb.BinaryReader): DimensionSize;
  }

  export namespace DimensionSize {
    export type AsObject = {
      size: number;
    };
  }

  export enum DimensionCase {
    DIMENSION_NOT_SET = 0,
    ELEMENTS = 3,
    SIZE = 4,
  }
}

export class Unit extends jspb.Message {
  getName(): string;
  setName(value: string): void;

  getEquation(): string;
  setEquation(value: string): void;

  getDisabled(): boolean;
  setDisabled(value: boolean): void;

  clearAliasList(): void;
  getAliasList(): Array<string>;
  setAliasList(value: Array<string>): void;
  addAlias(value: string, index?: number): string;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Unit.AsObject;
  static toObject(includeInstance: boolean, msg: Unit): Unit.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Unit, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Unit;
  static deserializeBinaryFromReader(message: Unit, reader: jspb.BinaryReader): Unit;
}

export namespace Unit {
  export type AsObject = {
    name: string;
    equation: string;
    disabled: boolean;
    aliasList: Array<string>;
  };
}

export class Source extends jspb.Message {
  getExtension$(): Source.ExtensionMap[keyof Source.ExtensionMap];
  setExtension$(value: Source.ExtensionMap[keyof Source.ExtensionMap]): void;

  getContent(): string;
  setContent(value: string): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Source.AsObject;
  static toObject(includeInstance: boolean, msg: Source): Source.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Source, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Source;
  static deserializeBinaryFromReader(message: Source, reader: jspb.BinaryReader): Source;
}

export namespace Source {
  export type AsObject = {
    extension: Source.ExtensionMap[keyof Source.ExtensionMap];
    content: string;
  };

  export interface ExtensionMap {
    UNSPECIFIED: 0;
    XMILE: 1;
    VENSIM: 2;
  }

  export const Extension: ExtensionMap;
}

export class Project extends jspb.Message {
  getName(): string;
  setName(value: string): void;

  hasSimSpecs(): boolean;
  clearSimSpecs(): void;
  getSimSpecs(): SimSpecs | undefined;
  setSimSpecs(value?: SimSpecs): void;

  clearDimensionsList(): void;
  getDimensionsList(): Array<Dimension>;
  setDimensionsList(value: Array<Dimension>): void;
  addDimensions(value?: Dimension, index?: number): Dimension;

  clearUnitsList(): void;
  getUnitsList(): Array<Unit>;
  setUnitsList(value: Array<Unit>): void;
  addUnits(value?: Unit, index?: number): Unit;

  clearModelsList(): void;
  getModelsList(): Array<Model>;
  setModelsList(value: Array<Model>): void;
  addModels(value?: Model, index?: number): Model;

  hasSource(): boolean;
  clearSource(): void;
  getSource(): Source | undefined;
  setSource(value?: Source): void;

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
    simSpecs?: SimSpecs.AsObject;
    dimensionsList: Array<Dimension.AsObject>;
    unitsList: Array<Unit.AsObject>;
    modelsList: Array<Model.AsObject>;
    source?: Source.AsObject;
  };
}

export interface SimMethodMap {
  EULER: 0;
  RUNGE_KUTTA_4: 1;
}

export const SimMethod: SimMethodMap;

export interface BuiltinIdMap {
  UNSPECIFIED: 0;
  ABS: 1;
  ARCCOS: 2;
  ARCSIN: 3;
  ARCTAN: 4;
  COS: 5;
  EXP: 6;
  INF: 7;
  INT: 8;
  LN: 9;
  LOG10: 10;
  MAX: 11;
  MIN: 12;
  PI: 13;
  PULSE: 14;
  RAMP: 15;
  SAFEDIV: 16;
  SIN: 17;
  SQRT: 18;
  STEP: 19;
  TAN: 20;
  SIGN: 21;
}

export const BuiltinId: BuiltinIdMap;
