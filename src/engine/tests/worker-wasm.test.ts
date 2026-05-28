// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// Phase 3 of the @simlin/engine wasm backend: drive engine:'wasm' end-to-end
// through the real postMessage protocol (request/response serialization, the
// FIFO queue, handleResponse, deserializeError) using the in-memory loopback
// that wires a real WorkerBackend to a real WorkerServer (which wraps a
// DirectBackend). A node DirectBackend is the oracle: the worker-driven wasm
// series must equal the DirectBackend wasm series exactly, and the VM series
// within the engine's parity tolerance. There is no real Worker/jsdom here;
// testEnvironment is node.

import { readFileSync } from 'fs';
import { join } from 'path';

import { WorkerBackend } from '../src/worker-backend';
import { WorkerServer } from '../src/worker-server';
import { DirectBackend } from '../src/direct-backend';
import type { WorkerRequest, WorkerResponse } from '../src/worker-protocol';
import type { ModelHandle } from '../src/backend';
import { expectScoresClose, linksByKey } from './ltm-test-helpers';

const wasmPath = join(__dirname, '..', 'core', 'libsimlin.wasm');

function loadTeacupXmile(): Uint8Array {
  const xmilePath = join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  return readFileSync(xmilePath);
}

// Scalar LTM fixture (one stock + one flow + three auxes) committed in-tree at
// test/logistic_growth_ltm/. The wasm backend supports every equation in this
// model, so the wasm compile with enableLtm must succeed; the LTM analysis
// surfaces nontrivial per-link scores against a known feedback structure. Same
// fixture wasm-ltm.test.ts uses, so the worker leg here exercises the identical
// model the DirectBackend parity test pins.
function loadLogisticGrowthLtmXmile(): Uint8Array {
  const xmilePath = join(__dirname, '..', '..', '..', 'test', 'logistic_growth_ltm', 'logistic_growth.stmx');
  return readFileSync(xmilePath);
}

function loadWasmSource(): Uint8Array {
  return readFileSync(wasmPath);
}

// A model the wasm backend cannot compile: `summed = SUM(source[lo:hi])` uses a
// runtime view range `[lo:hi]` whose bounds reference scalar auxes (not
// constants and not dimension elements), so the range cannot be constant-folded
// and codegen emits the ViewRangeDynamic opcode, which wasmgen reports as
// Unsupported (GH #612). The VM runs the same model fine. This is the same
// fixture the Phase 2 DirectBackend tests used to prove there is no silent VM
// fallback; here it must reject across the worker boundary instead.
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

// Tolerance for the worker-wasm-vs-VM comparison, matching the Phase 2
// DirectBackend parity tests (wasm-backend.test.ts). The wasm blob mirrors the
// VM opcode-for-opcode, but wasm is not bit-identical to the VM's native libm
// by design, so transcendental-heavy variables can differ by reassociation
// noise within this bound. Worker-vs-DirectBackend on the same engine ('wasm')
// is the same compiled simulation, so that comparison is exact.
const TOL = 1e-9;

interface WorkerWasmPair {
  backend: WorkerBackend;
  server: WorkerServer;
  /** Requests delivered to the server, in order (the backend -> server leg). */
  requests: WorkerRequest[];
  /**
   * Transfer lists the server attached to its responses (the server -> backend
   * leg). The zero-copy Float64Array from getSeries travels on this leg, so
   * this is where the transfer assertion must look (the request leg carries no
   * transfer for getSeries).
   */
  responseTransfers: (Transferable[] | undefined)[];
}

// Wire a real WorkerBackend to a real WorkerServer via fake transport closures,
// mirroring createTestPair in worker-backend.test.ts but additionally recording
// the served requests and the server's response-side transfer lists (the leg
// the zero-copy getSeries buffer rides on, mirroring worker-server.test.ts's
// safe-buffer-transfer harness).
function createWorkerWasmPair(): WorkerWasmPair {
  let backendOnMessage: ((msg: WorkerResponse) => void) | null = null;
  const requests: WorkerRequest[] = [];
  const responseTransfers: (Transferable[] | undefined)[] = [];

  const server = new WorkerServer((msg: WorkerResponse, transfer?: Transferable[]) => {
    responseTransfers.push(transfer);
    if (backendOnMessage) {
      setTimeout(() => backendOnMessage!(msg), 0);
    }
  });

  const backend = new WorkerBackend(
    (msg: WorkerRequest) => {
      requests.push(msg);
      setTimeout(() => server.handleMessage(msg), 0);
    },
    (callback: (msg: WorkerResponse) => void) => {
      backendOnMessage = callback;
    },
  );

  return { backend, server, requests, responseTransfers };
}

