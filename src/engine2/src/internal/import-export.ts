// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Import and export functions

import { getExports } from '@system-dynamics/engine2/internal/wasm';
import { free, copyToWasm, copyFromWasm, allocOutPtr, readOutPtr, allocOutUsize, readOutUsize } from './memory';
import { SimlinProjectPtr } from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error';

/**
 * Open a project from XMILE format.
 * @param data XMILE XML data
 * @returns Project pointer
 */
export function simlin_project_open_xmile(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_project_open_xmile as (ptr: number, len: number, outErr: number) => number;

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
 * Check if the WASM module was built with Vensim MDL support.
 * @returns True if simlin_project_open_vensim is available
 */
export function hasVensimSupport(): boolean {
  const exports = getExports();
  return typeof exports.simlin_project_open_vensim === 'function';
}

/**
 * Open a project from Vensim MDL format.
 * @param data MDL file data
 * @returns Project pointer
 */
export function simlin_project_open_vensim(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const importFn = exports.simlin_project_open_vensim as (ptr: number, len: number, outErr: number) => number;
  const dataPtr = copyToWasm(data);
  const outErrPtr = allocOutPtr();

  try {
    const result = importFn(dataPtr, data.length, outErrPtr);
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
 * Serialize a project to XMILE format.
 * @param project Project pointer
 * @returns XMILE XML data
 */
export function simlin_project_serialize_xmile(project: SimlinProjectPtr): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_project_serialize_xmile as (
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
