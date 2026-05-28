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

  // A disposed wasm entry is kept in the handle map as a tombstone (so a
  // use-after-dispose still throws the clear "has been disposed" diagnostic),
  // but it must NOT keep pinning the heavy wasm state -- the WebAssembly.Instance
  // and decoded layout -- or memory grows unbounded across create/dispose cycles
  // even though simDispose was called. These white-box checks reach into the
  // private handle map to confirm the heavy refs are released on dispose while
  // the tombstone (and its disposed-error semantics) is preserved.
  describe('disposal releases heavy wasm state but keeps the tombstone', () => {
    type DisposedEntryView = {
      disposed: boolean;
      wasmInstance?: WebAssembly.Instance;
      wasmLayout?: unknown;
      wasmExports?: unknown;
    };
    function entryOf(sim: SimHandle): DisposedEntryView {
      const handles = (backend as unknown as { _handles: Map<number, DisposedEntryView> })._handles;
      const entry = handles.get(sim as unknown as number);
      if (!entry) {
        throw new Error('sim entry not found');
      }
      return entry;
    }

    it('simDispose releases the wasm instance, exports, and layout', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');

      // Precondition: a live wasm sim holds all three heavy refs.
      const live = entryOf(sim);
      expect(live.wasmInstance).toBeInstanceOf(WebAssembly.Instance);
      expect(live.wasmExports).toBeDefined();
      expect(live.wasmLayout).toBeDefined();

      backend.simDispose(sim);

      // The tombstone is preserved (entry still present, marked disposed) ...
      const dead = entryOf(sim);
      expect(dead.disposed).toBe(true);
      // ... but the heavy wasm state is released so GC can reclaim it.
      expect(dead.wasmInstance).toBeUndefined();
      expect(dead.wasmExports).toBeUndefined();
      expect(dead.wasmLayout).toBeUndefined();

      // Nulling the heavy fields must not change the disposed-error semantics:
      // getEntry checks `disposed` before touching any wasm field.
      expect(() => backend.simRunToEnd(sim)).toThrow(/disposed/);

      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });

    it('projectDispose releases heavy wasm state of a live child sim', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const sim = backend.simNew(modelHandle, false, 'wasm');

      // Disposing the project must cascade the same release to its wasm child.
      backend.projectDispose(projectHandle);

      const dead = entryOf(sim);
      expect(dead.disposed).toBe(true);
      expect(dead.wasmInstance).toBeUndefined();
      expect(dead.wasmExports).toBeUndefined();
      expect(dead.wasmLayout).toBeUndefined();
      expect(() => backend.simRunToEnd(sim)).toThrow(/disposed/);
    });
  });
});

