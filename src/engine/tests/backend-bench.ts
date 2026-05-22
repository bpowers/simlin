// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// The node VM-vs-wasm eval benchmark runner. It loads fishbanks / WORLD3 /
// C-LEARN, drives them through the public @simlin/engine API
// (Project.open* -> Model.simulate({engine}) -> Sim.reset/runToEnd), and times
// only the simulation (eval) region -- the blob compile/instantiate happens
// once in untimed setup, mirroring backend_bench.rs's eval-vs-eval methodology.
// The pure statistic + warmup/measure policy lives in the always-on-tested
// bench-stats.ts (Functional Core); this shell only does I/O + orchestration.
// Per the "no stale benchmark data" rule it prints results to the console and
// never writes a results file.

import * as fs from 'fs';
import * as path from 'path';
import { performance } from 'node:perf_hooks';

import { Project } from '../src/project';
import { Model } from '../src/model';
import { runTimedAsync, seriesClose, type BenchOpts, type Stat } from './bench-stats';

// Which execution backend a Sim runs on. Mirrors SimEngine in src/backend.ts;
// the public Model.simulate accepts this union directly, so the benchmark needs
// no engine import.
type Engine = 'vm' | 'wasm';

type ModelKey = 'fishbanks' | 'wrld3' | 'clearn';

type ModelSpec = {
  readonly label: string;
  readonly path: string;
  readonly open: (data: Uint8Array) => Promise<Project>;
};

// Repo root is three levels up from src/engine/tests/. Paths and formats mirror
// backend_bench.rs's MODELS table: fishbanks is XMILE; WORLD3 and C-LEARN are
// Vensim .mdl (the C-LEARN .xmile is a header-only stub -- the .mdl is the real
// model).
const MODELS: Record<ModelKey, ModelSpec> = {
  fishbanks: {
    label: 'fishbanks',
    path: path.join(__dirname, '..', '..', '..', 'default_projects', 'fishbanks', 'model.xmile'),
    open: (data) => Project.open(data),
  },
  wrld3: {
    label: 'WORLD3-03',
    path: path.join(__dirname, '..', '..', '..', 'test', 'metasd', 'WRLD3-03', 'wrld3-03.mdl'),
    open: (data) => Project.openVensim(data),
  },
  clearn: {
    label: 'C-LEARN v77',
    path: path.join(__dirname, '..', '..', '..', 'test', 'xmutil_test_models', 'C-LEARN v77 for Vensim.mdl'),
    open: (data) => Project.openVensim(data),
  },
};

const MODEL_ORDER: ReadonlyArray<ModelKey> = ['fishbanks', 'wrld3', 'clearn'];

export type BenchRow = {
  readonly model: string;
  readonly vm: Stat;
  readonly wasm: Stat;
  readonly ratio: number;
};

export type BenchTable = ReadonlyArray<BenchRow>;

/** Which models to run, drawn from {fishbanks, wrld3, clearn}; default is all
 *  three. A BENCH_MODELS comma-list env narrows the set (mirroring
 *  backend_bench.rs). */
function selectedModels(): ReadonlyArray<ModelKey> {
  const env = process.env.BENCH_MODELS;
  if (!env) {
    return MODEL_ORDER;
  }
  const requested = new Set(
    env
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s.length > 0),
  );
  return MODEL_ORDER.filter((key) => requested.has(key));
}

/**
 * Median eval (simulation) time for `model` on one engine.
 *
 * The Sim is created once in untimed setup -- for wasm this compiles and
 * instantiates the blob, for vm it creates the libsimlin sim. Each measured
 * iteration calls reset() (setup: re-arms the run cursor; for wasm the resumable
 * reset re-runs without recompiling) BEFORE the clock is sampled, then times
 * only runToEnd(). Result extraction (getRun/getSeries) is never timed.
 */
async function timeEngine(model: Model, engine: Engine, opts: Readonly<BenchOpts>): Promise<Stat> {
  const sim = await model.simulate({}, { engine });
  try {
    return await runTimedAsync(opts, async () => {
      await sim.reset();
      const t0 = performance.now();
      await sim.runToEnd();
      return performance.now() - t0;
    });
  } finally {
    await sim.dispose();
  }
}

