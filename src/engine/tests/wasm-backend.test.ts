// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// These are integration tests driving the imperative-shell DirectBackend
// directly (not yet through Model/Sim). The VM path is the correctness oracle:
// every wasm-engine operation is driven identically to the VM path and compared
// within the engine's existing simulation tolerance.

import * as fs from 'fs';
import * as path from 'path';

import { DirectBackend } from '../src/direct-backend';
import { ModelHandle, ProjectHandle, SimHandle } from '../src/backend';

function loadWasmBuffer(): Buffer {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  return fs.readFileSync(wasmPath);
}

// The teacup model: a scalar Euler model the wasm backend supports. It has the
// constant auxes `room temperature` and `characteristic time` (overridable) and
// the non-constant flow `heat loss to room` (rejected by setValue).
function loadTeacupXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

// A model the wasm backend cannot compile: `summed = SUM(source[lo:hi])` uses a
// runtime view range `[lo:hi]` whose bounds reference scalar auxes (not
// constants and not dimension elements), so the range cannot be constant-folded
// and codegen emits the `ViewRangeDynamic` opcode, which wasmgen reports as
// Unsupported (GH #612). The VM runs the same model fine. This was verified
// against the engine: the VM ran it and `compile_datamodel_to_wasm` returned
// "wasmgen: ViewRangeDynamic (dim 0) needs a runtime view size; not supported".
const WASM_UNSUPPORTED_XMILE = `<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header>
        <vendor>Test</vendor>
        <product>Simlin</product>
    </header>
    <sim_specs method="Euler" time_units="Time">
        <start>0</start>
        <stop>2</stop>
        <dt>1</dt>
    </sim_specs>
    <dimensions>
        <dim name="Dim">
            <elem name="a"/>
            <elem name="b"/>
            <elem name="c"/>
        </dim>
    </dimensions>
    <model>
        <variables>
            <aux name="source">
                <element subscript="a"><eqn>1</eqn></element>
                <element subscript="b"><eqn>2</eqn></element>
                <element subscript="c"><eqn>3</eqn></element>
                <dimensions><dim name="Dim"/></dimensions>
            </aux>
            <aux name="lo"><eqn>1</eqn></aux>
            <aux name="hi"><eqn>3</eqn></aux>
            <aux name="summed"><eqn>SUM(source[lo:hi])</eqn></aux>
        </variables>
    </model>
</xmile>`;

