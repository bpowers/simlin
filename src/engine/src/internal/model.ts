// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Model functions

import { getExports, getMemory } from '@simlin/engine/internal/wasm';
import {
  malloc,
  free,
  stringToWasm,
  wasmToStringAndFree,
  copyFromWasm,
  allocOutPtr,
  readOutPtr,
  allocOutUsize,
  readOutUsize,
} from './memory';
import { SimlinModelPtr, SimlinLinksPtr } from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error';

/**
 * Increment the reference count of a model.
 * @param model Model pointer
 */
export function simlin_model_ref(model: SimlinModelPtr): void {
  const exports = getExports();
  const fn = exports.simlin_model_ref as (ptr: number) => void;
  fn(model);
}

/**
 * Decrement the reference count of a model. Frees if count reaches zero.
 * @param model Model pointer
 */
export function simlin_model_unref(model: SimlinModelPtr): void {
  const exports = getExports();
  const fn = exports.simlin_model_unref as (ptr: number) => void;
  fn(model);
}

/**
 * Get the resolved display name of this model.
 * @param model Model pointer
 * @returns The model's display name
 */
export function simlin_model_get_name(model: SimlinModelPtr): string {
  const exports = getExports();
  const fn = exports.simlin_model_get_name as (model: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(model, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const name = wasmToStringAndFree(result);
    if (name === null) {
      throw new SimlinError('model name returned null pointer', 0);
    }
    return name;
  } finally {
    free(outErrPtr);
  }
}

/**
 * Get the number of variables in a model.
 * @param model Model pointer
 * @param typeMask Bitmask of variable types (0 = all)
 * @param filter Substring filter (null = no filter)
 * @returns Number of variables
 */
export function simlin_model_get_var_count(model: SimlinModelPtr, typeMask: number = 0, filter: string | null = null): number {
  const exports = getExports();
  const fn = exports.simlin_model_get_var_count as (model: number, typeMask: number, filter: number, outCount: number, outErr: number) => void;

  const outCountPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();
  const filterPtr = filter !== null ? stringToWasm(filter) : 0;

  try {
    fn(model, typeMask, filterPtr, outCountPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    return readOutUsize(outCountPtr);
  } finally {
    if (filterPtr !== 0) free(filterPtr);
    free(outCountPtr);
    free(outErrPtr);
  }
}

/**
 * Get the LaTeX representation of a variable's equation.
 * @param model Model pointer
 * @param ident Variable identifier
 * @returns LaTeX string, or null if not found
 */
export function simlin_model_get_latex_equation(model: SimlinModelPtr, ident: string): string | null {
  const exports = getExports();
  const fn = exports.simlin_model_get_latex_equation as (model: number, ident: number, outErr: number) => number;

  const identPtr = stringToWasm(ident);
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(model, identPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    if (result === 0) {
      return null;
    }

    return wasmToStringAndFree(result);
  } finally {
    free(identPtr);
    free(outErrPtr);
  }
}

/**
 * Get links from a model.
 * @param model Model pointer
 * @returns Links pointer
 */
export function simlin_model_get_links(model: SimlinModelPtr): SimlinLinksPtr {
  const exports = getExports();
  const fn = exports.simlin_model_get_links as (model: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(model, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    return result;
  } finally {
    free(outErrPtr);
  }
}

/**
 * Get variable names from a model.
 * @param model Model pointer
 * @param typeMask Bitmask of variable types (0 = all)
 * @param filter Substring filter (null = no filter)
 * @returns Array of variable names
 */
export function simlin_model_get_var_names(model: SimlinModelPtr, typeMask: number = 0, filter: string | null = null): string[] {
  const exports = getExports();
  const fn = exports.simlin_model_get_var_names as (
    model: number,
    typeMask: number,
    filter: number,
    result: number,
    max: number,
    outWritten: number,
    outErr: number,
  ) => void;

  // First get the count
  const count = simlin_model_get_var_count(model, typeMask, filter);
  if (count === 0) {
    return [];
  }

  const filterPtr = filter !== null ? stringToWasm(filter) : 0;

  // Allocate array of pointers (4 bytes each on wasm32)
  const resultPtr = malloc(count * 4);
  const outWrittenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(model, typeMask, filterPtr, resultPtr, count, outWrittenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const written = readOutUsize(outWrittenPtr);
    const names: string[] = [];
    const memory = getMemory();
    const view = new DataView(memory.buffer);

    for (let i = 0; i < written; i++) {
      const strPtr = view.getUint32(resultPtr + i * 4, true);
      if (strPtr !== 0) {
        const name = wasmToStringAndFree(strPtr);
        if (name !== null) {
          names.push(name);
        }
      }
    }

    return names;
  } finally {
    if (filterPtr !== 0) free(filterPtr);
    free(resultPtr);
    free(outWrittenPtr);
    free(outErrPtr);
  }
}

/**
 * Get the incoming links (dependencies) for a variable.
 * @param model Model pointer
 * @param varName Variable name
 * @returns Array of incoming variable names
 */
export function simlin_model_get_incoming_links(model: SimlinModelPtr, varName: string): string[] {
  const exports = getExports();
  const fn = exports.simlin_model_get_incoming_links as (
    model: number,
    varName: number,
    result: number,
    max: number,
    outWritten: number,
    outErr: number,
  ) => void;

  const varNamePtr = stringToWasm(varName);

  // First call with max=0 to get the count
  const outCountPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(model, varNamePtr, 0, 0, outCountPtr, outErrPtr);
    let errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const count = readOutUsize(outCountPtr);
    if (count === 0) {
      return [];
    }

    // Now allocate and get the actual links
    const resultPtr = malloc(count * 4);
    const outWrittenPtr = allocOutUsize();

    try {
      fn(model, varNamePtr, resultPtr, count, outWrittenPtr, outErrPtr);
      errPtr = readOutPtr(outErrPtr);

      if (errPtr !== 0) {
        const code = simlin_error_get_code(errPtr);
        const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
        simlin_error_free(errPtr);
        throw new SimlinError(message, code);
      }

      const written = readOutUsize(outWrittenPtr);
      const names: string[] = [];
      const memory = getMemory();
      const view = new DataView(memory.buffer);

      for (let i = 0; i < written; i++) {
        const strPtr = view.getUint32(resultPtr + i * 4, true);
        if (strPtr !== 0) {
          const name = wasmToStringAndFree(strPtr);
          if (name !== null) {
            names.push(name);
          }
        }
      }

      return names;
    } finally {
      free(resultPtr);
      free(outWrittenPtr);
    }
  } finally {
    free(varNamePtr);
    free(outCountPtr);
    free(outErrPtr);
  }
}

/**
 * Call a WASM FFI function that writes its result to an out_buffer/out_len pair.
 * Handles error checking, buffer copying, and memory cleanup.
 */
function callBufferReturningFn(
  invoke: (outBuf: number, outLen: number, outErr: number) => void,
): Uint8Array {
  const outBufPtr = allocOutPtr();
  const outLenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    invoke(outBufPtr, outLenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const bufPtr = readOutPtr(outBufPtr);
    const len = readOutUsize(outLenPtr);
    const data = copyFromWasm(bufPtr, len);
    free(bufPtr);
    return data;
  } finally {
    free(outBufPtr);
    free(outLenPtr);
    free(outErrPtr);
  }
}

/**
 * Get a single variable's data as JSON.
 */
export function simlin_model_get_var_json(model: SimlinModelPtr, varName: string): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_model_get_var_json as (
    model: number,
    varName: number,
    outBuf: number,
    outLen: number,
    outErr: number,
  ) => void;

  const varNamePtr = stringToWasm(varName);
  try {
    return callBufferReturningFn((outBuf, outLen, outErr) => fn(model, varNamePtr, outBuf, outLen, outErr));
  } finally {
    free(varNamePtr);
  }
}

/**
 * Get simulation specs as JSON.
 */
export function simlin_model_get_sim_specs_json(model: SimlinModelPtr): Uint8Array {
  const fn = getExports().simlin_model_get_sim_specs_json as (
    model: number,
    outBuf: number,
    outLen: number,
    outErr: number,
  ) => void;

  return callBufferReturningFn((outBuf, outLen, outErr) => fn(model, outBuf, outLen, outErr));
}
