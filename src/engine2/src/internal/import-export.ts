// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Import and export functions

import { getExports } from '@system-dynamics/engine2/internal/wasm';
import { free, copyToWasm, copyFromWasm, allocOutPtr, readOutPtr, allocOutUsize, readOutUsize } from './memory.js';
import { SimlinProjectPtr, SimlinErrorCode } from './types.js';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error.js';

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
 * @returns True if simlin_import_mdl is available
 */
export function hasVensimSupport(): boolean {
  const exports = getExports();
  return typeof exports.simlin_import_mdl === 'function';
}

/**
 * Import a project from Vensim MDL format.
 * Note: This function is only available when libsimlin is built with the 'vensim' feature.
 * The default WASM build (--no-default-features) does not include this function.
 * Use hasVensimSupport() to check availability before calling.
 * @param data MDL file data
 * @returns Project pointer
 * @throws SimlinError with code Generic if vensim support is not available
 */
export function simlin_import_mdl(data: Uint8Array): SimlinProjectPtr {
  const exports = getExports();
  const fn = exports.simlin_import_mdl;

  // Guard against missing export when vensim feature is disabled
  if (typeof fn !== 'function') {
    throw new SimlinError(
      'simlin_import_mdl is not available: libsimlin was built without Vensim support. ' +
        'Rebuild with the "vensim" feature enabled to import MDL files.',
      SimlinErrorCode.Generic,
    );
  }

  const importFn = fn as (ptr: number, len: number, outErr: number) => number;
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
 * Export a project to XMILE format.
 * @param project Project pointer
 * @returns XMILE XML data
 */
export function simlin_export_xmile(project: SimlinProjectPtr): Uint8Array {
  const exports = getExports();
  const fn = exports.simlin_export_xmile as (proj: number, outBuf: number, outLen: number, outErr: number) => void;

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