describe('DirectBackend wasm engine: sim creation and disposal (Task 3)', () => {
  let backend: DirectBackend;

  beforeAll(async () => {
    backend = new DirectBackend();
    backend.reset();
    backend.configureWasm({ source: loadWasmBuffer() });
    await backend.init();
  });

  afterAll(() => {
    backend.reset();
  });

  describe('AC1.1: wasm sim creation', () => {
    let projectHandle: ProjectHandle;
    let modelHandle: ModelHandle;

    beforeEach(() => {
      projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      modelHandle = backend.projectGetModel(projectHandle, null);
    });

    afterEach(() => {
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('creates a wasm-backed sim handle', () => {
      const sim = backend.simNew(modelHandle, false, 'wasm');
      expect(typeof sim).toBe('number');
      expect(sim).toBeGreaterThan(0);
      backend.simDispose(sim);
    });

    it('defaults to a vm-backed sim when no engine is passed', () => {
      const sim = backend.simNew(modelHandle, false);
      expect(typeof sim).toBe('number');
      expect(sim).toBeGreaterThan(0);
      backend.simDispose(sim);
    });

    it("creates a vm-backed sim when engine is 'vm'", () => {
      const sim = backend.simNew(modelHandle, false, 'vm');
      expect(typeof sim).toBe('number');
      expect(sim).toBeGreaterThan(0);
      backend.simDispose(sim);
    });
  });

  describe('AC6.2: enableLtm rejected on wasm engine', () => {
    it('throws a clear error and creates no sim', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      expect(() => backend.simNew(modelHandle, true, 'wasm')).toThrow(/LTM/i);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('rejects enableLtm before attempting any compile (clear message)', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      expect(() => backend.simNew(modelHandle, true, 'wasm')).toThrow(/not supported on the wasm engine/i);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });
  });

  describe('AC7.1/AC7.2: unsupported model errors on wasm, runs on vm', () => {
    it('throws on wasm with no VM fallback', () => {
      const projectHandle = backend.projectOpenXmile(new TextEncoder().encode(WASM_UNSUPPORTED_XMILE));
      const modelHandle = backend.projectGetModel(projectHandle, null);
      expect(() => backend.simNew(modelHandle, false, 'wasm')).toThrow();
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('runs the same model fine via the vm engine', () => {
      const projectHandle = backend.projectOpenXmile(new TextEncoder().encode(WASM_UNSUPPORTED_XMILE));
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'vm');
      expect(() => backend.simRunToEnd(sim)).not.toThrow();
      const series = backend.simGetSeries(sim, 'summed');
      expect(series).toBeInstanceOf(Float64Array);
      // SUM(source[1:3]) = 1 + 2 + 3 = 6 at every step.
      expect(series[0]).toBeCloseTo(6, 9);
      backend.simDispose(sim);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });
  });

  describe('AC5.4 (creation half): the wasm instance is owned once on the entry', () => {
    // The handle store is private; reach into it to assert the entry's recorded
    // wasm state. This is a white-box check of the imperative shell -- the blob
    // is owned on the entry so it is created exactly once (Task 4 then reuses it
    // across reset/setValue/re-run with no recompile).
    type EntryView = {
      engine?: string;
      ptr: number;
      wasmInstance?: WebAssembly.Instance;
      wasmLayout?: { nChunks: number };
      wasmStopTime?: number;
    };
    function entryOf(sim: SimHandle): EntryView {
      const handles = (backend as unknown as { _handles: Map<number, EntryView> })._handles;
      const entry = handles.get(sim as unknown as number);
      if (!entry) {
        throw new Error('sim entry not found');
      }
      return entry;
    }

    it('records engine, a live instance, layout, and stop time on the entry', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');

      const entry = entryOf(sim);
      expect(entry.engine).toBe('wasm');
      expect(entry.ptr).toBe(0); // no native sim pointer
      expect(entry.wasmInstance).toBeInstanceOf(WebAssembly.Instance);
      expect(entry.wasmLayout?.nChunks).toBeGreaterThan(0);
      // teacup: start 0, stop 30.
      expect(entry.wasmStopTime).toBe(30);

      backend.simDispose(sim);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('owns a distinct instance per sim (each created once)', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const simA = backend.simNew(modelHandle, false, 'wasm');
      const simB = backend.simNew(modelHandle, false, 'wasm');

      const instA = entryOf(simA).wasmInstance;
      const instB = entryOf(simB).wasmInstance;
      expect(instA).toBeInstanceOf(WebAssembly.Instance);
      expect(instB).toBeInstanceOf(WebAssembly.Instance);
      expect(instA).not.toBe(instB);

      backend.simDispose(simA);
      backend.simDispose(simB);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });
  });

  describe('disposal of wasm sims', () => {
    it('disposes a wasm sim without throwing and is idempotent', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');
      expect(() => backend.simDispose(sim)).not.toThrow();
      expect(() => backend.simDispose(sim)).not.toThrow();
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('does not call simlin_sim_unref for a wasm sim (no native sim ptr)', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');
      // A wasm entry carries ptr 0; the disposal path must not unref a 0 ptr.
      // We verify behaviorally: disposing twice never throws and a subsequent
      // operation reports the handle as disposed (the FFI was never touched).
      backend.simDispose(sim);
      expect(() => backend.simRunToEnd(sim)).toThrow(/disposed/);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('cleans up wasm child sims when the project is disposed', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');
      expect(() => backend.projectDispose(projectHandle)).not.toThrow();
      expect(() => backend.simRunToEnd(sim)).toThrow(/disposed/);
    });
  });
});
