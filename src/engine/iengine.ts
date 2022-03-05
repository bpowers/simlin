// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export interface Engine {
  free(): void;
  /**
   * @returns {Uint8Array}
   */
  serializeToProtobuf(): Uint8Array;
  /**
   * @param {Function} callback
   * @returns {number}
   */
  onChange(callback: () => undefined): number;
  /**
   * @param {number} callback_id
   */
  removeOnChangeCallback(callback_id: number): void;
  /**
   * @param {number} value
   * @returns {Error | undefined}
   */
  setSimSpecStart(value: number): Error | undefined;
  /**
   * @param {number} value
   * @returns {Error | undefined}
   */
  setSimSpecStop(value: number): Error | undefined;
  /**
   * @param {number} value
   * @param {boolean} is_reciprocal
   * @returns {Error | undefined}
   */
  setSimSpecDt(value: number, is_reciprocal: boolean): Error | undefined;
  /**
   * @param {number} value
   * @param {boolean} is_reciprocal
   * @returns {Error | undefined}
   */
  setSimSpecSavestep(value: number, is_reciprocal: boolean): Error | undefined;
  /**
   */
  clearSimSpecSavestep(): void;
  /**
   * @param {number} method
   * @returns {Error | undefined}
   */
  setSimSpecMethod(method: number): Error | undefined;
  /**
   * @param {string} value
   * @returns {Error | undefined}
   */
  setSimSpecTimeUnits(value: string): Error | undefined;
  /**
   * @returns {boolean}
   */
  isSimulatable(): boolean;
  /**
   * @param {string} model_name
   * @returns {any}
   */
  getModelVariableErrors(model_name: string): any;
  getModelVariableUnitErrors(model_name: string): any;
  /**
   * @param {string} model_name
   * @returns {Array<any>}
   */
  getModelErrors(model_name: string): Array<any>;
  /**
   * @param {string} model_name
   * @param {string} kind
   * @param {string} name
   * @returns {Error | undefined}
   */
  addNewVariable(model_name: string, kind: string, name: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} ident
   * @returns {Error | undefined}
   */
  deleteVariable(model_name: string, ident: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} stock
   * @param {string} flow
   * @param {string} dir
   * @returns {Error | undefined}
   */
  addStocksFlow(model_name: string, stock: string, flow: string, dir: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} stock
   * @param {string} flow
   * @param {string} dir
   * @returns {Error | undefined}
   */
  removeStocksFlow(model_name: string, stock: string, flow: string, dir: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} ident
   * @param {string} new_equation
   * @returns {Error | undefined}
   */
  setEquation(model_name: string, ident: string, new_equation: string): Error | undefined;
  setUnits(model_name: string, ident: string, new_units: string): Error | undefined;
  setDocumentation(model_name: string, ident: string, new_docs: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} ident
   * @param {Uint8Array} graphical_function_pb
   * @returns {Error | undefined}
   */
  setGraphicalFunction(model_name: string, ident: string, graphical_function_pb: Uint8Array): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} ident
   * @returns {Error | undefined}
   */
  removeGraphicalFunction(model_name: string, ident: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} old_name
   * @param {string} new_name
   * @returns {Error | undefined}
   */
  rename(model_name: string, old_name: string, new_name: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {number} view_off
   * @param {Uint8Array} view_pb
   * @returns {Error | undefined}
   */
  setView(model_name: string, view_off: number, view_pb: Uint8Array): Error | undefined;
  /**
   */
  simRunToEnd(): void;
  /**
   * @returns {Array<any>}
   */
  simVarNames(): Array<any>;
  /**
   * @param {string} ident
   * @returns {Float64Array}
   */
  simSeries(ident: string): Float64Array;
  /**
   */
  simClose(): void;
}
/**
 */
export interface EquationError {
  free(): void;
  /**
   * @returns {number}
   */
  code: number;
  /**
   * @returns {number}
   */
  start: number;
  /**
   * @returns {number}
   */
  end: number;
}
/**
 */
export interface Error {
  free(): void;
  /**
   * @param {number} kind
   * @param {number} code
   * @param {string | undefined} details
   * @returns {Error}
   */
  /**
   * @returns {string | undefined}
   */
  getDetails(): string | undefined;
  /**
   * @returns {number}
   */
  code: number;
  /**
   * @returns {number}
   */
  kind: number;
}

export interface UnitError {
  free(): void;
  /**
   * @returns {string | undefined}
   */
  get_details(): string | undefined;
  /**
   * @returns {number}
   */
  code: number;
  /**
   * @returns {number}
   */
  end: number;
  /**
   * @returns {boolean}
   */
  is_consistency_error: boolean;
  /**
   * @returns {number}
   */
  start: number;
}

export enum ErrorKind {
  Import,
  Model,
  Simulation,
  Variable,
}

export enum ErrorCode {
  NoError, // will never be produced
  DoesNotExist, // the named entity doesn't exist
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
  UnitDefinitionErrors,
  Generic,
  NoAppInUnits,
  NoSubscriptInUnits,
  NoIfInUnits,
  NoUnaryOpInUnits,
  BadBinaryOpInUnits,
  NoConstInUnits,
  ExpectedInteger,
  ExpectedIntegerOne,
  DuplicateUnit,
  ExpectedModule,
  ExpectedIdent,
  UnitMismatch,
}
