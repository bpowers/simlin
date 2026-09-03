#!/usr/bin/env node
// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Allocator A/B benchmark for the libsimlin wasm bundle, on the heaviest model
// we have (C-LEARN v77, ~53k MDL lines). It drives the public @simlin/engine
// API exactly as the app does -- Project.openVensim (parse + salsa sync),
// Model.simulate (salsa compile + Vm::new), Sim.runToEnd, then every series
// and, with LTM, the link scores -- and times each stage per bundle. The
// compile stage is the allocator-bound one (tens of millions of small,
// short-lived allocations on this model), which is why bundles built with
// different global allocators are compared here rather than on a synthetic
// allocation loop.
//
// Usage:
//   node src/engine/bench/clearn-alloc.mjs [options] NAME=PATH.wasm [NAME=PATH.wasm ...]
//     --iters N        measured iterations per bundle and LTM mode (default 10)
//     --warmup N       discarded iterations before measuring (default 2)
//     --ltm MODE       on | off | both (default both)
//     --model PATH     a Vensim .mdl (default: C-LEARN v77)
//     --count-grows    rewrite each bundle so every `memory.grow` also bumps an
//                      exported counter (needs `wasm-tools` on PATH); the
//                      rewritten copy is what gets measured
//     --json PATH      also write the per-iteration samples as JSON
//
// Methodology. The bundles are interleaved (A, B, A, B, ...) so machine drift
// is shared rather than attributed to whichever ran last; medians and minima
// are reported, never means. Each iteration instantiates a FRESH instance of a
// module compiled once per bundle: a fresh linear memory means every iteration
// starts from a cold heap, the state a page load leaves the allocator in, and
// no iteration inherits fragmentation from the one before -- while V8 keeps
// its optimized code for the shared module, so the warm-up iterations warm the
// JIT and only the JIT. Instantiation itself is not timed.
//
// Footprint. `memory.size` never shrinks, so the page count after the last
// stage is that iteration's peak; it is reported alongside the timings. The
// number of `memory.grow` calls is exact only with --count-grows; without it
// the script reports how many times the memory's ArrayBuffer identity changed
// between stage boundaries, which is a lower bound (one boundary can hide many
// grows). Results print to the console and are not committed: the harness is
// regenerable and checked-in numbers go stale.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { execFileSync } from 'node:child_process';
import { parseArgs } from 'node:util';
import { fileURLToPath } from 'node:url';

import { Project, ready, resetWasm } from '@simlin/engine';
import { getExports, getMemory } from '@simlin/engine/internal/wasm';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const DEFAULT_MODEL = path.join(REPO_ROOT, 'test', 'xmutil_test_models', 'C-LEARN v77 for Vensim.mdl');
const WASM_PAGE = 65536;

const { values: opts, positionals } = parseArgs({
  options: {
    iters: { type: 'string', default: '10' },
    warmup: { type: 'string', default: '2' },
    ltm: { type: 'string', default: 'both' },
    model: { type: 'string', default: DEFAULT_MODEL },
    'count-grows': { type: 'boolean', default: false },
    json: { type: 'string' },
    help: { type: 'boolean', default: false },
  },
  allowPositionals: true,
});

if (opts.help || positionals.length === 0) {
  console.error('usage: clearn-alloc.mjs [--iters N] [--warmup N] [--ltm on|off|both] [--model PATH]');
  console.error('                        [--count-grows] [--json PATH] NAME=PATH.wasm [NAME=PATH.wasm ...]');
  process.exit(positionals.length === 0 ? 2 : 0);
}

const iters = Number.parseInt(opts.iters, 10);
const warmup = Number.parseInt(opts.warmup, 10);
if (!Number.isInteger(iters) || iters < 1 || !Number.isInteger(warmup) || warmup < 0) {
  throw new Error(`--iters must be >= 1 and --warmup >= 0 (got ${opts.iters}, ${opts.warmup})`);
}
const ltmModes = { on: [true], off: [false], both: [true, false] }[opts.ltm];
if (ltmModes === undefined) {
  throw new Error(`--ltm must be on, off or both (got ${opts.ltm})`);
}

/**
 * Rewrite a bundle so each `memory.grow` goes through a counting shim exported
 * as the mutable i32 global `__grow_count`. Text-level: the shim function and
 * its global are appended after every existing definition, so no existing
 * function or global index moves and the rest of the module is untouched. The
 * regex matches only instruction lines (a data segment's bytes are printed on
 * `(data ...)` lines), so a string constant containing "memory.grow" is safe.
 */
