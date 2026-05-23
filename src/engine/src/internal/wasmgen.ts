// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Mixed (unavoidable)
// Reason: This module is the complete "compile a model to a wasm blob and read
// it back" contract. The two pure functions (parseWasmLayout, readStridedSeries)
// are the Functional Core; the FFI wrapper (simlin_model_compile_to_wasm) is the
// Imperative Shell. They live together because the shell produces the very layout
// bytes the core decodes and they share the WasmLayout/WasmBlobExports types --
// the same arrangement the sibling FFI wrappers in model.ts use. The pure
// functions take plain buffers (not a live instance) so they remain unit-testable
// in isolation, which their tests exercise without any WASM instance.

import { getExports } from '@simlin/engine/internal/wasm';
import { free, copyFromWasm, allocOutPtr, readOutPtr, allocOutUsize, readOutUsize } from './memory';
import { SimlinModelPtr } from './types';
import {
  simlin_error_free,
  simlin_error_get_code,
  simlin_error_get_message,
  readAllErrorDetails,
  SimlinError,
} from './error';

const textDecoder = new TextDecoder();

/**
 * Geometry and the canonical-name -> slot-offset map decoded from a compiled
 * model's serialized WasmLayout.
 *
 * `varOffsets` keys are canonical idents (the same keys the VM's
 * `simlin_sim_get_var_names` returns); a caller must canonicalize a raw name
 * before looking it up. Results in the blob's linear memory are stored
 * step-major (one contiguous chunk of `nSlots` f64 per saved step), so a
 * variable's series is read by striding the results region by `nSlots`.
 */
export interface WasmLayout {
  /** Number of f64 slots per saved step (the step-major row width). */
  nSlots: number;
  /** Number of saved steps (== the VM's saved-row count / series length). */
  nChunks: number;
  /** Byte offset of the results region within the blob's linear memory. */
  resultsOffset: number;
  /** Canonical variable name -> its f64 slot offset within a step. */
  varOffsets: Map<string, number>;
}

/**
 * The exports of a compiled-model wasm blob. The blob is import-free; the host
 * instantiates it and drives `run`/`run_to`/`reset` directly (libsimlin is not
 * on this hot path). `run_to` is resumable: it calls the idempotent
 * `run_initials` internally and resumes from where a prior call stopped;
 * `reset` clears the run cursor while preserving constant overrides.
 */
export interface WasmBlobExports {
  memory: WebAssembly.Memory;
  run(): void;
  run_to(time: number): void;
  run_initials(): void;
  reset(): void;
  /** Override a constant by slot offset; returns 0 on success, nonzero if the slot is not a settable constant. */
  set_value(offset: number, value: number): number;
  clear_values(): void;
  n_slots: WebAssembly.Global;
  n_chunks: WebAssembly.Global;
  results_offset: WebAssembly.Global;
  /** Live count of saved rows: 0 before any run / after `reset`, `n_chunks` after a full run. */
  saved_steps: WebAssembly.Global;
}

/**
 * Compile a model to a self-contained WebAssembly blob plus its serialized
 * WasmLayout.
 *
 * Imperative shell: mirrors `model.ts`'s `callBufferReturningFn` error-ptr +
 * out-buffer idiom, but returns two buffers (the wasm bytes and the layout
 * bytes). On an unsupported model the FFI stores a `SimlinErrorCode::Generic`
 * error, which surfaces here as a thrown `SimlinError` (no VM fallback).
 *
 * @param model Model pointer (owned by the caller).
 * @returns The wasm blob bytes and the serialized WasmLayout bytes.
 */
