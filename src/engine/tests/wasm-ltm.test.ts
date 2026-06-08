// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// Public-API parity tests for LTM analysis on the wasm engine. The bytecode
// VM is the correctness oracle: a wasm-engine LTM run is compared against the
// VM-engine result for the same model within a tight tolerance, both at the
// Sim.getLinks() level and at the Run.links facade. The logistic_growth_ltm
// fixture is a small, well-known feedback model (one stock, one flow, three
// auxes) where the LTM analysis surfaces nontrivial per-link scores.

import * as fs from 'fs';
import * as path from 'path';

import { Project, Sim, Run, configureWasm, ready, resetWasm } from '../src';
import { expectScoresClose, linksByKey } from './ltm-test-helpers';

async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

// A small, scalar feedback model with an LTM fixture committed in-tree.
// Variables: stock `population`, flow `net birth rate`, auxes
// `maximum growth rate`, `carrying capacity`, `fractional growth rate`,
// `fraction of carrying capacity used`. The wasm backend supports every
// equation in this model, so the wasm compile must succeed.
function loadLogisticGrowthLtmXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', '..', 'test', 'logistic_growth_ltm', 'logistic_growth.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('LTM on the wasm engine (public API)', () => {
  let project: Project;

  beforeAll(async () => {
    await loadWasm();
    project = await Project.open(loadLogisticGrowthLtmXmile());
  });

  afterAll(async () => {
    await project.dispose();
  });

  it('simulate({engine:wasm, enableLtm}) resolves to a Sim', async () => {
    const model = await project.mainModel();
    const sim = await model.simulate({}, { engine: 'wasm', enableLtm: true });
    expect(sim).toBeInstanceOf(Sim);
    expect(sim.ltmEnabled).toBe(true);
    await sim.dispose();
  });

  it('wasm getLinks returns scored links', async () => {
    const model = await project.mainModel();
    const sim = await model.simulate({}, { engine: 'wasm', enableLtm: true });
    await sim.runToEnd();

    const links = await sim.getLinks();
    expect(links.length).toBeGreaterThan(0);

    // The fixture has a feedback structure, so the analysis must produce at
    // least one link with a defined per-step score whose length matches the
    // completed-step count. (Self-loops carry no score, so we look for any
    // link that did receive one rather than asserting all links score.)
    const stepCount = await sim.getStepCount();
    expect(stepCount).toBeGreaterThan(0);
    const scored = links.filter((l) => l.score !== undefined);
    expect(scored.length).toBeGreaterThan(0);
    for (const link of scored) {
      expect(link.score).toBeInstanceOf(Float64Array);
      expect(link.score!.length).toBe(stepCount);
    }

    await sim.dispose();
  });

  it('wasm getLinks scores match VM', async () => {
    const model = await project.mainModel();
    const vmSim = await model.simulate({}, { engine: 'vm', enableLtm: true });
    const wasmSim = await model.simulate({}, { engine: 'wasm', enableLtm: true });
    await vmSim.runToEnd();
    await wasmSim.runToEnd();

    const vmLinks = await vmSim.getLinks();
    const wasmLinks = await wasmSim.getLinks();

    // Same set of (from,to) edges with the same polarities on both engines.
    const vmByKey = linksByKey(vmLinks);
    const wasmByKey = linksByKey(wasmLinks);
    expect(wasmByKey.size).toBe(vmByKey.size);
    expect([...wasmByKey.keys()].sort()).toEqual([...vmByKey.keys()].sort());

    // Every per-edge score matches within tolerance; polarities are exact.
    // The relative score (GH #652) is produced by the same shared analytic
    // core, so it must match column-for-column too, be present exactly when
    // the raw score is, and stay bounded in [-1, 1].
    let relScored = 0;
    for (const [key, vmLink] of vmByKey) {
      const wasmLink = wasmByKey.get(key);
      expect(wasmLink).toBeDefined();
      expect(wasmLink!.polarity).toBe(vmLink.polarity);
      if (vmLink.score === undefined) {
        expect(wasmLink!.score).toBeUndefined();
        expect(vmLink.relativeScore).toBeUndefined();
        expect(wasmLink!.relativeScore).toBeUndefined();
      } else {
        expect(wasmLink!.score).toBeDefined();
        expectScoresClose(wasmLink!.score!, vmLink.score);
        // A scored link always carries a relative series of the same length.
        expect(vmLink.relativeScore).toBeDefined();
        expect(wasmLink!.relativeScore).toBeDefined();
        expect(vmLink.relativeScore!.length).toBe(vmLink.score.length);
        expectScoresClose(wasmLink!.relativeScore!, vmLink.relativeScore!);
        for (const v of vmLink.relativeScore!) {
          if (Number.isFinite(v)) {
            expect(Math.abs(v)).toBeLessThanOrEqual(1 + 1e-9);
          }
        }
        relScored++;
      }
    }
    expect(relScored).toBeGreaterThan(0);

    await vmSim.dispose();
    await wasmSim.dispose();
  });

  // The wasm blob's results region is allocated for the full nChunks capacity,
  // but G_SAVED records how many rows the sim has actually written.  Mid-run
  // (via runTo) getLinks() must marshal only the saved rows, not the entire
  // capacity -- otherwise the from-wasm analyzer would see uninit/stale tail
  // rows and getLinks()/Run.links would diverge from getSeries(), which
  // already truncates to saved_steps.  This test pins that contract: a
  // partial run produces per-link score arrays whose length equals
  // getStepCount(), and whose values match the first getStepCount() elements
  // of the VM oracle's full run.
  it('wasm getLinks after partial run matches VM prefix (no stale tail)', async () => {
    const model = await project.mainModel();
    const wasmSim = await model.simulate({}, { engine: 'wasm', enableLtm: true });
    // logistic_growth.stmx is t in [0, 100] dt=1 -> 101 saved samples.
    // Halfway through the run exercises the saved_steps < nChunks path.
    await wasmSim.runTo(50);
    const savedSteps = await wasmSim.getStepCount();
    expect(savedSteps).toBeGreaterThan(0);
    expect(savedSteps).toBeLessThan(101);

    const wasmLinks = await wasmSim.getLinks();
    const scored = wasmLinks.filter((l) => l.score !== undefined);
    expect(scored.length).toBeGreaterThan(0);
    for (const link of scored) {
      // The score array length is bounded by saved_steps, not nChunks;
      // a regression to the full-capacity slab would surface as
      // link.score!.length === 101 here.
      expect(link.score!.length).toBe(savedSteps);
      // No stale tail: every value is a finite f64 produced by the blob,
      // not uninit garbage.
      for (let i = 0; i < link.score!.length; i++) {
        expect(Number.isFinite(link.score![i])).toBe(true);
      }
    }

    // The partial-run wasm scores must match the first savedSteps elements
    // of the VM oracle's full-run scores: same model, same inputs, same
    // analytic core -- the difference is purely how many rows are passed
    // through the from-wasm FFI.  This is the strongest assertion against
    // stale-tail data because uninit slab bytes would produce arbitrary
    // values that the VM oracle does not.
    const vmSim = await model.simulate({}, { engine: 'vm', enableLtm: true });
    await vmSim.runToEnd();
    const vmLinks = await vmSim.getLinks();

    const vmByKey = linksByKey(vmLinks);
    const wasmByKey = linksByKey(wasmLinks);
    for (const [key, vmLink] of vmByKey) {
      const wasmLink = wasmByKey.get(key);
      expect(wasmLink).toBeDefined();
      if (vmLink.score === undefined) {
        expect(wasmLink!.score).toBeUndefined();
      } else {
        expect(wasmLink!.score).toBeDefined();
        expect(wasmLink!.score!.length).toBe(savedSteps);
        // VM score is the full-run series; take its first savedSteps to
        // compare against the partial-run wasm series.
        expectScoresClose(wasmLink!.score!, vmLink.score.subarray(0, savedSteps));
      }
    }

    await wasmSim.dispose();
    await vmSim.dispose();
  });

  it('Run.links populated for wasm LTM run', async () => {
    const model = await project.mainModel();
    const vmRun = await model.run({}, { analyzeLtm: true, engine: 'vm' });
    const wasmRun = await model.run({}, { analyzeLtm: true, engine: 'wasm' });

    expect(wasmRun).toBeInstanceOf(Run);
    expect(wasmRun.links.length).toBeGreaterThan(0);

    // The Run-level link set, polarities, and scores match the VM run.
    const vmByKey = linksByKey(vmRun.links);
    const wasmByKey = linksByKey(wasmRun.links);
    expect(wasmByKey.size).toBe(vmByKey.size);
    expect([...wasmByKey.keys()].sort()).toEqual([...vmByKey.keys()].sort());

    for (const [key, vmLink] of vmByKey) {
      const wasmLink = wasmByKey.get(key);
      expect(wasmLink).toBeDefined();
      expect(wasmLink!.polarity).toBe(vmLink.polarity);
      if (vmLink.score === undefined) {
        expect(wasmLink!.score).toBeUndefined();
      } else {
        expect(wasmLink!.score).toBeDefined();
        expectScoresClose(wasmLink!.score!, vmLink.score);
      }
    }
  });
});

