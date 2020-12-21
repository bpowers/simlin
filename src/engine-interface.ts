// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// copied from wasm-bindgen output, turned into an interface to avoid
// sad `WebAssembly module is included in initial chunk.
// This is not allowed, because WebAssembly download and compilation must happen asynchronous.
// Add an async splitpoint (i. e. import()) somewhere between your entrypoint and the WebAssembly module:`
// error.
export interface Engine {
  free(): void;
  /**
   * @returns {Uint8Array}
   */
  serializeToProtobuf(): Uint8Array;
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
   * @param {string} _model_name
   * @param {string} _kind
   * @param {string} _name
   * @returns {Error | undefined}
   */
  addNewVariable(_model_name: string, _kind: string, _name: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _ident
   * @returns {Error | undefined}
   */
  deleteVariable(_model_name: string, _ident: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _stock
   * @param {string} _flow
   * @param {string} _dir
   * @returns {Error | undefined}
   */
  addStocksFlow(_model_name: string, _stock: string, _flow: string, _dir: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _stock
   * @param {string} _flow
   * @param {string} _dir
   * @returns {Error | undefined}
   */
  removeStocksFlow(_model_name: string, _stock: string, _flow: string, _dir: string): Error | undefined;
  /**
   * @param {string} model_name
   * @param {string} ident
   * @param {string} new_equation
   * @returns {Error | undefined}
   */
  setEquation(model_name: string, ident: string, new_equation: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _ident
   * @param {Uint8Array} _gf
   * @returns {Error | undefined}
   */
  setGraphicalFunction(_model_name: string, _ident: string, _gf: Uint8Array): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _ident
   * @returns {Error | undefined}
   */
  removeGraphicalFunction(_model_name: string, _ident: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _old_ident
   * @param {string} _new_ident
   * @returns {Error | undefined}
   */
  rename(_model_name: string, _old_ident: string, _new_ident: string): Error | undefined;
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

export interface Error {
  free(): void;
  /**
   * @returns {string | undefined}
   */
  getDetails(): string | undefined;
  /**
   * @returns {number}
   */
  code: ErrorCode;
  /**
   * @returns {number}
   */
  kind: ErrorKind;
}

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
}

export function errorCodeDescription(code: ErrorCode): string {
  switch (code) {
    case ErrorCode.NoError:
      return 'Internal error';
    case ErrorCode.DoesNotExist:
      return 'Does not exist';
    case ErrorCode.XmlDeserialization:
      return 'XML deserialization error';
    case ErrorCode.VensimConversion:
      return 'Vensim conversion error';
    case ErrorCode.ProtobufDecode:
      return 'Internal error (protocol buffer decoding)';
    case ErrorCode.InvalidToken:
      return 'Invalid input in equation';
    case ErrorCode.UnrecognizedEOF:
      return 'Unexpectedly reached the end of the equation (mismatched parens?)';
    case ErrorCode.UnrecognizedToken:
      return 'Unrecognized input in equation';
    case ErrorCode.ExtraToken:
      return 'Extra input after equation fully parsed';
    case ErrorCode.UnclosedComment:
      return 'Unclosed comment';
    case ErrorCode.UnclosedQuotedIdent:
      return 'Unclosed quoted identifier';
    case ErrorCode.ExpectedNumber:
      return 'Expected a literal number';
    case ErrorCode.UnknownBuiltin:
      return 'Reference to unknown or unimplemented builtin';
    case ErrorCode.BadBuiltinArgs:
      return 'Builtin function arguments';
    case ErrorCode.EmptyEquation:
      return 'Variable has empty equation';
    case ErrorCode.BadModuleInputDst:
      return 'Module input destination is unknown';
    case ErrorCode.BadModuleInputSrc:
      return 'Module input source is unknown';
    case ErrorCode.NotSimulatable:
      return 'Model has errors and is not simulatable';
    case ErrorCode.BadTable:
      return 'No graphical function for specified variable';
    case ErrorCode.BadSimSpecs:
      return 'Simulation Specs are not valid';
    case ErrorCode.NoAbsoluteReferences:
      return 'Absolute references are not supported';
    case ErrorCode.CircularDependency:
      return 'Circular dependency';
    case ErrorCode.ArraysNotImplemented:
      return 'Arrays not implemented';
    case ErrorCode.MultiDimensionalArraysNotImplemented:
      return 'Multi-dimensional arrays not implemented';
    case ErrorCode.BadDimensionName:
      return 'Unknown dimension name';
    case ErrorCode.BadModelName:
      return 'Unknown model name';
    case ErrorCode.MismatchedDimensions:
      return 'Mismatched dimensions';
    case ErrorCode.ArrayReferenceNeedsExplicitSubscripts:
      return 'Array reference needs explicit subscripts';
    case ErrorCode.DuplicateVariable:
      return 'Duplicate variable';
    case ErrorCode.UnknownDependency:
      return 'Equation refers to unknown variable';
  }
}

export enum ErrorKind {
  Import,
  Model,
  Simulation,
  Variable,
}
