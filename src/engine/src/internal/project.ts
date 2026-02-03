// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Project management functions

import { getExports, getMemory } from '@simlin/engine/internal/wasm';
import {
  malloc,
  free,
  stringToWasm,
  wasmToStringAndFree,
  copyToWasm,
  copyFromWasm,
  allocOutPtr,
  readOutPtr,
  allocOutUsize,
  readOutUsize,
} from './memory';
import { SimlinProjectPtr, SimlinModelPtr, SimlinErrorPtr, SimlinJsonFormat } from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  SimlinError,
  readAllErrorDetails,
} from './error';

/**
 * Open a project from protobuf data.
 * @param data Protobuf-encoded project data
 * @returns Project pointer
 * @throws SimlinError on failure
 */
export function simlin_project_open_protobuf(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_project_open_protobuf as (ptr: number, len: number, outErr: number) => number;

  const dataPtr = copyToWasm(data);
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(dataPtr, data.length, outErrPtr);
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
    free(dataPtr);
    free(outErrPtr);
  }
}

/**
 * Open a project from JSON data.
 * @param data JSON-encoded project data
 * @param format JSON format (Native or SDAI)
 * @returns Project pointer
 * @throws SimlinError on failure
 */
export function simlin_project_open_json(data: Uint8Array, format: SimlinJsonFormat): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_project_open_json as (ptr: number, len: number, fmt: number, outErr: number) => number;

  const dataPtr = copyToWasm(data);
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(dataPtr, data.length, format, outErrPtr);
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
    free(dataPtr);
    free(outErrPtr);
  }
}

/**
 * Increment the reference count of a project.
 * @param project Project pointer
 */
export function simlin_project_ref(project: SimlinProjectPtr): void {
  const exports = getExports();
  const fn = exports.simlin_project_ref as (ptr: number) => void;
  fn(project);
}

/**
 * Decrement the reference count of a project. Frees if count reaches zero.
 * @param project Project pointer
 */
export function simlin_project_unref(project: SimlinProjectPtr): void {
  const exports = getExports();
  const fn = exports.simlin_project_unref as (ptr: number) => void;
  fn(project);
}

/**
 * Get the number of models in a project.
 * @param project Project pointer
 * @returns Number of models
 */
export function simlin_project_get_model_count(project: SimlinProjectPtr): number {
  const exports = getExports();
  const fn = exports.simlin_project_get_model_count as (proj: number, outCount: number, outErr: number) => void;

  const outCountPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(project, outCountPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }

    return readOutUsize(outCountPtr);
  } finally {
    free(outCountPtr);
    free(outErrPtr);
  }
}

/**
 * Get model names from a project.
 * @param project Project pointer
 * @returns Array of model names
 */
export function simlin_project_get_model_names(project: SimlinProjectPtr): string[] {
  const exports = getExports();
  const fn = exports.simlin_project_get_model_names as (
    proj: number,
    result: number,
    max: number,
    outWritten: number,
    outErr: number,
  ) => void;

  // First get the count
  const count = simlin_project_get_model_count(project);
  if (count === 0) {
    return [];
  }

  // Allocate array of pointers (4 bytes each on wasm32)
  const resultPtr = malloc(count * 4);
  const outWrittenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(project, resultPtr, count, outWrittenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

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
    free(outErrPtr);
  }
}

/**
 * Get a model from a project.
 * @param project Project pointer
 * @param modelName Model name (null for default/main model)
 * @returns Model pointer
 */
export function simlin_project_get_model(project: SimlinProjectPtr, modelName: string | null): SimlinModelPtr {
  const exports = getExports();
  const fn = exports.simlin_project_get_model as (proj: number, name: number, outErr: number) => number;

  const namePtr = modelName !== null ? stringToWasm(modelName) : 0;
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(project, namePtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }

    return result;
  } finally {
    if (namePtr !== 0) free(namePtr);
    free(outErrPtr);
  }
}

/**
 * Serialize a project to protobuf.
 * @param project Project pointer
 * @returns Protobuf-encoded project data
 */