describe('WorkerBackend wasm engine parity (Phase 3)', () => {
  // A fresh DirectBackend oracle per suite. The worker pair owns its own
  // DirectBackend inside the WorkerServer; both load the same wasm blob.
  let oracle: DirectBackend;

  beforeAll(async () => {
    oracle = new DirectBackend();
    oracle.reset();
    oracle.configureWasm({ source: loadWasmSource() });
    await oracle.init();
  });

  afterAll(() => {
    oracle.reset();
  });

  function expectSeriesClose(actual: Float64Array, expected: Float64Array): void {
    expect(actual.length).toBe(expected.length);
    for (let i = 0; i < expected.length; i++) {
      expect(Math.abs(actual[i] - expected[i])).toBeLessThanOrEqual(TOL);
    }
  }

  describe('AC8.1: worker wasm series match the node DirectBackend (and the VM)', () => {
    it('teacup_temperature via the worker wasm path equals DirectBackend wasm exactly and the VM within tolerance', async () => {
      const { backend } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = await backend.projectGetModel(projHandle, null);
      const simHandle = await backend.simNew(modelHandle, false, 'wasm');
      await backend.simRunToEnd(simHandle);
      const workerSeries = await backend.simGetSeries(simHandle, 'teacup_temperature');

      // Oracle: the same model, on the node DirectBackend, on both engines.
      const oracleProject = oracle.projectOpenXmile(loadTeacupXmile());
      const oracleModel = oracle.projectGetModel(oracleProject, null);
      const wasmSim = oracle.simNew(oracleModel, false, 'wasm');
      const vmSim = oracle.simNew(oracleModel, false, 'vm');
      oracle.simRunToEnd(wasmSim);
      oracle.simRunToEnd(vmSim);
      const directWasmSeries = oracle.simGetSeries(wasmSim, 'teacup_temperature');
      const directVmSeries = oracle.simGetSeries(vmSim, 'teacup_temperature');

      // Worker wasm vs DirectBackend wasm: same compiled simulation -> exact.
      expect(workerSeries).toBeInstanceOf(Float64Array);
      expect(Array.from(workerSeries)).toEqual(Array.from(directWasmSeries));
      // Worker wasm vs the VM oracle: within the engine's parity tolerance.
      expectSeriesClose(workerSeries, directVmSeries);

      oracle.simDispose(wasmSim);
      oracle.simDispose(vmSim);
      oracle.modelDispose(oracleModel);
      oracle.projectDispose(oracleProject);
    });

    it('every variable via the worker wasm path matches DirectBackend wasm exactly and the VM within tolerance', async () => {
      const { backend } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = await backend.projectGetModel(projHandle, null);
      const simHandle = await backend.simNew(modelHandle, false, 'wasm');
      await backend.simRunToEnd(simHandle);

      const oracleProject = oracle.projectOpenXmile(loadTeacupXmile());
      const oracleModel = oracle.projectGetModel(oracleProject, null);
      const wasmSim = oracle.simNew(oracleModel, false, 'wasm');
      const vmSim = oracle.simNew(oracleModel, false, 'vm');
      oracle.simRunToEnd(wasmSim);
      oracle.simRunToEnd(vmSim);

      const names = await backend.simGetVarNames(simHandle);
      expect(names.length).toBeGreaterThan(0);
      for (const name of names) {
        const workerSeries = await backend.simGetSeries(simHandle, name);
        // Exact against the DirectBackend wasm engine.
        expect(Array.from(workerSeries)).toEqual(Array.from(oracle.simGetSeries(wasmSim, name)));
        // Within tolerance against the VM oracle.
        expectSeriesClose(workerSeries, oracle.simGetSeries(vmSim, name));
      }

      oracle.simDispose(wasmSim);
      oracle.simDispose(vmSim);
      oracle.modelDispose(oracleModel);
      oracle.projectDispose(oracleProject);
    });
  });

  describe('AC8.2: minimal additive protocol + zero-copy getSeries for the wasm engine', () => {
    it('getSeries round-trips a Float64Array and adds exactly one one-element transfer on the response leg', async () => {
      const { backend, responseTransfers } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = await backend.projectGetModel(projHandle, null);
      const simHandle = await backend.simNew(modelHandle, false, 'wasm');
      await backend.simRunToEnd(simHandle);

      // Measure the transfer delta around the single getSeries call: the
      // cumulative responseTransfers list accumulates across every prior op, so
      // assert on what this one call appends, not on the absolute length.
      const before = responseTransfers.length;
      const series = await backend.simGetSeries(simHandle, 'teacup_temperature');
      const appended = responseTransfers.slice(before);

      expect(series).toBeInstanceOf(Float64Array);
      expect(series.length).toBeGreaterThan(0);
      // Exactly one response carried a transfer for this call, and it was the
      // single Float64Array buffer (zero-copy), and the view owns its buffer.
      const withTransfer = appended.filter((t): t is Transferable[] => t !== undefined && t.length > 0);
      expect(withTransfer.length).toBe(1);
      expect(withTransfer[0].length).toBe(1);
      expect(series.byteOffset).toBe(0);
      expect(series.buffer.byteLength).toBe(series.byteLength);
    });

    it('the served simNew request carries engine:wasm and introduces no new message type', async () => {
      const { backend, requests } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = await backend.projectGetModel(projHandle, null);
      await backend.simNew(modelHandle, false, 'wasm');

      const simNewRequests = requests.filter((r) => r.type === 'simNew');
      expect(simNewRequests.length).toBe(1);
      const simNew = simNewRequests[0] as Extract<WorkerRequest, { type: 'simNew' }>;
      expect(simNew.engine).toBe('wasm');
      expect(simNew.enableLtm).toBe(false);

      // The protocol delta is purely additive: only the pre-existing request
      // type strings appear on the wire (no new discriminant was introduced).
      const KNOWN_TYPES = new Set([
        'init',
        'isInitialized',
        'reset',
        'configureWasm',
        'projectOpenXmile',
        'projectOpenProtobuf',
        'projectOpenJson',
        'projectOpenVensim',
        'projectDispose',
        'projectGetModelCount',
        'projectGetModelNames',
        'projectGetModel',
        'projectIsSimulatable',
        'projectSerializeProtobuf',
        'projectSerializeJson',
        'projectSerializeXmile',
        'projectRenderSvg',
        'projectRenderPng',
        'projectGetErrors',
        'projectApplyPatch',
        'modelGetName',
        'modelDispose',
        'modelGetIncomingLinks',
        'modelGetLinks',
        'modelGetLoops',
        'modelGetLatexEquation',
        'modelGetVarJson',
        'modelGetVarNames',
        'modelGetSimSpecsJson',
        'simNew',
        'simDispose',
        'simRunTo',
        'simRunToEnd',
        'simReset',
        'simGetTime',
        'simGetStepCount',
        'simGetValue',
        'simSetValue',
        'simGetSeries',
        'simGetVarNames',
        'simGetLinks',
      ]);
      for (const req of requests) {
        expect(KNOWN_TYPES.has(req.type)).toBe(true);
      }
    });

    it('the VM path still omits the engine field (additive: undefined engine -> absent field)', async () => {
      const { backend, requests } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(loadTeacupXmile());
      const modelHandle = await backend.projectGetModel(projHandle, null);
      // No engine argument: the message must serialize engine as undefined,
      // i.e. structurally identical to the pre-Phase-3 simNew message.
      await backend.simNew(modelHandle, false);

      const simNew = requests.find((r) => r.type === 'simNew') as Extract<WorkerRequest, { type: 'simNew' }>;
      expect(simNew.engine).toBeUndefined();
    });
  });

  describe('worker-boundary error propagation (no silent VM fallback)', () => {
    it('rejects a wasm-unsupported model rather than silently switching to the VM', async () => {
      const { backend } = createWorkerWasmPair();
      await backend.init(loadWasmSource());
      const projHandle = await backend.projectOpenXmile(new TextEncoder().encode(WASM_UNSUPPORTED_XMILE));
      const modelHandle = await backend.projectGetModel(projHandle, null);

      // wasm codegen reports the dynamic view range as Unsupported; that error
      // must surface across the worker boundary, NOT be swallowed by a VM run.
      await expect(backend.simNew(modelHandle, false, 'wasm')).rejects.toThrow();

      // Prove no silent fallback happened: the same model runs fine on the VM
      // engine through the same worker, so the wasm rejection was specific to
      // the wasm path (not a malformed model that would also break the VM).
      const vmSim = await backend.simNew(modelHandle, false, 'vm');
      await backend.simRunToEnd(vmSim);
      const series = await backend.simGetSeries(vmSim, 'summed');
      expect(series).toBeInstanceOf(Float64Array);
      // SUM(source[1:3]) = 1 + 2 + 3 = 6 at every step.
      expect(series[0]).toBeCloseTo(6, 9);
    });

    it('the wasm-unsupported model also rejects on the node DirectBackend (oracle agreement)', () => {
      // Pin the no-fallback contract at the oracle layer too: the worker
      // rejection above mirrors the DirectBackend, not worker-only behavior.
      const oracleProject = oracle.projectOpenXmile(new TextEncoder().encode(WASM_UNSUPPORTED_XMILE));
      const oracleModel: ModelHandle = oracle.projectGetModel(oracleProject, null);
      expect(() => oracle.simNew(oracleModel, false, 'wasm')).toThrow();
      oracle.modelDispose(oracleModel);
      oracle.projectDispose(oracleProject);
    });
  });
});