export function simlin_model_compile_to_wasm(model: SimlinModelPtr): { wasm: Uint8Array; layout: Uint8Array } {
  const fn = getExports().simlin_model_compile_to_wasm as (
    model: number,
    outWasm: number,
    outWasmLen: number,
    outLayout: number,
    outLayoutLen: number,
    outErr: number,
  ) => void;

  const outWasmPtr = allocOutPtr();
  const outWasmLenPtr = allocOutUsize();
  const outLayoutPtr = allocOutPtr();
  const outLayoutLenPtr = allocOutUsize();
  const outErrPtr = allocOutPtr();

  try {
    fn(model, outWasmPtr, outWasmLenPtr, outLayoutPtr, outLayoutLenPtr, outErrPtr);
    const errPtr = readOutPtr(outErrPtr);

    if (errPtr !== 0) {
      const code = simlin_error_get_code(errPtr);
      const message = simlin_error_get_message(errPtr) ?? 'Unknown error';
      const details = readAllErrorDetails(errPtr);
      simlin_error_free(errPtr);
      throw new SimlinError(message, code, details);
    }

    const wasmPtr = readOutPtr(outWasmPtr);
    const wasmLen = readOutUsize(outWasmLenPtr);
    const wasm = copyFromWasm(wasmPtr, wasmLen);
    free(wasmPtr);

    const layoutPtr = readOutPtr(outLayoutPtr);
    const layoutLen = readOutUsize(outLayoutLenPtr);
    const layout = copyFromWasm(layoutPtr, layoutLen);
    free(layoutPtr);

    return { wasm, layout };
  } finally {
    free(outWasmPtr);
    free(outWasmLenPtr);
    free(outLayoutPtr);
    free(outLayoutLenPtr);
    free(outErrPtr);
  }
}

/**
 * Decode a serialized WasmLayout from its little-endian wire format.
 *
 * Functional core: pure decode of the byte buffer
 * `simlin_model_compile_to_wasm` returns. Wire format: u64 nSlots, u64 nChunks,
 * u64 resultsOffset, u32 count, then `count` entries of
 * { u32 nameLen, utf8 name, u64 offset }. The u64 fields are read via
 * `getBigUint64` and narrowed to `number` (slot/offset/step counts are far
 * below 2^53).
 *
 * @param bytes The serialized layout buffer.
 * @returns The decoded geometry and name->offset map.
 */
export function parseWasmLayout(bytes: Uint8Array): WasmLayout {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let p = 0;

  const readU64 = (): number => {
    const value = Number(view.getBigUint64(p, true));
    p += 8;
    return value;
  };
  const readU32 = (): number => {
    const value = view.getUint32(p, true);
    p += 4;
    return value;
  };

  const nSlots = readU64();
  const nChunks = readU64();
  const resultsOffset = readU64();
  const count = readU32();

  const varOffsets = new Map<string, number>();
  for (let i = 0; i < count; i++) {
    const nameLen = readU32();
    const name = textDecoder.decode(bytes.subarray(p, p + nameLen));
    p += nameLen;
    const offset = readU64();
    varOffsets.set(name, offset);
  }

  return { nSlots, nChunks, resultsOffset, varOffsets };
}

/**
 * Read one variable's series out of a step-major results region.
 *
 * Functional core: takes an `ArrayBufferLike` (the blob's linear memory, or any
 * buffer in a test) rather than a live `WebAssembly.Instance`, so it is
 * unit-testable in isolation. Allocates exactly one `Float64Array(nChunks)` and
 * fills it via strided `DataView.getFloat64` reads -- no intermediate arrays.
 *
 * @param memory The linear-memory buffer holding the step-major results region.
 * @param layout The decoded layout (provides resultsOffset, nSlots, nChunks).
 * @param slot The variable's slot offset within a step (from `varOffsets`).
 * @returns A new `Float64Array` of length `nChunks` -- the variable's series.
 */
export function readStridedSeries(memory: ArrayBufferLike, layout: WasmLayout, slot: number): Float64Array {
  const view = new DataView(memory);
  const series = new Float64Array(layout.nChunks);
  for (let c = 0; c < layout.nChunks; c++) {
    series[c] = view.getFloat64(layout.resultsOffset + (c * layout.nSlots + slot) * 8, true);
  }
  return series;
}
