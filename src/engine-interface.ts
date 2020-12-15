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
   * @param {string} _flow
   * @param {string} _dir
   * @returns {Error | undefined}
   */
  addStocksFlow(_model_name: string, _flow: string, _dir: string): Error | undefined;
  /**
   * @param {string} _model_name
   * @param {string} _flow
   * @param {string} _dir
   * @returns {Error | undefined}
   */
  removeStocksFlow(_model_name: string, _flow: string, _dir: string): Error | undefined;
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
   */
  simRunToEnd(): void;
  /**
   * @returns {Array<any>}
   */
  simVarNames(): Array<any>;
  /**
   * @param {string} _ident
   * @returns {Float64Array}
   */
  simSeries(_ident: string): Float64Array;
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
  code: number;
  /**
   * @returns {number}
   */
  kind: number;
}