// VM-vs-wasm parity: the bytecode VM is the correctness oracle. Each wasm-engine
// operation is driven identically to the VM path and compared within a tight
// tolerance (the wasm blob mirrors the VM opcode-for-opcode, so identical f64
// arithmetic is expected). Teacup is the supported scalar fixture; its constant
// `room temperature` is the override exercised by the setValue cases.
describe('DirectBackend wasm engine: per-op vm/wasm parity (Task 4)', () => {
  let backend: DirectBackend;

  // Tolerance for VM-vs-wasm comparison. Both executors run the same compiled
  // simulation, so the difference is at most floating-point reassociation noise.
  const TOL = 1e-9;

  beforeAll(async () => {
    backend = new DirectBackend();
    backend.reset();
    backend.configureWasm({ source: loadWasmBuffer() });
    await backend.init();
  });

  afterAll(() => {
    backend.reset();
  });

  // Open teacup and return both a vm sim and a wasm sim for the same model, plus
  // a disposer. Each test drives the two identically and compares.
  function openPair(): {
    vm: SimHandle;
    wasm: SimHandle;
    projectHandle: ProjectHandle;
    modelHandle: ModelHandle;
    dispose: () => void;
  } {
    const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
    const modelHandle = backend.projectGetModel(projectHandle, null);
    const vm = backend.simNew(modelHandle, false, 'vm');
    const wasm = backend.simNew(modelHandle, false, 'wasm');
    const dispose = () => {
      backend.simDispose(vm);
      backend.simDispose(wasm);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    };
    return { vm, wasm, projectHandle, modelHandle, dispose };
  }

  function expectSeriesClose(actual: Float64Array, expected: Float64Array): void {
    expect(actual.length).toBe(expected.length);
    for (let i = 0; i < expected.length; i++) {
      expect(Math.abs(actual[i] - expected[i])).toBeLessThanOrEqual(TOL);
    }
  }

  describe('AC2.1: runToEnd series parity', () => {
    it('wasm runToEnd series equal the VM for every variable', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);

      const names = backend.simGetVarNames(wasm);
      expect(names.length).toBeGreaterThan(0);
      for (const name of names) {
        expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
      }
      dispose();
    });
  });

  describe('AC2.2: runTo(t) then getValue parity', () => {
    // After a runTo(t) that stops mid-interval, the live curr chunk is fully
    // self-consistent at the resting time on BOTH backends: stocks + reserved
    // time vars AND every flow/aux/constant are evaluated for the same time and
    // stocks. Both backends re-evaluate root flows at the resting curr after the
    // overshoot break (#625), so a mid-run getValue of ANY variable agrees with
    // the VM -- not just the integrated state. (Previously the VM left stale
    // non-stock slots -- e.g. 0 for a constant -- and the wasm left them one step
    // behind, so this parity was scoped to stocks + reserved time vars.)
    it('wasm getValue after runTo(t) equals the VM for every variable', () => {
      const { vm, wasm, dispose } = openPair();
      const t = 15.05; // mid-interval (teacup dt=0.125): both rest at t=15.125
      backend.simRunTo(vm, t);
      backend.simRunTo(wasm, t);

      // Every variable -- stocks, flows, auxes, constants, and reserved time vars
      // -- is the well-defined "value at the current time" on both backends.
      for (const name of backend.simGetVarNames(wasm)) {
        expect(Math.abs(backend.simGetValue(wasm, name) - backend.simGetValue(vm, name))).toBeLessThanOrEqual(TOL);
      }
      // simGetTime must agree too (it reads slot 0 of the live curr chunk).
      expect(Math.abs(backend.simGetTime(wasm) - backend.simGetTime(vm))).toBeLessThanOrEqual(TOL);
      dispose();
    });

    it('getValue after runToEnd equals the VM for every variable', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      // After a full run the curr chunk is fully evaluated, so every variable --
      // stocks, flows, auxes, constants, and the reserved time vars -- matches.
      for (const name of backend.simGetVarNames(wasm)) {
        expect(Math.abs(backend.simGetValue(wasm, name) - backend.simGetValue(vm, name))).toBeLessThanOrEqual(TOL);
      }
      dispose();
    });
  });

  describe('AC2.3: segmented runTo equals a single runTo and the VM', () => {
    it('runTo(t1)+runTo(t2) equals runTo(t2) and the VM', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const vm = backend.simNew(modelHandle, false, 'vm');
      const wasmSeg = backend.simNew(modelHandle, false, 'wasm');
      const wasmOne = backend.simNew(modelHandle, false, 'wasm');

      const t1 = 7;
      const t2 = 19;
      backend.simRunTo(vm, t2);
      backend.simRunTo(wasmSeg, t1);
      backend.simRunTo(wasmSeg, t2);
      backend.simRunTo(wasmOne, t2);

      // Segmented vs single (wasm-vs-wasm): both fully evaluate their stopping
      // chunk, so getValue agrees on EVERY variable -- this is the core "segments
      // accumulate to the same place" check.
      for (const name of backend.simGetVarNames(wasmSeg)) {
        expect(Math.abs(backend.simGetValue(wasmSeg, name) - backend.simGetValue(wasmOne, name))).toBeLessThanOrEqual(
          TOL,
        );
      }
      // Against the VM: the live integrated state (stock + time) matches mid-run.
      // (Mid-run getSeries is unavailable on the VM -- it builds Results only at
      // the end -- and non-stock getValue is a VM artifact mid-run; see AC2.2.)
      for (const name of ['teacup_temperature', 'time']) {
        expect(Math.abs(backend.simGetValue(wasmSeg, name) - backend.simGetValue(vm, name))).toBeLessThanOrEqual(TOL);
      }

      backend.simDispose(vm);
      backend.simDispose(wasmSeg);
      backend.simDispose(wasmOne);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });
  });

  describe('AC2.4: runTo past the stop time clamps to the end', () => {
    it('runTo(stop*2) equals runToEnd and the VM', () => {
      const projectHandle = backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = backend.projectGetModel(projectHandle, null);
      const vm = backend.simNew(modelHandle, false, 'vm');
      const wasmPast = backend.simNew(modelHandle, false, 'wasm');
      const wasmEnd = backend.simNew(modelHandle, false, 'wasm');

      // teacup stop is 30; run well past it.
      backend.simRunToEnd(vm);
      backend.simRunTo(wasmPast, 60);
      backend.simRunToEnd(wasmEnd);

      for (const name of backend.simGetVarNames(wasmPast)) {
        expectSeriesClose(backend.simGetSeries(wasmPast, name), backend.simGetSeries(wasmEnd, name));
        expectSeriesClose(backend.simGetSeries(wasmPast, name), backend.simGetSeries(vm, name));
      }

      backend.simDispose(vm);
      backend.simDispose(wasmPast);
      backend.simDispose(wasmEnd);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    });
  });

  describe('AC3.1/AC3.2: reset parity', () => {
    it('reset then re-run reproduces the compiled defaults (matches VM)', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(wasm);
      const before = backend.simGetSeries(wasm, 'teacup_temperature');

      backend.simReset(wasm);
      backend.simRunToEnd(wasm);
      const after = backend.simGetSeries(wasm, 'teacup_temperature');

      // Reset+re-run with no override reproduces the same defaults.
      expectSeriesClose(after, before);

      // And matches the VM run.
      backend.simRunToEnd(vm);
      expectSeriesClose(after, backend.simGetSeries(vm, 'teacup_temperature'));
      dispose();
    });

    it('reset preserves a constant override (matches VM reset semantics)', () => {
      const { vm, wasm, dispose } = openPair();

      // Override the same constant on both, run, reset, run again. The VM's
      // reset preserves overrides; the wasm reset must do the same.
      backend.simSetValue(vm, 'room temperature', 40);
      backend.simSetValue(wasm, 'room temperature', 40);
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      backend.simReset(vm);
      backend.simReset(wasm);
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);

      for (const name of backend.simGetVarNames(wasm)) {
        expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
      }
      // Sanity: the override is still in effect after reset (room temperature 40,
      // not the compiled default 70).
      expect(backend.simGetSeries(wasm, 'room_temperature')[0]).toBeCloseTo(40, 9);
      dispose();
    });

    // reset must return the live curr chunk (what getTime/getValue read) to the
    // fresh pre-run state, not leave the previous run's end-of-run values there.
    // The blob's reset clears only the run cursor (mirroring Vm::reset), so the
    // host must present the fresh state -- exactly as libsimlin's FFI does by
    // recreating a zeroed VM. The VM reads 0 for every variable after reset (a
    // freshly-created sim reads 0); the wasm twin must match, not leak stale tail.
    it('reset returns the live curr state to the fresh pre-run state (matches the VM)', () => {
      const { vm, wasm, dispose } = openPair();

      // Run both to the end so the live curr chunk holds end-of-run values, then
      // confirm they agree there before reset (precondition for a meaningful test).
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      expect(backend.simGetValue(wasm, 'teacup_temperature')).toBeGreaterThan(0);

      backend.simReset(vm);
      backend.simReset(wasm);

      // After reset (no re-run) every by-name read must equal the VM's fresh-state
      // value, not the previous run's stale tail. Both are exactly 0 here.
      for (const name of backend.simGetVarNames(wasm)) {
        expect(backend.simGetValue(wasm, name)).toBe(backend.simGetValue(vm, name));
      }
      expect(backend.simGetTime(wasm)).toBe(backend.simGetTime(vm));
      // Spot-check the well-known reads: the stock and the reserved time var.
      expect(backend.simGetValue(wasm, 'teacup_temperature')).toBe(0);
      expect(backend.simGetTime(wasm)).toBe(0);
      dispose();
    });

    // Regression (PR #628 follow-up): a constant override must survive reset in
    // the live curr state too -- getValue of the overridden constant after reset
    // must return the override, matching the VM. The wasm reset used to zero-fill
    // the curr chunk in the host, clobbering the override it had just mirrored;
    // the blob now reapplies overrides into curr on reset, so the host does no
    // shadow write into curr at all.
    it('reset preserves an override in the live curr state (getValue matches VM)', () => {
      const { vm, wasm, dispose } = openPair();

      backend.simSetValue(vm, 'room temperature', 40);
      backend.simSetValue(wasm, 'room temperature', 40);
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      backend.simReset(vm);
      backend.simReset(wasm);

      // The overridden constant reads the override (not 0) on both engines.
      expect(backend.simGetValue(wasm, 'room_temperature')).toBe(40);
      expect(backend.simGetValue(wasm, 'room_temperature')).toBe(backend.simGetValue(vm, 'room_temperature'));
      // A non-overridden variable still reads the fresh-zero state on both.
      expect(backend.simGetValue(wasm, 'teacup_temperature')).toBe(backend.simGetValue(vm, 'teacup_temperature'));
      expect(backend.simGetValue(wasm, 'teacup_temperature')).toBe(0);
      dispose();
    });
  });

  // Regression (PR #628 follow-up): re-running on an already-complete slab (a
  // second runToEnd, or interactive scrubbing that stays at the end) must be a
  // no-op. The blob's run_to used to re-enter its stepping loop and write one
  // results row past the slab -- corrupting adjacent memory and pushing
  // saved_steps to nChunks + 1. The loop now breaks at the top when the slab is
  // full, so the completed-step count and every series stay put.
  describe('re-running on a complete slab is a no-op (no out-of-bounds write)', () => {
    it('runToEnd twice leaves the step count and series unchanged (matches VM)', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);

      const names = backend.simGetVarNames(wasm);
      const before = names.map((n) => backend.simGetSeries(wasm, n));
      const fullCount = backend.simGetStepCount(wasm);

      // Re-trigger the run on the full slab: a second runToEnd and a runTo well
      // past the stop time. Both must do nothing.
      backend.simRunToEnd(wasm);
      backend.simRunTo(wasm, 1e9);

      // The completed-step count must not advance past the slab capacity (the OOB
      // save used to bump saved_steps to nChunks + 1).
      expect(backend.simGetStepCount(wasm)).toBe(fullCount);
      expect(backend.simGetStepCount(wasm)).toBe(backend.simGetStepCount(vm));

      // Every series is unchanged and still matches the VM.
      for (let i = 0; i < names.length; i++) {
        expectSeriesClose(backend.simGetSeries(wasm, names[i]), before[i]);
        expectSeriesClose(backend.simGetSeries(wasm, names[i]), backend.simGetSeries(vm, names[i]));
      }
      dispose();
    });
  });

  describe('AC4.1/AC4.2/AC4.4: by-name reads parity', () => {
    it('getSeries for every variable equals the VM', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      for (const name of backend.simGetVarNames(wasm)) {
        expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
      }
      dispose();
    });

    it('getVarNames and getStepCount equal the VM (exact array equality)', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);

      // The VM's getVarNames includes the reserved time vars (it filters only
      // $-prefixed names); the wasm path must produce the identical array.
      expect(backend.simGetVarNames(wasm)).toEqual(backend.simGetVarNames(vm));
      expect(backend.simGetStepCount(wasm)).toBe(backend.simGetStepCount(vm));

      // The reserved names are present (not filtered out).
      const names = backend.simGetVarNames(wasm);
      expect(names).toContain('time');
      expect(names).toContain('dt');
      expect(names).toContain('initial_time');
      expect(names).toContain('final_time');
      dispose();
    });

    it('getSeries(unknownName) throws like the VM', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      expect(() => backend.simGetSeries(vm, 'definitely_not_a_var')).toThrow();
      expect(() => backend.simGetSeries(wasm, 'definitely_not_a_var')).toThrow();
      dispose();
    });
  });

  describe('AC4.3: getSeries returns a single Float64Array of length nChunks', () => {
    it('returns one Float64Array whose length equals the step count', () => {
      const { wasm, dispose } = openPair();
      backend.simRunToEnd(wasm);
      const stepCount = backend.simGetStepCount(wasm);
      const series = backend.simGetSeries(wasm, 'teacup_temperature');
      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBe(stepCount);
      dispose();
    });
  });

  // getSeries must truncate to the COMPLETED-step count, never the slab capacity
  // (nChunks). The wasm results slab keeps its full capacity across a partial run
  // and across reset (reset clears the run cursor but does NOT zero the slab), so
  // reading nChunks rows unconditionally would surface uncommitted/stale tail
  // rows. The VM truncates by step count -- it returns only saved rows mid-run
  // and libsimlin further bounds the read by the passed count -- so the wasm twin
  // must do the same to keep getSeries().length == getStepCount() at parity.
  describe('getSeries truncates to completed steps (not slab capacity)', () => {
    it('returns the VM full-run prefix after a partial runTo(t), with no stale tail', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      const vmFull = backend.simGetSeries(vm, 'teacup_temperature');

      // teacup: start 0, stop 30; stop at a strictly-interior time so saved_steps
      // is strictly less than nChunks -- the window where the bug surfaces.
      backend.simRunTo(wasm, 15);
      const partial = backend.simGetStepCount(wasm);
      expect(partial).toBeGreaterThan(0);
      expect(partial).toBeLessThan(vmFull.length);

      const wasmPartial = backend.simGetSeries(wasm, 'teacup_temperature');
      // Length tracks completed steps, not the slab capacity ...
      expect(wasmPartial.length).toBe(partial);
      // ... and the committed rows are exactly the VM full-run's prefix.
      expectSeriesClose(wasmPartial, vmFull.slice(0, partial));
      dispose();
    });

    it('returns an empty series after reset (the prior run is not surfaced as a stale tail)', () => {
      const { wasm, dispose } = openPair();
      backend.simRunToEnd(wasm);
      expect(backend.simGetSeries(wasm, 'teacup_temperature').length).toBeGreaterThan(0);

      backend.simReset(wasm);
      // saved_steps is 0 after reset even though the slab still holds the prior
      // run's rows; getSeries must agree with getStepCount and report 0 rows.
      expect(backend.simGetStepCount(wasm)).toBe(0);
      expect(backend.simGetSeries(wasm, 'teacup_temperature').length).toBe(0);
      dispose();
    });
  });

  // getStepCount reports COMPLETED steps, not the slab capacity (nChunks). A
  // fresh or just-reset wasm sim has saved no rows yet, so it must report 0 --
  // matching the documented "number of simulation steps completed" contract and
  // the VM (whose count only becomes nonzero once a run has produced Results).
  // After a full run the count equals nChunks and the VM's count (parity).
  describe('getStepCount reflects completed steps (not slab capacity)', () => {
    it('is 0 on a fresh wasm sim, equals the VM after a full run, 0 again after reset', () => {
      const { vm, wasm, dispose } = openPair();

      // Fresh: no run has happened, so no rows are saved.
      expect(backend.simGetStepCount(wasm)).toBe(0);

      // After a full run: equals nChunks (the slab capacity) and equals the VM.
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      const fullCount = backend.simGetStepCount(vm);
      expect(fullCount).toBeGreaterThan(0);
      expect(backend.simGetStepCount(wasm)).toBe(fullCount);

      // After reset (no re-run): back to 0.
      backend.simReset(wasm);
      expect(backend.simGetStepCount(wasm)).toBe(0);

      // After re-running: the completed count returns to the full count.
      backend.simRunToEnd(wasm);
      expect(backend.simGetStepCount(wasm)).toBe(fullCount);
      dispose();
    });

    it('is strictly between 0 and the full count after a partial runTo(t)', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simRunToEnd(vm);
      const fullCount = backend.simGetStepCount(vm);

      // teacup: start 0, stop 30; run to a strictly-interior time.
      backend.simRunTo(wasm, 15);
      const partial = backend.simGetStepCount(wasm);
      expect(partial).toBeGreaterThan(0);
      expect(partial).toBeLessThan(fullCount);
      dispose();
    });
  });

  describe('AC5.1/AC5.2/AC5.3: setValue (constants only) + mid-run', () => {
    it('setValue(const) then run matches the VM under the same override', () => {
      const { vm, wasm, dispose } = openPair();
      backend.simSetValue(vm, 'room temperature', 55);
      backend.simSetValue(wasm, 'room temperature', 55);
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);
      for (const name of backend.simGetVarNames(wasm)) {
        expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
      }
      dispose();
    });

    // setValue must update the live curr state, not only the override region read
    // by the next run. The VM's apply_override writes the new value into the live
    // curr chunk immediately (set_value_now, vm.rs:869-873), so getValue() reflects
    // the override before any run; the blob's set_value only writes the constants
    // region, so the wasm host must mirror the live write to keep an interactive
    // read at parity (the divergence the reviewer flagged).
    it('setValue(const) is reflected by getValue immediately, before any run (matches the VM)', () => {
      const { vm, wasm, dispose } = openPair();
      // No run has happened: a fresh sim's live curr chunk is the pre-run zero
      // state, so this read exercises the override write, not a run's output.
      backend.simSetValue(vm, 'room temperature', 55);
      backend.simSetValue(wasm, 'room temperature', 55);

      expect(backend.simGetValue(vm, 'room_temperature')).toBe(55);
      expect(backend.simGetValue(wasm, 'room_temperature')).toBe(backend.simGetValue(vm, 'room_temperature'));
      dispose();
    });

    it('setValue(nonConstant) throws, matching the VM constants-only rejection', () => {
      const { vm, wasm, dispose } = openPair();
      // heat_loss_to_room is a flow (computed), not a settable constant.
      expect(() => backend.simSetValue(vm, 'heat loss to room', 1)).toThrow();
      expect(() => backend.simSetValue(wasm, 'heat loss to room', 1)).toThrow();
      dispose();
    });

    it('setValue(unknownVariable) throws', () => {
      const { wasm, dispose } = openPair();
      expect(() => backend.simSetValue(wasm, 'definitely_not_a_var', 1)).toThrow();
      dispose();
    });

    it('mid-run setValue affects only post-t1 steps (matches VM driven identically)', () => {
      const { vm, wasm, dispose } = openPair();
      const t1 = 10;
      backend.simRunTo(vm, t1);
      backend.simRunTo(wasm, t1);
      backend.simSetValue(vm, 'room temperature', 30);
      backend.simSetValue(wasm, 'room temperature', 30);
      backend.simRunToEnd(vm);
      backend.simRunToEnd(wasm);

      // Full-series parity against the VM driven the same way.
      for (const name of backend.simGetVarNames(wasm)) {
        expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
      }
      dispose();
    });
  });

  describe('AC6.1: getLinks rejected on the wasm engine', () => {
    it('getLinks on a wasm sim throws a clear error', () => {
      const { vm, wasm, dispose } = openPair();
      // The VM path returns links (empty with LTM off); the wasm path rejects.
      expect(() => backend.simGetLinks(vm)).not.toThrow();
      expect(() => backend.simGetLinks(wasm)).toThrow(/not supported on the wasm engine/i);
      dispose();
    });
  });
});