// AC3.2 (TS side): an LTM model the wasm backend cannot compile rejects from
// Model.simulate({engine:'wasm', enableLtm:true}) as a thrown Error -- the
// DirectBackend deliberately has no VM fallback (see src/engine/CLAUDE.md).
// The fixture in test/ltm_dynamic_range_unsupported/ is the same XMILE the
// engine/FFI Rust tests use (single source of truth), combining a feedback
// loop (so LTM is genuinely enabled) with SUM(source[lo:hi]) which the
// fully-unrolled wasm emitter cannot lower (GH #612). The VM still handles
// the dynamic-range subscript, so the same model still simulates on
// engine:'vm' -- confirming this is a wasm-only limitation.
function loadDynamicRangeUnsupportedXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', '..', 'test', 'ltm_dynamic_range_unsupported', 'model.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('LTM on the wasm engine: unsupported model', () => {
  let project: Project;

  beforeAll(async () => {
    await loadWasm();
    project = await Project.open(loadDynamicRangeUnsupportedXmile());
  });

  afterAll(async () => {
    await project.dispose();
  });

  it('unsupported LTM model rejects on wasm but runs on vm', async () => {
    const model = await project.mainModel();

    // The wasm path rejects: the compile FFI returns a SimlinError that the
    // DirectBackend surfaces as a thrown Error (no silent VM fallback, no
    // silently-wrong result slab). We do not pin the exact error text -- the
    // message is allowed to evolve -- only that simulate rejects.
    await expect(model.simulate({}, { engine: 'wasm', enableLtm: true })).rejects.toThrow();

    // The same model simulates fine on the VM with LTM enabled: confirming
    // the limitation is wasm-backend-specific, not a structural model error.
    const vmSim = await model.simulate({}, { engine: 'vm', enableLtm: true });
    await vmSim.runToEnd();
    const stepCount = await vmSim.getStepCount();
    expect(stepCount).toBeGreaterThan(0);

    // The VM analyzer must have produced at least one causal link on the
    // model (the feedback loop in the fixture); this is the secondary AC3.2
    // sanity check that LTM is genuinely on.
    const links = await vmSim.getLinks();
    expect(links.length).toBeGreaterThan(0);

    await vmSim.dispose();
  });
});
