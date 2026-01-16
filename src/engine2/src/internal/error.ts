// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Error handling functions

import { getExports, getMemory } from '@system-dynamics/engine2/internal/wasm';
import { wasmToString } from './memory.js';
import { Ptr, SimlinErrorPtr, SimlinErrorCode, SimlinErrorKind, SimlinUnitErrorKind, ErrorDetail } from './types.js';

/**
 * Get the string representation of an error code.
 * @param code Error code
 * @returns Human-readable error string
 */
export function simlin_error_str(code: SimlinErrorCode): string {
  const exports = getExports();
  const fn = exports.simlin_error_str as (code: number) => number;
  const ptr = fn(code);
  // This string is static and should NOT be freed
  return wasmToString(ptr) ?? `Unknown error ${code}`;
}

/**
 * Free an error object.
 * @param err Error pointer
 */
export function simlin_error_free(err: SimlinErrorPtr): void {
  if (err === 0) return;
  const exports = getExports();
  const fn = exports.simlin_error_free as (ptr: number) => void;
  fn(err);
}

/**
 * Get the error code from an error object.
 * @param err Error pointer
 * @returns Error code
 */
export function simlin_error_get_code(err: SimlinErrorPtr): SimlinErrorCode {
  const exports = getExports();
  const fn = exports.simlin_error_get_code as (ptr: number) => number;
  return fn(err);
}

/**
 * Get the message from an error object.
 * @param err Error pointer
 * @returns Error message or null
 */
export function simlin_error_get_message(err: SimlinErrorPtr): string | null {
  const exports = getExports();
  const fn = exports.simlin_error_get_message as (ptr: number) => number;
  const ptr = fn(err);
  // This string is owned by the error and should NOT be freed
  return wasmToString(ptr);
}

/**
 * Get the number of error details.
 * @param err Error pointer
 * @returns Number of error details
 */
export function simlin_error_get_detail_count(err: SimlinErrorPtr): number {
  const exports = getExports();
  const fn = exports.simlin_error_get_detail_count as (ptr: number) => number;
  return fn(err);
}

/**
 * Get the error details array pointer.
 * @param err Error pointer
 * @returns Pointer to error details array
 */
export function simlin_error_get_details(err: SimlinErrorPtr): Ptr {
  const exports = getExports();
  const fn = exports.simlin_error_get_details as (ptr: number) => number;
  return fn(err);
}

/**
 * Get a specific error detail by index.
 * @param err Error pointer
 * @param index Index of the detail
 * @returns Pointer to error detail
 */
export function simlin_error_get_detail(err: SimlinErrorPtr, index: number): Ptr {
  const exports = getExports();
  const fn = exports.simlin_error_get_detail as (ptr: number, idx: number) => number;
  return fn(err, index);
}

// Size of SimlinErrorDetail struct in bytes (for wasm32)
// code: u32, message: ptr, model_name: ptr, variable_name: ptr, start_offset: u16, end_offset: u16, kind: u32, unit_error_kind: u32
// = 4 + 4 + 4 + 4 + 2 + 2 + 4 + 4 = 28 bytes (with padding it may be 32)
// const ERROR_DETAIL_SIZE = 32;

/**
 * Read an ErrorDetail struct from WASM memory.
 * @param ptr Pointer to SimlinErrorDetail
 * @returns ErrorDetail object
 */
export function readErrorDetail(ptr: Ptr): ErrorDetail {
  const memory = getMemory();
  const view = new DataView(memory.buffer);

  const code = view.getUint32(ptr, true) as SimlinErrorCode;
  const messagePtr = view.getUint32(ptr + 4, true);
  const modelNamePtr = view.getUint32(ptr + 8, true);
  const variableNamePtr = view.getUint32(ptr + 12, true);
  const startOffset = view.getUint16(ptr + 16, true);
  const endOffset = view.getUint16(ptr + 18, true);
  const kind = view.getUint32(ptr + 20, true) as SimlinErrorKind;
  const unitErrorKind = view.getUint32(ptr + 24, true) as SimlinUnitErrorKind;

  return {
    code,
    message: wasmToString(messagePtr),
    modelName: wasmToString(modelNamePtr),
    variableName: wasmToString(variableNamePtr),
    startOffset,
    endOffset,
    kind,
    unitErrorKind,
  };
}

/**
 * Read all error details from an error object.
 * @param err Error pointer
 * @returns Array of ErrorDetail objects
 */
export function readAllErrorDetails(err: SimlinErrorPtr): ErrorDetail[] {
  if (err === 0) return [];

  const count = simlin_error_get_detail_count(err);
  const details: ErrorDetail[] = [];

  for (let i = 0; i < count; i++) {
    const detailPtr = simlin_error_get_detail(err, i);
    if (detailPtr !== 0) {
      details.push(readErrorDetail(detailPtr));
    }
  }

  return details;
}

/**
 * Custom error class for libsimlin errors.
 */
export class SimlinError extends Error {
  constructor(
    message: string,
    public code: SimlinErrorCode,
    public details: ErrorDetail[] = [],
  ) {
    super(message);
    this.name = 'SimlinError';
  }
}
