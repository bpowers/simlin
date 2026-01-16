// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Memory management helpers for WASM interop

import { getExports, getMemory } from '@system-dynamics/engine2/internal/wasm';
import { Ptr } from './types.js';

// Text encoder/decoder for string conversion
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/**
 * Allocate memory in the WASM heap.
 * @param size Number of bytes to allocate
 * @returns Pointer to allocated memory
 */
export function malloc(size: number): Ptr {
  const exports = getExports();
  const fn = exports.simlin_malloc as (size: number) => number;
  return fn(size);
}

/**
 * Free memory allocated with malloc.
 * @param ptr Pointer to free
 */
export function free(ptr: Ptr): void {
  if (ptr === 0) return;
  const exports = getExports();
  const fn = exports.simlin_free as (ptr: number) => void;
  fn(ptr);
}

/**
 * Free a string allocated by libsimlin.
 * @param ptr Pointer to string
 */
export function freeString(ptr: Ptr): void {
  if (ptr === 0) return;
  const exports = getExports();
  const fn = exports.simlin_free_string as (ptr: number) => void;
  fn(ptr);
}

/**
 * Copy a JavaScript string to WASM memory as a null-terminated C string.
 * The caller is responsible for freeing the memory.
 * @param str String to copy
 * @returns Pointer to the string in WASM memory
 */
export function stringToWasm(str: string): Ptr {
  const bytes = textEncoder.encode(str + '\0');
  const ptr = malloc(bytes.length);
  const memory = getMemory();
  const view = new Uint8Array(memory.buffer, ptr, bytes.length);
  view.set(bytes);
  return ptr;
}

// Maximum string length to prevent runaway reads (1MB)
const MAX_STRING_LENGTH = 1024 * 1024;

/**
 * Read a null-terminated C string from WASM memory.
 * Does NOT free the string.
 * @param ptr Pointer to the string
 * @param maxLength Maximum length to read (defaults to 1MB)
 * @returns JavaScript string, or null if ptr is 0
 */
export function wasmToString(ptr: Ptr, maxLength: number = MAX_STRING_LENGTH): string | null {
  if (ptr === 0) return null;
  const memory = getMemory();
  const view = new Uint8Array(memory.buffer);
  const bufferEnd = view.length;

  // Find null terminator with bounds checking
  let end = ptr;
  const limit = Math.min(ptr + maxLength, bufferEnd);
  while (end < limit && view[end] !== 0) {
    end++;
  }

  // If we hit the limit without finding null terminator, this is likely a bug
  if (end >= limit && view[end] !== 0) {
    throw new Error(`wasmToString: string exceeds maximum length ${maxLength} or is not null-terminated`);
  }

  const bytes = view.slice(ptr, end);
  return textDecoder.decode(bytes);
}

/**
 * Read a null-terminated C string from WASM memory and free it.
 * @param ptr Pointer to the string
 * @returns JavaScript string, or null if ptr is 0
 */
export function wasmToStringAndFree(ptr: Ptr): string | null {
  const str = wasmToString(ptr);
  freeString(ptr);
  return str;
}

/**
 * Copy a Uint8Array to WASM memory.
 * The caller is responsible for freeing the memory.
 * @param data Data to copy
 * @returns Pointer to the data in WASM memory
 */
export function copyToWasm(data: Uint8Array): Ptr {
  const ptr = malloc(data.length);
  const memory = getMemory();
  const view = new Uint8Array(memory.buffer, ptr, data.length);
  view.set(data);
  return ptr;
}

/**
 * Copy data from WASM memory to a new Uint8Array.
 * @param ptr Pointer to the data
 * @param length Number of bytes to copy
 * @returns New Uint8Array with copied data
 */
export function copyFromWasm(ptr: Ptr, length: number): Uint8Array {
  const memory = getMemory();
  const view = new Uint8Array(memory.buffer, ptr, length);
  return new Uint8Array(view); // Copy to avoid memory view issues
}

/**
 * Allocate space for an output pointer (4 bytes on wasm32).
 * @returns Pointer to allocated space
 */
export function allocOutPtr(): Ptr {
  return malloc(4);
}

/**
 * Read a pointer value from an output pointer.
 * Uses DataView to avoid alignment requirements.
 * @param outPtr Pointer to the output pointer
 * @returns The pointer value
 */
export function readOutPtr(outPtr: Ptr): Ptr {
  const memory = getMemory();
  const view = new DataView(memory.buffer);
  return view.getUint32(outPtr, true);
}

/**
 * Allocate space for an output usize (4 bytes on wasm32).
 * @returns Pointer to allocated space
 */
export function allocOutUsize(): Ptr {
  return malloc(4);
}

/**
 * Read a usize value from an output pointer.
 * Uses DataView to avoid alignment requirements.
 * @param outPtr Pointer to the output usize
 * @returns The usize value
 */
export function readOutUsize(outPtr: Ptr): number {
  const memory = getMemory();
  const view = new DataView(memory.buffer);
  return view.getUint32(outPtr, true);
}

/**
 * Read a double value from WASM memory.
 * Uses DataView to avoid alignment requirements.
 * @param ptr Pointer to the double
 * @returns The double value
 */
export function readDouble(ptr: Ptr): number {
  const memory = getMemory();
  const view = new DataView(memory.buffer);
  return view.getFloat64(ptr, true);
}

/**
 * Read an array of float64 values from WASM memory.
 * Uses DataView to avoid alignment requirements and copies data.
 * @param ptr Pointer to the array
 * @param count Number of elements
 * @returns New Float64Array with copied data
 */
export function readFloat64Array(ptr: Ptr, count: number): Float64Array {
  const memory = getMemory();
  const view = new DataView(memory.buffer);
  const result = new Float64Array(count);
  for (let i = 0; i < count; i++) {
    result[i] = view.getFloat64(ptr + i * 8, true);
  }
  return result;
}

/**
 * Read a uint16 value from WASM memory.
 * @param ptr Pointer to the uint16
 * @returns The uint16 value
 */
export function readU16(ptr: Ptr): number {
  const memory = getMemory();
  const view = new Uint16Array(memory.buffer, ptr, 1);
  return view[0];
}

/**
 * Read a uint32 value from WASM memory.
 * @param ptr Pointer to the uint32
 * @returns The uint32 value
 */
export function readU32(ptr: Ptr): number {
  const memory = getMemory();
  const view = new Uint32Array(memory.buffer, ptr, 1);
  return view[0];
}
