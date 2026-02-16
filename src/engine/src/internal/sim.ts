// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Simulation functions

import { getExports } from '@simlin/engine/internal/wasm';
import { getMemory } from '@simlin/engine/internal/wasm';
import {
  free,
  stringToWasm,
  wasmToStringAndFree,
  allocOutPtr,
  readOutPtr,
  allocOutUsize,
  readOutUsize,
  readDouble,
  readFloat64Array,
  malloc,
} from './memory';
import { SimlinModelPtr, SimlinSimPtr } from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error';

/**
 * Create a new simulation context.
 * @param model Model pointer
 * @param enableLtm Enable Loop Tendency Method analysis
 * @returns Simulation pointer
 */
export function simlin_sim_new(model: SimlinModelPtr, enableLtm: boolean): SimlinSimPtr {
  const exports = getExports();
  const fn = exports.simlin_sim_new as (model: number, ltm: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(model, enableLtm ? 1 : 0, outErrPtr);
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
 * Increment the reference count of a simulation.
 * @param sim Simulation pointer
 */
export function simlin_sim_ref(sim: SimlinSimPtr): void {
  const exports = getExports();
  const fn = exports.simlin_sim_ref as (ptr: number) => void;
  fn(sim);
}

/**
 * Decrement the reference count of a simulation. Frees if count reaches zero.
 * @param sim Simulation pointer
 */
export function simlin_sim_unref(sim: SimlinSimPtr): void {
  const exports = getExports();
  const fn = exports.simlin_sim_unref as (ptr: number) => void;
  fn(sim);
}

/**
 * Run simulation to a specific time.
 * @param sim Simulation pointer
 * @param time Target time
 */
export function simlin_sim_run_to(sim: SimlinSimPtr, time: number): void {
  const exports = getExports();
  const fn = exports.simlin_sim_run_to as (sim: number, time: number, outErr: number) => void;

  const outErrPtr = allocOutPtr();

  try {
    fn(sim, time, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }
  } finally {
    free(outErrPtr);
  }
}

/**
 * Run simulation to the end.
 * @param sim Simulation pointer
 */
export function simlin_sim_run_to_end(sim: SimlinSimPtr): void {
  const exports = getExports();
  const fn = exports.simlin_sim_run_to_end as (sim: number, outErr: number) => void;

  const outErrPtr = allocOutPtr();

  try {
    fn(sim, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }
  } finally {
    free(outErrPtr);
  }
}

/**
 * Reset simulation to initial state.
 * @param sim Simulation pointer
 */
export function simlin_sim_reset(sim: SimlinSimPtr): void {
  const exports = getExports();
  const fn = exports.simlin_sim_reset as (sim: number, outErr: number) => void;

  const outErrPtr = allocOutPtr();

  try {
    fn(sim, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }
  } finally {
    free(outErrPtr);
  }
}

/**
 * Get the step count from simulation.
 * @param sim Simulation pointer
 * @returns Number of steps
 */
export function simlin_sim_get_stepcount(sim: SimlinSimPtr): number {
  const exports = getExports();
  const fn = exports.simlin_sim_get_stepcount as (sim: number, outCount: number, outErr: number) => void;

  const outCountPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, outCountPtr, outErrPtr);
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
    free(outCountPtr);
    free(outErrPtr);
  }
}

/**
 * Get the current value of a variable.
 * @param sim Simulation pointer
 * @param name Variable name
 * @returns Current value
 */
export function simlin_sim_get_value(sim: SimlinSimPtr, name: string): number {
  const exports = getExports();
  const fn = exports.simlin_sim_get_value as (sim: number, name: number, outVal: number, outErr: number) => void;

  const namePtr = stringToWasm(name);
  const outValPtr = malloc(8); // f64
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, namePtr, outValPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    return readDouble(outValPtr);
  } finally {
    free(namePtr);
    free(outValPtr);
    free(outErrPtr);
  }
}

