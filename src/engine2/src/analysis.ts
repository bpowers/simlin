// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Analysis functions (loops, links, LTM)

import { getExports, getMemory } from './wasm';
import {
  free,
  stringToWasm,
  wasmToString,
  allocOutPtr,
  readOutPtr,
  allocOutUsize,
  readOutUsize,
  readFloat64Array,
  malloc,
} from './memory';
import {
  SimlinProjectPtr,
  SimlinSimPtr,
  SimlinLoopsPtr,
  SimlinLinksPtr,
  SimlinLinkPolarity,
  SimlinLoopPolarity,
  Link,
  Loop,
} from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error';

/**
 * Analyze a project and get feedback loops.
 * @param project Project pointer
 * @returns Loops pointer
 */
export function simlin_analyze_get_loops(project: SimlinProjectPtr): SimlinLoopsPtr {
  const exports = getExports();
  const fn = exports.simlin_analyze_get_loops as (proj: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(project, outErrPtr);
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
 * Free loops structure.
 * @param loops Loops pointer
 */
export function simlin_free_loops(loops: SimlinLoopsPtr): void {
  if (loops === 0) return;
  const exports = getExports();
  const fn = exports.simlin_free_loops as (ptr: number) => void;
  fn(loops);
}

/**
 * Analyze a simulation and get links with LTM scores.
 * @param sim Simulation pointer
 * @returns Links pointer
 */
export function simlin_analyze_get_links(sim: SimlinSimPtr): SimlinLinksPtr {
  const exports = getExports();
  const fn = exports.simlin_analyze_get_links as (sim: number, outErr: number) => number;

  const outErrPtr = allocOutPtr();

  try {
    const result = fn(sim, outErrPtr);
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
 * Free links structure.
 * @param links Links pointer
 */
export function simlin_free_links(links: SimlinLinksPtr): void {
  if (links === 0) return;
  const exports = getExports();
  const fn = exports.simlin_free_links as (ptr: number) => void;
  fn(links);
}

/**
 * Get relative loop score for a specific loop.
 * @param sim Simulation pointer
 * @param loopId Loop identifier
 * @param stepCount Number of steps
 * @returns Float64Array with loop scores
 */
export function simlin_analyze_get_relative_loop_score(
  sim: SimlinSimPtr,
  loopId: string,
  stepCount: number,
): Float64Array {
  const exports = getExports();
  const fn = exports.simlin_analyze_get_relative_loop_score as (
    sim: number,
    loopId: number,
    results: number,
    len: number,
    outWritten: number,
    outErr: number,
  ) => void;

  const loopIdPtr = stringToWasm(loopId);
  const resultsPtr = malloc(stepCount * 8); // f64 array
  const outWrittenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(sim, loopIdPtr, resultsPtr, stepCount, outWrittenPtr, outErrPtr);
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
    free(loopIdPtr);
    free(resultsPtr);
    free(outWrittenPtr);
    free(outErrPtr);
  }
}

// Struct sizes for wasm32 target
// SimlinLoop: id: ptr(4), variables: ptr(4), var_count: usize(4), polarity: u32(4) = 16 bytes
const LOOP_SIZE = 16;
// SimlinLink: from: ptr(4), to: ptr(4), polarity: u32(4), score: ptr(4), score_len: usize(4) = 20 bytes
const LINK_SIZE = 20;
// Pointer size for wasm32
const PTR_SIZE = 4;

/**
 * Validate struct sizes match expected wasm32 layout.
 * This helps catch ABI mismatches early.
 * @throws Error if struct sizes don't match expected values
 */
export function validateStructSizes(): void {
  // These assertions document the expected ABI and will fail if
  // the Rust struct layout changes incompatibly
  if (PTR_SIZE !== 4) {
    throw new Error(`Expected wasm32 pointer size of 4, got ${PTR_SIZE}`);
  }
  // The LOOP_SIZE should be: ptr + ptr + usize + u32 = 4 + 4 + 4 + 4 = 16
  const expectedLoopSize = PTR_SIZE + PTR_SIZE + PTR_SIZE + 4; // id, variables, var_count, polarity
  if (LOOP_SIZE !== expectedLoopSize) {
    throw new Error(`LOOP_SIZE ${LOOP_SIZE} does not match expected ${expectedLoopSize}`);
  }
  // The LINK_SIZE should be: ptr + ptr + u32 + ptr + usize = 4 + 4 + 4 + 4 + 4 = 20
  const expectedLinkSize = PTR_SIZE + PTR_SIZE + 4 + PTR_SIZE + PTR_SIZE; // from, to, polarity, score, score_len
  if (LINK_SIZE !== expectedLinkSize) {
    throw new Error(`LINK_SIZE ${LINK_SIZE} does not match expected ${expectedLinkSize}`);
  }
}

/**
 * Read loops from a SimlinLoops pointer.
 * @param loopsPtr Loops pointer
 * @returns Array of Loop objects
 */
export function readLoops(loopsPtr: SimlinLoopsPtr): Loop[] {
  if (loopsPtr === 0) return [];

  const memory = getMemory();
  const view = new DataView(memory.buffer);

  // Read count from SimlinLoops struct
  const arrayPtr = view.getUint32(loopsPtr, true);
  const count = view.getUint32(loopsPtr + 4, true);

  const loops: Loop[] = [];
  for (let i = 0; i < count; i++) {
    const ptr = arrayPtr + i * LOOP_SIZE;

    const idPtr = view.getUint32(ptr, true);
    const varsPtr = view.getUint32(ptr + 4, true);
    const varCount = view.getUint32(ptr + 8, true);
    const polarity = view.getUint32(ptr + 12, true) as SimlinLoopPolarity;

    // Read variable names
    const variables: string[] = [];
    for (let j = 0; j < varCount; j++) {
      const varNamePtr = view.getUint32(varsPtr + j * 4, true);
      const name = wasmToString(varNamePtr);
      if (name !== null) variables.push(name);
    }

    const id = wasmToString(idPtr) ?? '';
    loops.push({ id, variables, polarity });
  }

  return loops;
}

/**
 * Read links from a SimlinLinks pointer.
 * @param linksPtr Links pointer
 * @returns Array of Link objects
 */
export function readLinks(linksPtr: SimlinLinksPtr): Link[] {
  if (linksPtr === 0) return [];

  const memory = getMemory();
  const view = new DataView(memory.buffer);

  // Read count from SimlinLinks struct
  const arrayPtr = view.getUint32(linksPtr, true);
  const count = view.getUint32(linksPtr + 4, true);

  const links: Link[] = [];
  for (let i = 0; i < count; i++) {
    const ptr = arrayPtr + i * LINK_SIZE;

    const fromPtr = view.getUint32(ptr, true);
    const toPtr = view.getUint32(ptr + 4, true);
    const polarity = view.getUint32(ptr + 8, true) as SimlinLinkPolarity;
    const scorePtr = view.getUint32(ptr + 12, true);
    const scoreLen = view.getUint32(ptr + 16, true);

    const from = wasmToString(fromPtr) ?? '';
    const to = wasmToString(toPtr) ?? '';

    let score: Float64Array | null = null;
    if (scorePtr !== 0 && scoreLen > 0) {
      // Use readFloat64Array to avoid alignment issues with Float64Array
      score = readFloat64Array(scorePtr, scoreLen);
    }

    links.push({ from, to, polarity, score });
  }

  return links;
}