function instrumentGrows(wasmPath, outPath) {
  const wat = execFileSync('wasm-tools', ['print', wasmPath], { maxBuffer: 1 << 30, encoding: 'utf8' });
  let sites = 0;
  let text = wat.replace(/^(\s+)memory\.grow\s*$/gm, (_m, indent) => {
    sites++;
    return `${indent}call $__grow_hook`;
  });
  const hook = [
    '  (func $__grow_hook (param i32) (result i32)',
    '    global.get $__grow_count',
    '    i32.const 1',
    '    i32.add',
    '    global.set $__grow_count',
    '    local.get 0',
    '    memory.grow)',
    '  (global $__grow_count (mut i32) (i32.const 0))',
    '  (export "__grow_count" (global $__grow_count))',
    ')',
  ].join('\n');
  const end = text.lastIndexOf(')');
  text = text.slice(0, end) + hook + text.slice(end + 1);
  const watPath = `${outPath}.wat`;
  fs.writeFileSync(watPath, text);
  try {
    execFileSync('wasm-tools', ['parse', watPath, '-o', outPath]);
  } finally {
    fs.unlinkSync(watPath);
  }
  return sites;
}

function parseBundleArg(arg, tmpDir) {
  const eq = arg.indexOf('=');
  const name = eq >= 0 ? arg.slice(0, eq) : path.basename(arg, '.wasm');
  let wasmPath = path.resolve(eq >= 0 ? arg.slice(eq + 1) : arg);
  let growSites;
  if (opts['count-grows']) {
    const instrumented = path.join(tmpDir, `${name}-grow.wasm`);
    growSites = instrumentGrows(wasmPath, instrumented);
    wasmPath = instrumented;
  }
  return { name, wasmPath, growSites, sizeBytes: fs.statSync(wasmPath).size };
}

/** Upper-middle element of the sorted samples, matching tests/bench-stats.ts. */
function median(xs) {
  const sorted = [...xs].sort((a, b) => a - b);
  return sorted[sorted.length >> 1];
}

function pages() {
  return getMemory().buffer.byteLength / WASM_PAGE;
}

/**
 * One measured iteration on a fresh instance: returns per-stage milliseconds,
 * the memory pages after each stage, the peak (final) page count, and the
 * grow counts (exact when instrumented; otherwise a lower bound from buffer
 * identity changes observed between stages).
 */
async function runOnce(module, modelBytes, enableLtm) {
  const ms = {};
  const pagesAfter = {};
  let bufferChanges = 0;
  let lastBuffer = null;

  const stage = async (name, body) => {
    const t0 = performance.now();
    const out = await body();
    ms[name] = performance.now() - t0;
    const memory = getMemory();
    if (lastBuffer !== null && memory.buffer !== lastBuffer) {
      bufferChanges++;
    }
    lastBuffer = memory.buffer;
    pagesAfter[name] = pages();
    return out;
  };

  await ready(module);
  lastBuffer = getMemory().buffer;
  const initialPages = pages();

  const project = await stage('open', () => Project.openVensim(modelBytes));
  const model = await project.mainModel();
  const sim = await stage('compile', () => model.simulate({}, { enableLtm, engine: 'vm' }));
  await stage('run', () => sim.runToEnd());
  const stepCount = await sim.getStepCount();
  const names = await sim.getVarNames();
  await stage('series', async () => {
    for (const name of names) {
      await sim.getSeries(name);
    }
  });
  if (enableLtm) {
    const links = await stage('links', () => sim.getLinks());
    if (links.length === 0) {
      throw new Error('LTM run produced no links');
    }
  }
  await stage('dispose', async () => {
    await sim.dispose();
    await project.dispose();
  });

  const growGlobal = getExports().__grow_count;
  const grows = growGlobal instanceof WebAssembly.Global ? growGlobal.value : undefined;
  const peakPages = pages();
  await resetWasm();

  ms.total = Object.values(ms).reduce((a, b) => a + b, 0);
  return { ms, pagesAfter, initialPages, peakPages, grows, bufferChanges, stepCount, varCount: names.length };
}