/**
 * Set the value of a variable.
 * @param sim Simulation pointer
 * @param name Variable name
 * @param value New value
 */
export function simlin_sim_set_value(sim: SimlinSimPtr, name: string, value: number): void {
  const exports = getExports();
  const fn = exports.simlin_sim_set_value as (sim: number, name: number, val: number, outErr: number) => void;

  const namePtr = stringToWasm(name);
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, namePtr, value, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }
  } finally {
    free(namePtr);
    free(outErrPtr);
  }
}

/**
 * Get time series data for a variable.
 * @param sim Simulation pointer
 * @param name Variable name
 * @param stepCount Number of steps to read
 * @returns Float64Array with time series data
 */
export function simlin_sim_get_series(sim: SimlinSimPtr, name: string, stepCount: number): Float64Array {
  const exports = getExports();
  const fn = exports.simlin_sim_get_series as (
    sim: number,
    name: number,
    results: number,
    len: number,
    outWritten: number,
    outErr: number,
  ) => void;

  const namePtr = stringToWasm(name);
  const resultsPtr = malloc(stepCount * 8); // f64 array
  const outWrittenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, namePtr, resultsPtr, stepCount, outWrittenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const written = readOutUsize(outWrittenPtr);
    // Use readFloat64Array to avoid alignment issues with Float64Array
    return readFloat64Array(resultsPtr, written);
  } finally {
    free(namePtr);
    free(resultsPtr);
    free(outWrittenPtr);
    free(outErrPtr);
  }
}

/**
 * Set a value by offset.
 * @param sim Simulation pointer
 * @param offset Variable offset
 * @param value New value
 */
export function simlin_sim_set_value_by_offset(sim: SimlinSimPtr, offset: number, value: number): void {
  const exports = getExports();
  const fn = exports.simlin_sim_set_value_by_offset as (
    sim: number,
    offset: number,
    val: number,
    outErr: number,
  ) => void;

  const outErrPtr = allocOutPtr();

  try {
    fn(sim, offset, value, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }
  } finally {
    free(outErrPtr);
  }
}

/**
 * Get the column offset for a variable.
 * @param sim Simulation pointer
 * @param name Variable name
 * @returns Column offset
 */
export function simlin_sim_get_offset(sim: SimlinSimPtr, name: string): number {
  const exports = getExports();
  const fn = exports.simlin_sim_get_offset as (sim: number, name: number, outOffset: number, outErr: number) => void;

  const namePtr = stringToWasm(name);
  const outOffsetPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, namePtr, outOffsetPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    return readOutUsize(outOffsetPtr);
  } finally {
    free(namePtr);
    free(outOffsetPtr);
    free(outErrPtr);
  }
}

/**
 * Get the number of simulation-level variable names.
 * @param sim Simulation pointer
 * @returns Number of variables
 */
export function simlin_sim_get_var_count(sim: SimlinSimPtr): number {
  const exports = getExports();
  const fn = exports.simlin_sim_get_var_count as (sim: number, outCount: number, outErr: number) => void;

  const outCountPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, outCountPtr, outErrPtr);
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
    free(outCountPtr);
    free(outErrPtr);
  }
}

/**
 * Get simulation-level variable names.
 * @param sim Simulation pointer
 * @returns Array of variable names
 */
export function simlin_sim_get_var_names(sim: SimlinSimPtr): string[] {
  const exports = getExports();
  const fn = exports.simlin_sim_get_var_names as (
    sim: number,
    result: number,
    max: number,
    outWritten: number,
    outErr: number,
  ) => void;

  const count = simlin_sim_get_var_count(sim);
  if (count === 0) {
    return [];
  }

  const resultPtr = malloc(count * 4);
  const outWrittenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, resultPtr, count, outWrittenPtr, outErrPtr);
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
    free(resultPtr);
    free(outWrittenPtr);
    free(outErrPtr);
  }
}