export function simlin_project_serialize_protobuf(project: SimlinProjectPtr): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_project_serialize_protobuf as (
    proj: number,
    outBuf: number,
    outLen: number,
    outErr: number,
  ) => void;

  const outBufPtr = allocOutPtr();
  const outLenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(project, outBufPtr, outLenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
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
 * Serialize a project to JSON.
 * @param project Project pointer
 * @param format JSON format
 * @returns JSON-encoded project data
 */
export function simlin_project_serialize_json(project: SimlinProjectPtr, format: SimlinJsonFormat): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_project_serialize_json as (
    proj: number,
    fmt: number,
    outBuf: number,
    outLen: number,
    outErr: number,
  ) => void;

  const outBufPtr = allocOutPtr();
  const outLenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(project, format, outBufPtr, outLenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
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
 * Check if a project is simulatable.
 * @param project Project pointer
 * @param modelName Model name (null for default/main model)
 * @returns True if simulatable
 */
export function simlin_project_is_simulatable(project: SimlinProjectPtr, modelName: string | null): boolean {
  const exports = getExports();
  const fn = exports.simlin_project_is_simulatable as (proj: number, name: number, outErr: number) => number;

  const namePtr = modelName !== null ? stringToWasm(modelName) : 0;
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(project, namePtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      simlin_error_free(errPtr);
      return false;
    }

    return result !== 0;
  } finally {
    if (namePtr !== 0) free(namePtr);
    free(outErrPtr);
  }
}

/**
 * Get all errors in a project.
 * @param project Project pointer
 * @returns Error pointer (0 if no errors)
 * @throws SimlinError if the call itself fails (e.g., invalid project pointer)
 */
export function simlin_project_get_errors(project: SimlinProjectPtr): SimlinErrorPtr {
  const exports = getExports();
  const fn = exports.simlin_project_get_errors as (proj: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(project, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }

    return result;
  } finally {
    free(outErrPtr);
  }
}

/**
 * Add a new model to a project.
 * @param project Project pointer
 * @param modelName Model name
 */
export function simlin_project_add_model(project: SimlinProjectPtr, modelName: string): void {
  const exports = getExports();
  const fn = exports.simlin_project_add_model as (proj: number, name: number, outErr: number) => void;

  const namePtr = stringToWasm(modelName);
  const outErrPtr = allocOutPtr();

  try {
    fn(project, namePtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }
  } finally {
    free(namePtr);
    free(outErrPtr);
  }
}

/**
 * Apply a JSON patch to the project datamodel.
 * @param project Project pointer
 * @param patchData JSON-encoded patch data
 * @param dryRun If true, validate without applying
 * @param allowErrors If true, continue despite errors
 * @returns Collected errors if any (caller should free with simlin_error_free)
 */
export function simlin_project_apply_patch(
  project: SimlinProjectPtr,
  patchData: Uint8Array,
  dryRun: boolean,
  allowErrors: boolean,
): SimlinErrorPtr {
  const exports = getExports();
  const fn = exports.simlin_project_apply_patch as (
    proj: number,
    data: number,
    len: number,
    dryRun: number,
    allowErrors: number,
    outCollected: number,
    outErr: number,
  ) => void;

  const dataPtr = copyToWasm(patchData);
  const outCollectedPtr = allocOutPtr();
  const outErrPtr = allocOutPtr();

  try {
    fn(project, dataPtr, patchData.length, dryRun ? 1 : 0, allowErrors ? 1 : 0, outCollectedPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);

      // Also free any collected errors to prevent memory leak
      const collectedPtr = readOutPtr(outCollectedPtr);
      if (collectedPtr !== 0) {
        // Read collected error details and merge with main error details
        const collectedDetails = readAllErrorDetails(collectedPtr);
        details.push(...collectedDetails);
        simlin_error_free(collectedPtr);
      }

      throw new SimlinError(message, code, details);
    }

    return readOutPtr(outCollectedPtr);
  } finally {
    free(dataPtr);
    free(outCollectedPtr);
    free(outErrPtr);
  }
}