/**
 * Confirm the VM and wasm engines compute the same simulation, so the timings
 * describe a real, correct run (mirrors backend_bench.rs's cross_check). Runs
 * both engines to completion outside any timing and compares every variable's
 * full series within the engine's VM-vs-wasm parity tolerance (the pure
 * `seriesClose` predicate). Throws on the first mismatch -- a divergence beyond
 * that tolerance is a real parity bug, not something to benchmark over.
 */
async function crossCheck(model: Model, label: string): Promise<void> {
  const vmSim = await model.simulate({}, { engine: 'vm' });
  const wasmSim = await model.simulate({}, { engine: 'wasm' });
  try {
    await vmSim.runToEnd();
    await wasmSim.runToEnd();

    const names = await wasmSim.getVarNames();
    if (names.length === 0) {
      throw new Error(`cross-check failed for ${label}: model produced no variables`);
    }

    for (const name of names) {
      const vmSeries = await vmSim.getSeries(name);
      const wasmSeries = await wasmSim.getSeries(name);
      const result = seriesClose(vmSeries, wasmSeries);
      if (!result.match) {
        if (result.index < 0) {
          throw new Error(
            `cross-check failed for ${label}: series length differs for '${name}' ` +
              `(vm ${result.expected} vs wasm ${result.actual})`,
          );
        }
        throw new Error(
          `cross-check failed for ${label}: '${name}' diverges at step ${result.index} ` +
            `(vm ${result.expected} vs wasm ${result.actual})`,
        );
      }
    }
  } finally {
    await vmSim.dispose();
    await wasmSim.dispose();
  }
}

function fmtMs(v: number): string {
  if (!Number.isFinite(v)) {
    return '-';
  }
  if (v >= 100) {
    return v.toFixed(1);
  }
  if (v >= 1) {
    return v.toFixed(3);
  }
  return v.toFixed(4);
}

function fmtRatio(ratio: number): string {
  return Number.isFinite(ratio) ? `${ratio.toFixed(2)}x` : '-';
}

function printSummary(rows: BenchTable): void {
  const lines: Array<string> = [];
  lines.push('');
  lines.push('### Node VM-vs-wasm eval benchmark (median ms)');
  lines.push('');
  lines.push('| model | VM eval (median ms) | wasm eval (median ms) | wasm/VM | iters |');
  lines.push('|---|--:|--:|--:|--:|');
  for (const r of rows) {
    lines.push(
      `| ${r.model} | ${fmtMs(r.vm.medianMs)} | ${fmtMs(r.wasm.medianMs)} | ${fmtRatio(r.ratio)} ` +
        `| vm ${r.vm.iters} / wasm ${r.wasm.iters} |`,
    );
  }
  lines.push('');
  lines.push(
    '_Eval-only: blob compile/instantiate and result extraction are excluded. ' +
      'Absolute numbers include the async public-API overhead (per-call await), ' +
      'so the wasm/VM ratio is the meaningful figure._',
  );
  console.log(lines.join('\n'));
}

/**
 * Run the benchmark for the selected models. For each, load the project, take
 * its main model, cross-check VM-vs-wasm parity, then measure eval time on both
 * engines. Returns the rows (so a test can assert) and prints a markdown table.
 */
export async function runBenchmark(opts: Readonly<BenchOpts>): Promise<BenchTable> {
  const rows: Array<BenchRow> = [];

  for (const key of selectedModels()) {
    const spec = MODELS[key];
    const data = fs.readFileSync(spec.path);
    const project = await spec.open(data);
    try {
      const model = await project.mainModel();

      await crossCheck(model, spec.label);

      const vm = await timeEngine(model, 'vm', opts);
      const wasm = await timeEngine(model, 'wasm', opts);

      rows.push({ model: spec.label, vm, wasm, ratio: vm.medianMs / wasm.medianMs });
    } finally {
      await project.dispose();
    }
  }

  printSummary(rows);
  return rows;
}
