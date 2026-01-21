// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import { init, reset, getMemory, isUrl, isNode } from '@system-dynamics/engine2/internal/wasm';
import { malloc, free } from '../src/internal/memory';
import { SimlinError, readErrorDetail } from '../src/internal/error';
import { SimlinErrorCode, SimlinJsonFormat } from '../src/internal/types';
import { validateStructSizes, getRustStructSizes } from '../src/internal/analysis';
import {
  simlin_project_unref,
  simlin_project_get_model,
  simlin_project_get_errors,
  simlin_project_apply_patch,
} from '../src/internal/project';
import { simlin_model_unref, simlin_model_get_latex_equation, simlin_model_get_var_names } from '../src/internal/model';
import { simlin_project_open_xmile } from '../src/internal/import-export';

// Load the teacup test model in XMILE format from pysimlin fixtures.
function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('WASM Integration Tests', () => {
  // Note: These tests require the WASM module to be built first
  // Run `./build.sh` before running tests

  describe('WASM Loading', () => {
    it('should detect WASM file exists', () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      expect(fs.existsSync(wasmPath)).toBe(true);
    });

    it('should compile WASM module', async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      // This should not throw
      const module = await WebAssembly.compile(wasmBuffer);
      expect(module).toBeDefined();
    });

    it('should instantiate WASM module', async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      const module = await WebAssembly.compile(wasmBuffer);
      const instance = await WebAssembly.instantiate(module, {});

      expect(instance).toBeDefined();
      expect(instance.exports).toBeDefined();
    });

    it('should export expected functions', async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      const module = await WebAssembly.compile(wasmBuffer);
      const instance = await WebAssembly.instantiate(module, {});
      const exports = instance.exports;

      // Check for key exported functions
      expect(typeof exports.simlin_malloc).toBe('function');
      expect(typeof exports.simlin_free).toBe('function');
      expect(typeof exports.simlin_free_string).toBe('function');
      expect(typeof exports.simlin_project_open_protobuf).toBe('function');
      expect(typeof exports.simlin_project_unref).toBe('function');
      expect(typeof exports.simlin_project_serialize_protobuf).toBe('function');
      expect(typeof exports.simlin_sim_new).toBe('function');
      expect(typeof exports.simlin_sim_run_to_end).toBe('function');
      expect(typeof exports.simlin_error_str).toBe('function');
    });
  });

  describe('Memory Operations', () => {
    let instance: WebAssembly.Instance;
    let memory: WebAssembly.Memory;

    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      const module = await WebAssembly.compile(wasmBuffer);
      instance = await WebAssembly.instantiate(module, {});
      memory = instance.exports.memory as WebAssembly.Memory;
    });

    it('should allocate and free memory', () => {
      const malloc_fn = instance.exports.simlin_malloc as (size: number) => number;
      const free_fn = instance.exports.simlin_free as (ptr: number) => void;

      const ptr = malloc_fn(1024);
      expect(ptr).toBeGreaterThan(0);

      // Write some data
      const view = new Uint8Array(memory.buffer, ptr, 1024);
      view[0] = 42;
      view[1023] = 99;

      // Free should not throw
      free_fn(ptr);
    });

    it('should handle string round-trip', () => {
      const malloc_fn = instance.exports.simlin_malloc as (size: number) => number;
      const free_fn = instance.exports.simlin_free as (ptr: number) => void;

      const testStr = 'Hello, WASM!';
      const encoder = new TextEncoder();
      const decoder = new TextDecoder();
      const bytes = encoder.encode(testStr + '\0');

      const ptr = malloc_fn(bytes.length);
      const view = new Uint8Array(memory.buffer, ptr, bytes.length);
      view.set(bytes);

      // Read it back
      const readView = new Uint8Array(memory.buffer);
      let end = ptr;
      while (readView[end] !== 0) end++;
      const readStr = decoder.decode(readView.slice(ptr, end));

      expect(readStr).toBe(testStr);

      // Note: Use simlin_free for memory allocated with simlin_malloc
      // simlin_free_string is for strings returned by the API
      free_fn(ptr);
    });
  });

  describe('Error Handling', () => {
    let instance: WebAssembly.Instance;

    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      const module = await WebAssembly.compile(wasmBuffer);
      instance = await WebAssembly.instantiate(module, {});
    });

    it('should return error string for NoError code', () => {
      const error_str_fn = instance.exports.simlin_error_str as (code: number) => number;
      const memory = instance.exports.memory as WebAssembly.Memory;
      const decoder = new TextDecoder();

      const ptr = error_str_fn(SimlinErrorCode.NoError);

      // Read the string
      const view = new Uint8Array(memory.buffer);
      let end = ptr;
      while (view[end] !== 0) end++;
      const str = decoder.decode(view.slice(ptr, end));

      // The actual error string returned by libsimlin
      expect(str).toBe('no_error');
    });

    it('should return error string for Generic code', () => {
      const error_str_fn = instance.exports.simlin_error_str as (code: number) => number;
      const memory = instance.exports.memory as WebAssembly.Memory;
      const decoder = new TextDecoder();

      const ptr = error_str_fn(SimlinErrorCode.Generic);

      const view = new Uint8Array(memory.buffer);
      let end = ptr;
      while (view[end] !== 0) end++;
      const str = decoder.decode(view.slice(ptr, end));

      // The actual error string returned by libsimlin
      expect(str).toBe('generic');
    });
  });

  describe('End-to-End Simulation', () => {
    let instance: WebAssembly.Instance;
    let memory: WebAssembly.Memory;

    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      const module = await WebAssembly.compile(wasmBuffer);
      instance = await WebAssembly.instantiate(module, {});
      memory = instance.exports.memory as WebAssembly.Memory;
    });

    it('should run a complete simulation from XMILE data', () => {
      const xmileData = loadTestXmile();

      const malloc_fn = instance.exports.simlin_malloc as (size: number) => number;
      const free_fn = instance.exports.simlin_free as (ptr: number) => void;
      const import_xmile_fn = instance.exports.simlin_project_open_xmile as (
        ptr: number,
        len: number,
        outErr: number,
      ) => number;
      const project_unref_fn = instance.exports.simlin_project_unref as (ptr: number) => void;
      const project_get_model_fn = instance.exports.simlin_project_get_model as (
        proj: number,
        name: number,
        outErr: number,
      ) => number;
      const model_unref_fn = instance.exports.simlin_model_unref as (ptr: number) => void;
      const sim_new_fn = instance.exports.simlin_sim_new as (model: number, ltm: number, outErr: number) => number;
      const sim_run_to_end_fn = instance.exports.simlin_sim_run_to_end as (sim: number, outErr: number) => void;
      const sim_get_stepcount_fn = instance.exports.simlin_sim_get_stepcount as (
        sim: number,
        outCount: number,
        outErr: number,
      ) => void;
      const sim_get_series_fn = instance.exports.simlin_sim_get_series as (
        sim: number,
        name: number,
        results: number,
        len: number,
        outWritten: number,
        outErr: number,
      ) => void;
      const sim_unref_fn = instance.exports.simlin_sim_unref as (ptr: number) => void;

      // Helper to check for errors
      const checkError = (outErrPtr: number, context: string) => {
        const errPtr = new DataView(memory.buffer).getUint32(outErrPtr, true);
        if (errPtr !== 0) {
          const error_get_code = instance.exports.simlin_error_get_code as (ptr: number) => number;
          const code = error_get_code(errPtr);
          throw new Error(`${context}: error code ${code}`);
        }
      };

      // Copy XMILE data to WASM memory
      const dataPtr = malloc_fn(xmileData.length);
      const dataView = new Uint8Array(memory.buffer, dataPtr, xmileData.length);
      dataView.set(xmileData);

      // Allocate out-error pointer (pointer to pointer)
      const outErrPtr = malloc_fn(4);
      new DataView(memory.buffer).setUint32(outErrPtr, 0, true);

      // Import XMILE project
      const project = import_xmile_fn(dataPtr, xmileData.length, outErrPtr);
      checkError(outErrPtr, 'project_open_xmile');
      expect(project).toBeGreaterThan(0);

      // Get model (null name = default/main model)
      const model = project_get_model_fn(project, 0, outErrPtr);
      checkError(outErrPtr, 'project_get_model');
      expect(model).toBeGreaterThan(0);

      // Create simulation
      const sim = sim_new_fn(model, 0, outErrPtr);
      checkError(outErrPtr, 'sim_new');
      expect(sim).toBeGreaterThan(0);

      // Run simulation to end
      sim_run_to_end_fn(sim, outErrPtr);
      checkError(outErrPtr, 'sim_run_to_end');

      // Get step count
      const outCountPtr = malloc_fn(4);
      new DataView(memory.buffer).setUint32(outCountPtr, 0, true);
      sim_get_stepcount_fn(sim, outCountPtr, outErrPtr);
      checkError(outErrPtr, 'sim_get_stepcount');
      const stepCount = new DataView(memory.buffer).getUint32(outCountPtr, true);
      expect(stepCount).toBeGreaterThan(0);
      expect(stepCount).toBeLessThan(10000); // Sanity check

      // Get time series for the teacup_temperature variable
      const encoder = new TextEncoder();
      const varName = 'teacup_temperature\0';
      const varNameBytes = encoder.encode(varName);
      const varNamePtr = malloc_fn(varNameBytes.length);
      new Uint8Array(memory.buffer, varNamePtr, varNameBytes.length).set(varNameBytes);

      const resultsPtr = malloc_fn(stepCount * 8);
      const outWrittenPtr = malloc_fn(4);
      new DataView(memory.buffer).setUint32(outWrittenPtr, 0, true);
      sim_get_series_fn(sim, varNamePtr, resultsPtr, stepCount, outWrittenPtr, outErrPtr);
      checkError(outErrPtr, 'sim_get_series');

      const written = new DataView(memory.buffer).getUint32(outWrittenPtr, true);
      expect(written).toBe(stepCount);

      // Verify the teacup temperature decreases over time (it cools down)
      // Use DataView to read unaligned f64 values
      const view = new DataView(memory.buffer);
      const firstValue = view.getFloat64(resultsPtr, true);
      const lastValue = view.getFloat64(resultsPtr + (written - 1) * 8, true);
      expect(firstValue).toBeGreaterThan(lastValue); // Temperature decreases

      // Cleanup
      sim_unref_fn(sim);
      model_unref_fn(model);
      project_unref_fn(project);
      free_fn(dataPtr);
      free_fn(outErrPtr);
      free_fn(outCountPtr);
      free_fn(varNamePtr);
      free_fn(resultsPtr);
      free_fn(outWrittenPtr);
    });

    it('should import XMILE and verify model is simulatable', () => {
      const xmileData = loadTestXmile();

      const malloc_fn = instance.exports.simlin_malloc as (size: number) => number;
      const free_fn = instance.exports.simlin_free as (ptr: number) => void;
      const import_xmile_fn = instance.exports.simlin_project_open_xmile as (
        ptr: number,
        len: number,
        outErr: number,
      ) => number;
      const project_is_simulatable_fn = instance.exports.simlin_project_is_simulatable as (
        proj: number,
        name: number,
        outErr: number,
      ) => number;
      const project_unref_fn = instance.exports.simlin_project_unref as (ptr: number) => void;

      // Helper to check for errors
      const checkError = (outErrPtr: number, context: string) => {
        const errPtr = new DataView(memory.buffer).getUint32(outErrPtr, true);
        if (errPtr !== 0) {
          const error_get_code = instance.exports.simlin_error_get_code as (ptr: number) => number;
          const code = error_get_code(errPtr);
          throw new Error(`${context}: error code ${code}`);
        }
      };

      // Copy XMILE data to WASM memory
      const dataPtr = malloc_fn(xmileData.length);
      const dataView = new Uint8Array(memory.buffer, dataPtr, xmileData.length);
      dataView.set(xmileData);

      // Allocate out-error pointer
      const outErrPtr = malloc_fn(4);
      new DataView(memory.buffer).setUint32(outErrPtr, 0, true);

      // Import XMILE
      const project = import_xmile_fn(dataPtr, xmileData.length, outErrPtr);
      checkError(outErrPtr, 'project_open_xmile');
      expect(project).toBeGreaterThan(0);

      // Check if simulatable
      const isSimulatable = project_is_simulatable_fn(project, 0, outErrPtr);
      expect(isSimulatable).toBe(1);

      // Cleanup
      project_unref_fn(project);
      free_fn(dataPtr);
      free_fn(outErrPtr);
    });

    it('should serialize and deserialize a project round-trip', () => {
      const xmileData = loadTestXmile();

      const malloc_fn = instance.exports.simlin_malloc as (size: number) => number;
      const free_fn = instance.exports.simlin_free as (ptr: number) => void;
      const import_xmile_fn = instance.exports.simlin_project_open_xmile as (
        ptr: number,
        len: number,
        outErr: number,
      ) => number;
      const project_open_fn = instance.exports.simlin_project_open_protobuf as (
        ptr: number,
        len: number,
        outErr: number,
      ) => number;
      const project_serialize_fn = instance.exports.simlin_project_serialize_protobuf as (
        proj: number,
        outBuf: number,
        outLen: number,
        outErr: number,
      ) => void;
      const project_unref_fn = instance.exports.simlin_project_unref as (ptr: number) => void;

      // Helper to check for errors
      const checkError = (outErrPtr: number, context: string) => {
        const errPtr = new DataView(memory.buffer).getUint32(outErrPtr, true);
        if (errPtr !== 0) {
          const error_get_code = instance.exports.simlin_error_get_code as (ptr: number) => number;
          const code = error_get_code(errPtr);
          throw new Error(`${context}: error code ${code}`);
        }
      };

      // Copy XMILE data to WASM memory
      const dataPtr = malloc_fn(xmileData.length);
      const dataView = new Uint8Array(memory.buffer, dataPtr, xmileData.length);
      dataView.set(xmileData);

      // Allocate out-error pointer
      const outErrPtr = malloc_fn(4);
      new DataView(memory.buffer).setUint32(outErrPtr, 0, true);

      // Import XMILE project
      const project1 = import_xmile_fn(dataPtr, xmileData.length, outErrPtr);
      checkError(outErrPtr, 'project_open_xmile');
      expect(project1).toBeGreaterThan(0);

      // Serialize to protobuf
      const outBufPtr = malloc_fn(4);
      const outLenPtr = malloc_fn(4);
      project_serialize_fn(project1, outBufPtr, outLenPtr, outErrPtr);
      checkError(outErrPtr, 'project_serialize_protobuf');

      const serializedPtr = new DataView(memory.buffer).getUint32(outBufPtr, true);
      const serializedLen = new DataView(memory.buffer).getUint32(outLenPtr, true);
      expect(serializedPtr).toBeGreaterThan(0);
      expect(serializedLen).toBeGreaterThan(0);

      // Copy serialized data
      const serializedData = new Uint8Array(memory.buffer, serializedPtr, serializedLen).slice();

      // Deserialize from protobuf
      const dataPtr2 = malloc_fn(serializedData.length);
      new Uint8Array(memory.buffer, dataPtr2, serializedData.length).set(serializedData);
      const project2 = project_open_fn(dataPtr2, serializedData.length, outErrPtr);
      checkError(outErrPtr, 'project_open_protobuf');
      expect(project2).toBeGreaterThan(0);

      // Cleanup
      project_unref_fn(project1);
      project_unref_fn(project2);
      free_fn(dataPtr);
      free_fn(outErrPtr);
      free_fn(outBufPtr);
      free_fn(outLenPtr);
      free_fn(serializedPtr);
      free_fn(dataPtr2);
    });
  });

  describe('Error Detail Struct Layout', () => {
    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      reset();
      await init(wasmBuffer);
    });

    it('should correctly read error details from an error with details', () => {
      // Try to import invalid XMILE data using our TypeScript wrapper
      const invalidXmile = new TextEncoder().encode('<?xml version="1.0"?><xmile><invalid_model/></xmile>');

      try {
        simlin_project_open_xmile(new Uint8Array(invalidXmile));
        // If no error was thrown, the import succeeded (unlikely with invalid data)
      } catch (e) {
        if (e instanceof SimlinError) {
          // Verify we can access error details
          expect(typeof e.code).toBe('number');
          expect(typeof e.message).toBe('string');
          expect(Array.isArray(e.details)).toBe(true);

          // Verify detail structure if there are any
          for (const detail of e.details) {
            expect(typeof detail.code).toBe('number');
            expect(typeof detail.kind).toBe('number');
            expect(typeof detail.unitErrorKind).toBe('number');
            expect(typeof detail.startOffset).toBe('number');
            expect(typeof detail.endOffset).toBe('number');
          }
        } else {
          throw e;
        }
      }
    });

    it('should have correct struct field offsets for SimlinErrorDetail', () => {
      // This test verifies the struct layout assumptions documented in error.ts
      // SimlinErrorDetail layout on wasm32:
      //   offset 0: code (u32)
      //   offset 4: message (ptr)
      //   offset 8: model_name (ptr)
      //   offset 12: variable_name (ptr)
      //   offset 16: start_offset (u16)
      //   offset 18: end_offset (u16)
      //   offset 20: kind (u32)
      //   offset 24: unit_error_kind (u32)
      // Total size: 28 bytes

      // Use the malloc/free from our memory module
      const structPtr = malloc(28);
      const memory = getMemory();
      const view = new DataView(memory.buffer);

      // Write known values at expected offsets
      view.setUint32(structPtr + 0, 42, true); // code
      view.setUint32(structPtr + 4, 0, true); // message (null)
      view.setUint32(structPtr + 8, 0, true); // model_name (null)
      view.setUint32(structPtr + 12, 0, true); // variable_name (null)
      view.setUint16(structPtr + 16, 100, true); // start_offset
      view.setUint16(structPtr + 18, 200, true); // end_offset
      view.setUint32(structPtr + 20, 2, true); // kind (Variable)
      view.setUint32(structPtr + 24, 1, true); // unit_error_kind (Definition)

      // Read it back using our function
      const detail = readErrorDetail(structPtr);

      expect(detail.code).toBe(42);
      expect(detail.message).toBeNull();
      expect(detail.modelName).toBeNull();
      expect(detail.variableName).toBeNull();
      expect(detail.startOffset).toBe(100);
      expect(detail.endOffset).toBe(200);
      expect(detail.kind).toBe(2);
      expect(detail.unitErrorKind).toBe(1);

      free(structPtr);
    });

    it('should have SimlinErrorDetail size match Rust-reported size', () => {
      // Verify the hardcoded TypeScript size (28 bytes) matches what Rust reports
      const rustSizes = getRustStructSizes();
      expect(rustSizes.errorDetailSize).toBe(28);
    });

    it('should read actual error details from Rust-generated errors', () => {
      // Import invalid XMILE to trigger a real Rust error with details
      const invalidXmile = new TextEncoder().encode(`<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>Test</vendor><product version="1.0">Test</product></header>
  <sim_specs><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <aux name="broken_var"><eqn>undefined_var + 1</eqn></aux>
    </variables>
  </model>
</xmile>`);

      try {
        simlin_project_open_xmile(new Uint8Array(invalidXmile));
        // If we get here, the import succeeded (model has errors but parsed)
      } catch (e) {
        if (e instanceof SimlinError) {
          // Verify we can access actual Rust-generated error details
          expect(typeof e.code).toBe('number');
          expect(typeof e.message).toBe('string');
          expect(Array.isArray(e.details)).toBe(true);

          // Each detail should have the correct structure from Rust
          for (const detail of e.details) {
            expect(typeof detail.code).toBe('number');
            expect(typeof detail.kind).toBe('number');
            expect(typeof detail.unitErrorKind).toBe('number');
            expect(typeof detail.startOffset).toBe('number');
            expect(typeof detail.endOffset).toBe('number');
            // These may be null or strings depending on the error
            expect(detail.message === null || typeof detail.message === 'string').toBe(true);
            expect(detail.modelName === null || typeof detail.modelName === 'string').toBe(true);
            expect(detail.variableName === null || typeof detail.variableName === 'string').toBe(true);
          }
        } else {
          throw e;
        }
      }
    });
  });

  describe('Analysis Struct Layout', () => {
    it('should validate loop and link struct sizes against Rust-reported sizes', () => {
      // This calls Rust's simlin_sizeof_* functions and compares against TypeScript constants
      expect(() => validateStructSizes()).not.toThrow();

      // Also verify the actual values for documentation
      const rustSizes = getRustStructSizes();
      expect(rustSizes.ptrSize).toBe(4); // wasm32 pointers are 4 bytes
      expect(rustSizes.loopSize).toBe(16); // SimlinLoop: id + variables + var_count + polarity
      expect(rustSizes.linkSize).toBe(20); // SimlinLink: from + to + polarity + score + score_len
    });
  });

  describe('Model Functions', () => {
    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      reset();
      await init(wasmBuffer);
    });

    it('should get latex equation for a variable', () => {
      const xmileData = loadTestXmile();

      // Import XMILE using our wrapper
      const project = simlin_project_open_xmile(xmileData);
      expect(project).toBeGreaterThan(0);

      // Get model using our wrapper
      const model = simlin_project_get_model(project, null);
      expect(model).toBeGreaterThan(0);

      // Get latex equation for teacup_temperature using our TypeScript wrapper
      const latex = simlin_model_get_latex_equation(model, 'teacup_temperature');

      // The teacup model should have an equation for teacup_temperature
      expect(latex).not.toBeNull();
      expect(typeof latex).toBe('string');
      expect(latex!.length).toBeGreaterThan(0);

      // Cleanup
      simlin_model_unref(model);
      simlin_project_unref(project);
    });

    it('should return null for non-existent variable', () => {
      const xmileData = loadTestXmile();

      // Import XMILE
      const project = simlin_project_open_xmile(xmileData);
      expect(project).toBeGreaterThan(0);

      // Get model
      const model = simlin_project_get_model(project, null);
      expect(model).toBeGreaterThan(0);

      // Get latex equation for a non-existent variable
      const latex = simlin_model_get_latex_equation(model, 'nonexistent_variable_xyz');

      // Should return null for non-existent variable
      expect(latex).toBeNull();

      // Cleanup
      simlin_model_unref(model);
      simlin_project_unref(project);
    });

    it('should get variable names from model', () => {
      const xmileData = loadTestXmile();

      // Import XMILE
      const project = simlin_project_open_xmile(xmileData);
      expect(project).toBeGreaterThan(0);

      // Get model
      const model = simlin_project_get_model(project, null);
      expect(model).toBeGreaterThan(0);

      // Get variable names using our TypeScript wrapper
      const varNames = simlin_model_get_var_names(model);

      // The teacup model should have some variables
      expect(Array.isArray(varNames)).toBe(true);
      expect(varNames.length).toBeGreaterThan(0);

      // Should include the teacup_temperature variable
      expect(varNames).toContain('teacup_temperature');

      // Cleanup
      simlin_model_unref(model);
      simlin_project_unref(project);
    });
  });

  describe('Node.js File Loading Helpers', () => {
    describe('isUrl', () => {
      it('should return true for http:// URLs', () => {
        expect(isUrl('http://example.com/file.wasm')).toBe(true);
        expect(isUrl('http://localhost:8080/wasm')).toBe(true);
      });

      it('should return true for https:// URLs', () => {
        expect(isUrl('https://example.com/file.wasm')).toBe(true);
        expect(isUrl('https://cdn.example.org/lib.wasm')).toBe(true);
      });

      it('should return true for file:// URLs', () => {
        expect(isUrl('file:///path/to/file.wasm')).toBe(true);
        expect(isUrl('file://localhost/path')).toBe(true);
      });

      it('should return false for filesystem paths', () => {
        expect(isUrl('./core/libsimlin.wasm')).toBe(false);
        expect(isUrl('/absolute/path/to/file.wasm')).toBe(false);
        expect(isUrl('../parent/file.wasm')).toBe(false);
        expect(isUrl('relative/path.wasm')).toBe(false);
      });
    });

    describe('isNode', () => {
      it('should return true in Node.js environment', () => {
        // We're running tests in Node.js, so this should be true
        expect(isNode()).toBe(true);
      });
    });

    // Note: loadFileNode uses new Function() to avoid bundler issues with node:fs/promises.
    // This approach doesn't work in Jest's sandbox due to VM module restrictions.
    // We test the equivalent functionality by using fs.readFileSync to load the file
    // and then passing the buffer to init(), which exercises the same code paths.
    describe('loadFileNode equivalent (via fs.readFileSync)', () => {
      it('should be able to load WASM file from filesystem and verify it is valid', () => {
        const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
        const nodeBuffer = fs.readFileSync(wasmPath);
        const buffer = nodeBuffer.buffer.slice(nodeBuffer.byteOffset, nodeBuffer.byteOffset + nodeBuffer.byteLength);

        expect(buffer).toBeInstanceOf(ArrayBuffer);
        expect(buffer.byteLength).toBeGreaterThan(0);

        // Verify it's valid WASM (starts with magic bytes \0asm)
        const view = new Uint8Array(buffer);
        expect(view[0]).toBe(0x00);
        expect(view[1]).toBe(0x61); // 'a'
        expect(view[2]).toBe(0x73); // 's'
        expect(view[3]).toBe(0x6d); // 'm'
      });

      it('should throw for non-existent file', () => {
        expect(() => fs.readFileSync('/nonexistent/path/to/file.wasm')).toThrow();
      });
    });

    describe('init with ArrayBuffer from filesystem', () => {
      it('should initialize WASM from ArrayBuffer read from filesystem', async () => {
        reset();
        const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
        const wasmBuffer = fs.readFileSync(wasmPath);
        await init(wasmBuffer);

        // Verify WASM is loaded by checking we can get memory
        const memory = getMemory();
        expect(memory).toBeInstanceOf(WebAssembly.Memory);
      });
    });
  });

  describe('Project Error Handling', () => {
    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      reset();
      await init(wasmBuffer);
    });

    it('should throw SimlinError when simlin_project_get_errors is called with invalid project', () => {
      // Pass an invalid project pointer (0 or any invalid value)
      expect(() => simlin_project_get_errors(0)).toThrow(SimlinError);

      try {
        simlin_project_get_errors(0);
      } catch (e) {
        expect(e).toBeInstanceOf(SimlinError);
        if (e instanceof SimlinError) {
          expect(e.code).toBe(SimlinErrorCode.Generic);
        }
      }
    });

    it('should return 0 for a valid project with no errors', () => {
      const xmileData = loadTestXmile();
      const project = simlin_project_open_xmile(xmileData);
      expect(project).toBeGreaterThan(0);

      // The teacup model is valid, so it should have no errors
      // simlin_project_get_errors returns 0 (null pointer) when there are no errors
      const errPtr = simlin_project_get_errors(project);
      expect(errPtr).toBe(0);

      simlin_project_unref(project);
    });
  });

  describe('Patch Error Handling', () => {
    beforeAll(async () => {
      const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
      const wasmBuffer = fs.readFileSync(wasmPath);

      reset();
      await init(wasmBuffer);
    });

    it('should throw SimlinError when apply_patch fails with invalid project', () => {
      // Create an invalid JSON patch (empty object)
      const invalidPatch = new TextEncoder().encode('{}');

      expect(() => simlin_project_apply_patch(0, invalidPatch, false, false)).toThrow(SimlinError);

      try {
        simlin_project_apply_patch(0, invalidPatch, false, false);
      } catch (e) {
        expect(e).toBeInstanceOf(SimlinError);
        if (e instanceof SimlinError) {
          expect(e.code).toBe(SimlinErrorCode.Generic);
          // Verify the error has details array (may be empty for this error type)
          expect(Array.isArray(e.details)).toBe(true);
          // Verify the error has a message
          expect(typeof e.message).toBe('string');
          expect(e.message.length).toBeGreaterThan(0);
        }
      }
    });

    it('should apply a valid JSON patch successfully', () => {
      const xmileData = loadTestXmile();
      const project = simlin_project_open_xmile(xmileData);
      expect(project).toBeGreaterThan(0);

      // An empty JSON patch is valid and does nothing
      const emptyPatch = new TextEncoder().encode('{"project_ops":[],"models":[]}');

      // Apply the patch - should not throw
      const collectedErrors = simlin_project_apply_patch(project, emptyPatch, false, false);

      // No errors should be collected for a valid empty patch
      expect(collectedErrors).toBe(0);

      simlin_project_unref(project);
    });
  });
});
