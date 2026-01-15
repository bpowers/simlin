// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Import and export functions

import { getExports } from './wasm';
import {
  free,
  copyToWasm,
  copyFromWasm,
  allocOutPtr,
  readOutPtr,
  allocOutUsize,
  readOutUsize,
} from './memory';
import { SimlinProjectPtr } from './types';
import { simlin_error_free, simlin_error_get_code, simlin_error_get_message, SimlinError } from './error';

/**
 * Import a project from XMILE format.
 * @param data XMILE XML data
 * @returns Project pointer
 */
export function simlin_import_xmile(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_import_xmile as (ptr: number, len: number, outErr: number) => number;

  const dataPtr = copyToWasm(data);
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(dataPtr, data.length, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }

    return result;
  } finally {
    free(dataPtr);
    free(outErrPtr);
  }
}

/**
 * Import a project from Vensim MDL format.
 * @param data MDL file data
 * @returns Project pointer
 */
export function simlin_import_mdl(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_import_mdl as (ptr: number, len: number, outErr: number) => number;

  const dataPtr = copyToWasm(data);
  const outErrPtr = allocOutPtr();

  try {
    const result = fn(dataPtr, data.length, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      simlin_error_free(errPtr);
      throw new SimlinError(message, code);
    }

    return result;
  } finally {
    free(dataPtr);
    free(outErrPtr);
  }
}

/**
 * Export a project to XMILE format.
 * @param project Project pointer
 * @returns XMILE XML data
 */
export function simlin_export_xmile(project: SimlinProjectPtr): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_export_xmile as (
    proj: number,
    outBuf: number,
    outLen: number,
    outErr: number
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