async function compileBundle(bundle) {
  const bytes = fs.readFileSync(bundle.wasmPath);
  return WebAssembly.compile(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
}

function fmtMs(v) {
  return v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
}

function fmtRatio(v) {
  return Number.isFinite(v) ? `${v.toFixed(2)}x` : '-';
}

function fmtMiB(pagesCount) {
  return ((pagesCount * WASM_PAGE) / (1024 * 1024)).toFixed(1);
}

function printTable(bundles, enableLtm, samples) {
  const stages = ['open', 'compile', 'run', 'series', ...(enableLtm ? ['links'] : []), 'dispose', 'total'];
  const head = ['stage'];
  const align = ['---'];
  for (const [i, b] of bundles.entries()) {
    head.push(`${b.name} median`, `${b.name} min`);
    align.push('--:', '--:');
    if (i > 0) {
      head.push(`${b.name}/${bundles[0].name}`);
      align.push('--:');
    }
  }
  const rows = [head, align];
  const base = samples[bundles[0].name];
  for (const stage of stages) {
    const row = [`${stage} (ms)`];
    for (const [i, b] of bundles.entries()) {
      const xs = samples[b.name].map((s) => s.ms[stage]);
      row.push(fmtMs(median(xs)), fmtMs(Math.min(...xs)));
      if (i > 0) {
        row.push(fmtRatio(median(xs) / median(base.map((s) => s.ms[stage]))));
      }
    }
    rows.push(row);
  }
  const memRow = (label, pick, fmt) => {
    const row = [label];
    for (const [i, b] of bundles.entries()) {
      const xs = samples[b.name].map(pick);
      if (xs.some((x) => x === undefined)) {
        row.push('-', '-');
        if (i > 0) {
          row.push('-');
        }
        continue;
      }
      row.push(fmt(median(xs)), fmt(Math.min(...xs)));
      if (i > 0) {
        row.push(fmtRatio(median(xs) / median(base.map(pick))));
      }
    }
    rows.push(row);
  };
  memRow('peak memory.size (pages)', (s) => s.peakPages, String);
  memRow('peak memory.size (MiB)', (s) => s.peakPages, fmtMiB);
  memRow(
    'memory.grow calls' + (opts['count-grows'] ? '' : ' (lower bound)'),
    (s) => s.grows ?? s.bufferChanges,
    String,
  );

  const first = base[0];
  console.log('');
  console.log(
    `### ${path.basename(opts.model)}, LTM ${enableLtm ? 'on' : 'off'}: ${iters} iterations after ${warmup} warm-up, ` +
      `node ${process.version} (V8 ${process.versions.v8}), ${first.varCount} variables x ${first.stepCount} steps`,
  );
  console.log('');
  for (const row of rows) {
    console.log(`| ${row.join(' | ')} |`);
  }
}

async function main() {
  const tmpDir = opts['count-grows'] ? fs.mkdtempSync(path.join(os.tmpdir(), 'clearn-alloc-')) : null;
  try {
    const bundles = positionals.map((arg) => parseBundleArg(arg, tmpDir));
    const seen = new Set();
    for (const b of bundles) {
      if (seen.has(b.name)) {
        throw new Error(`duplicate bundle name ${b.name}; use NAME=PATH to disambiguate`);
      }
      seen.add(b.name);
    }
    const modelBytes = new Uint8Array(fs.readFileSync(opts.model));

    console.log(`model: ${opts.model} (${modelBytes.length} bytes)`);
    for (const b of bundles) {
      const sites = b.growSites === undefined ? '' : `, ${b.growSites} memory.grow site(s) instrumented`;
      console.log(`bundle ${b.name}: ${b.wasmPath} (${b.sizeBytes} bytes${sites})`);
      b.module = await compileBundle(b);
    }

    const all = {};
    for (const enableLtm of ltmModes) {
      const samples = Object.fromEntries(bundles.map((b) => [b.name, []]));
      for (let i = 0; i < warmup + iters; i++) {
        for (const b of bundles) {
          if (typeof globalThis.gc === 'function') {
            globalThis.gc();
          }
          const sample = await runOnce(b.module, modelBytes, enableLtm);
          if (i >= warmup) {
            samples[b.name].push(sample);
          }
          process.stderr.write(
            `${enableLtm ? 'ltm ' : 'nol '}${i < warmup ? 'warm' : 'iter'} ${String(i + 1).padStart(2)} ${b.name.padEnd(12)} ` +
              `open ${fmtMs(sample.ms.open).padStart(5)}  compile ${fmtMs(sample.ms.compile).padStart(5)}  ` +
              `run ${fmtMs(sample.ms.run).padStart(5)}  total ${fmtMs(sample.ms.total).padStart(5)} ms  ` +
              `pages ${String(sample.peakPages).padStart(5)}\n`,
          );
        }
      }
      printTable(bundles, enableLtm, samples);
      all[enableLtm ? 'ltm' : 'noltm'] = samples;
    }

    if (opts.json) {
      fs.writeFileSync(
        opts.json,
        JSON.stringify(
          {
            node: process.version,
            v8: process.versions.v8,
            model: opts.model,
            iters,
            warmup,
            bundles: bundles.map(({ name, wasmPath, sizeBytes, growSites }) => ({
              name,
              wasmPath,
              sizeBytes,
              growSites,
            })),
            samples: all,
          },
          null,
          2,
        ),
      );
    }
  } finally {
    if (tmpDir !== null) {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  }
}

await main();