// AC1.4: LTM on the wasm engine survives the worker boundary. The Phase 3
// DirectBackend change (cc0abdd) enabled enableLtm + engine:'wasm' end-to-end;
// because WorkerServer wraps a DirectBackend and simGetLinks is already in the
// worker protocol (the VM path used it), no protocol change is needed -- this
// test pins that promise by exercising the full WorkerBackend round-trip and
// comparing against the same model on node DirectBackend (exact) and on the VM
// (within the engine's parity tolerance). The shared helpers (expectScoresClose,
// linkKey, linksByKey) are imported from ltm-test-helpers.ts, which also
// documents the 1e-6 tolerance rationale.
describe('WorkerBackend LTM on the wasm engine (Phase 6)', () => {
  let oracle: DirectBackend;

  beforeAll(async () => {
    oracle = new DirectBackend();
    oracle.reset();
    oracle.configureWasm({ source: loadWasmSource() });
    await oracle.init();
  });

  afterAll(() => {
    oracle.reset();
  });

  // AC1.4: identical link set, identical polarities, identical per-step scores
  // against the node DirectBackend (the same compiled blob and the same
  // analytic core, so exact); also within 1e-6 of the VM run (the parity
  // oracle) for a three-way pin.
  it('worker wasm getLinks matches node + VM', async () => {
    const { backend } = createWorkerWasmPair();
    await backend.init(loadWasmSource());
    const projHandle = await backend.projectOpenXmile(loadLogisticGrowthLtmXmile());
    const modelHandle = await backend.projectGetModel(projHandle, null);
    const simHandle = await backend.simNew(modelHandle, true, 'wasm');
    await backend.simRunToEnd(simHandle);
    const workerLinks = await backend.simGetLinks(simHandle);

    const oracleProject = oracle.projectOpenXmile(loadLogisticGrowthLtmXmile());
    const oracleModel = oracle.projectGetModel(oracleProject, null);
    const nodeWasmSim = oracle.simNew(oracleModel, true, 'wasm');
    const vmSim = oracle.simNew(oracleModel, true, 'vm');
    oracle.simRunToEnd(nodeWasmSim);
    oracle.simRunToEnd(vmSim);
    const nodeLinks = oracle.simGetLinks(nodeWasmSim);
    const vmLinks = oracle.simGetLinks(vmSim);

    // The LTM analysis genuinely fired on this feedback model: at least one
    // link carries a per-step score series, otherwise the comparison would be
    // vacuous.
    expect(workerLinks.length).toBeGreaterThan(0);
    expect(workerLinks.some((l) => l.score !== undefined)).toBe(true);

    const workerByKey = linksByKey(workerLinks);
    const nodeByKey = linksByKey(nodeLinks);
    const vmByKey = linksByKey(vmLinks);

    // The (from, to) edge set is identical across all three -- analyses agree
    // on causal structure regardless of which backend produced the series.
    expect([...workerByKey.keys()].sort()).toEqual([...nodeByKey.keys()].sort());
    expect([...workerByKey.keys()].sort()).toEqual([...vmByKey.keys()].sort());

    for (const [key, nodeLink] of nodeByKey) {
      const workerLink = workerByKey.get(key);
      const vmLink = vmByKey.get(key);
      expect(workerLink).toBeDefined();
      expect(vmLink).toBeDefined();
      // Worker -> node: same compiled wasm blob, same analytic core, so the
      // polarities and per-step scores must agree byte-for-byte.
      expect(workerLink!.polarity).toBe(nodeLink.polarity);
      if (nodeLink.score === undefined) {
        expect(workerLink!.score).toBeUndefined();
      } else {
        expect(workerLink!.score).toBeDefined();
        expect(Array.from(workerLink!.score!)).toEqual(Array.from(nodeLink.score));
      }
      // Worker -> VM: same model, different evaluators; polarities are still
      // exact, scores agree within the documented LTM tolerance.
      expect(workerLink!.polarity).toBe(vmLink!.polarity);
      if (vmLink!.score === undefined) {
        expect(workerLink!.score).toBeUndefined();
      } else {
        expect(workerLink!.score).toBeDefined();
        expectScoresClose(workerLink!.score!, vmLink!.score);
      }
    }

    oracle.simDispose(nodeWasmSim);
    oracle.simDispose(vmSim);
    oracle.modelDispose(oracleModel);
    oracle.projectDispose(oracleProject);
  });
});