// A statically-arrayed model the wasm backend supports. `source` is dimensioned
// over `Dim` (a STATIC dimension -- NOT a dynamic `[lo:hi]` view range, which is
// the unsupported case), and `scaled` is an arrayed aux derived from it. Its
// layout keys are per-element with the canonical base + bracketed canonical
// element name (verified empirically: `source[boston]`, `scaled[la]`, ...). This
// exercises the part of the name pipeline that scalar teacup cannot: a raw,
// mixed-case array-element name (`source[Boston]`) flowing through
// canonicalizeIdent -> wasmLayout.varOffsets lookup -> readStridedSeries.
const WASM_ARRAYED_XMILE = `<?xml version="1.0" encoding="utf-8"?>
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
            <elem name="Boston"/>
            <elem name="LA"/>
        </dim>
    </dimensions>
    <model>
        <variables>
            <aux name="source">
                <element subscript="Boston"><eqn>10</eqn></element>
                <element subscript="LA"><eqn>20</eqn></element>
                <dimensions><dim name="Dim"/></dimensions>
            </aux>
            <aux name="scaled">
                <eqn>source*2</eqn>
                <dimensions><dim name="Dim"/></dimensions>
            </aux>
        </variables>
    </model>
</xmile>`;

// End-to-end name resolution for NON-SCALAR variables (the design's "correctness
// crux"). canonicalizeIdent is proven correct in isolation, but the TS-side
// canonicalize -> varOffsets lookup -> strided read had no test for an array
// element name (a key containing `[`/`]`); scalar teacup never exercises it. The
// VM is the oracle here, driven identically to wasm and compared within TOL.
describe('DirectBackend wasm engine: end-to-end name resolution for arrayed vars', () => {
  let backend: DirectBackend;
  const TOL = 1e-9;

  beforeAll(async () => {
    backend = new DirectBackend();
    backend.reset();
    backend.configureWasm({ source: loadWasmBuffer() });
    await backend.init();
  });

  afterAll(() => {
    backend.reset();
  });

  function expectSeriesClose(actual: Float64Array, expected: Float64Array): void {
    expect(actual.length).toBe(expected.length);
    for (let i = 0; i < expected.length; i++) {
      expect(Math.abs(actual[i] - expected[i])).toBeLessThanOrEqual(TOL);
    }
  }

  function openPair(): {
    vm: SimHandle;
    wasm: SimHandle;
    dispose: () => void;
  } {
    const projectHandle = backend.projectOpenXmile(new TextEncoder().encode(WASM_ARRAYED_XMILE));
    const modelHandle = backend.projectGetModel(projectHandle, null);
    const vm = backend.simNew(modelHandle, false, 'vm');
    const wasm = backend.simNew(modelHandle, false, 'wasm');
    const dispose = () => {
      backend.simDispose(vm);
      backend.simDispose(wasm);
      backend.modelDispose(modelHandle);
      backend.projectDispose(projectHandle);
    };
    return { vm, wasm, dispose };
  }

  // Guard the precondition the rest of the suite relies on: the fixture must be
  // a wasm-SUPPORTED model. If a future engine change made a static array
  // unsupported, this fails loudly here rather than masquerading as a parity bug.
  it('the static-arrayed fixture compiles to wasm without throwing', () => {
    const projectHandle = backend.projectOpenXmile(new TextEncoder().encode(WASM_ARRAYED_XMILE));
    const modelHandle = backend.projectGetModel(projectHandle, null);
    let wasm: SimHandle | undefined;
    expect(() => {
      wasm = backend.simNew(modelHandle, false, 'wasm');
    }).not.toThrow();
    if (wasm !== undefined) {
      backend.simDispose(wasm);
    }
    backend.modelDispose(modelHandle);
    backend.projectDispose(projectHandle);
  });

  it('getVarNames (wasm) exposes the per-element bracketed keys and equals the VM', () => {
    const { vm, wasm, dispose } = openPair();
    backend.simRunToEnd(vm);
    backend.simRunToEnd(wasm);

    const names = backend.simGetVarNames(wasm);
    expect(names).toEqual(backend.simGetVarNames(vm));
    // The arrayed vars appear as canonical per-element keys (base + bracketed,
    // lowercased element name), not as a bare scalar base name.
    expect(names).toContain('source[boston]');
    expect(names).toContain('source[la]');
    expect(names).toContain('scaled[boston]');
    expect(names).toContain('scaled[la]');
    dispose();
  });

  it('getSeries resolves a raw mixed-case array-element name to the VM series', () => {
    const { vm, wasm, dispose } = openPair();
    backend.simRunToEnd(vm);
    backend.simRunToEnd(wasm);

    // Each name is passed RAW (mixed case, original element casing) so the read
    // path must canonicalize it (`source[Boston]` -> `source[boston]`) before the
    // varOffsets lookup -- the exact integration scalar teacup cannot cover.
    for (const rawName of ['source[Boston]', 'source[LA]', 'scaled[Boston]', 'scaled[LA]']) {
      expectSeriesClose(backend.simGetSeries(wasm, rawName), backend.simGetSeries(vm, rawName));
    }
    dispose();
  });

  it('getSeries (wasm) equals the VM for every variable in the arrayed layout', () => {
    const { vm, wasm, dispose } = openPair();
    backend.simRunToEnd(vm);
    backend.simRunToEnd(wasm);
    const names = backend.simGetVarNames(wasm);
    expect(names.length).toBeGreaterThan(0);
    for (const name of names) {
      expectSeriesClose(backend.simGetSeries(wasm, name), backend.simGetSeries(vm, name));
    }
    dispose();
  });
});
