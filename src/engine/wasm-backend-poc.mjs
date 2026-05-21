// Throwaway proof-of-concept for the compile-to-WebAssembly backend.
//
// Demonstrates the "direct-drive" architecture end to end in Node:
//   1. load libsimlin.wasm (the engine, compiled to wasm)
//   2. open default_projects/population/model.xmile and get its model
//   3. call simlin_model_compile_to_wasm -> a *second* wasm module (the model)
//   4. JS instantiates that model module directly and drives its `run` export
//      (libsimlin is not on the per-run hot path)
//   5. check every VM variable's series shows up as a column of the blob's
//      results, and compare run-to-run timing of the blob vs the bytecode VM.
//
// Run:  node src/engine/wasm-backend-poc.mjs
//
// This file is exploratory scaffolding, not part of the @simlin/engine API.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(here, 'core', 'libsimlin.wasm');
const MODEL = join(here, '..', '..', 'default_projects', 'population', 'model.xmile');

// ── load libsimlin (mirrors src/engine/src/internal/wasm.node.ts) ──────────
let memory = new WebAssembly.Memory({ initial: 256, maximum: 16384 });
const lib = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(WASM)), {
  env: { memory },
});
const E = lib.exports;
if (E.memory instanceof WebAssembly.Memory) memory = E.memory;
E.simlin_init?.();

if (typeof E.simlin_model_compile_to_wasm !== 'function') {
  throw new Error('libsimlin.wasm is stale: missing simlin_model_compile_to_wasm (rebuild it)');
}

// ── minimal FFI glue (re-derived per call so memory growth is handled) ─────
const TD = new TextDecoder();
const TE = new TextEncoder();
const dv = () => new DataView(memory.buffer);
const malloc = (n) => {
  const p = E.simlin_malloc(n);
  if (!p && n) throw new Error('wasm allocation failed');
  return p;
};
const free = (p) => {
  if (p) E.simlin_free(p);
};
const u32 = (p) => dv().getUint32(p, true);
const outPtr = () => {
  const p = malloc(4);
  dv().setUint32(p, 0, true);
  return p;
};
const writeBytes = (bytes) => {
  const p = malloc(bytes.length);
  new Uint8Array(memory.buffer, p, bytes.length).set(bytes);
  return p;
};
const cstr = (s) => writeBytes(TE.encode(s + '\0'));
const readBytes = (p, n) => new Uint8Array(memory.buffer.slice(p, p + n));
const readCStr = (p) => {
  const v = new Uint8Array(memory.buffer);
  let e = p;
  while (v[e]) e++;
  return TD.decode(v.slice(p, e));
};
const f64Array = (p, n) => {
  const d = dv();
  const out = new Float64Array(n);
  for (let i = 0; i < n; i++) out[i] = d.getFloat64(p + i * 8, true);
  return out;
};
function checkErr(ep, what) {
  const err = u32(ep);
  if (err !== 0) {
    let msg = '(no message)';
    const mp = E.simlin_error_get_message(err);
    if (mp) msg = readCStr(mp);
    E.simlin_error_free(err);
    throw new Error(`${what}: ${msg}`);
  }
}

// ── open population, get its model, extract the compiled-model wasm ────────
const xmile = readFileSync(MODEL);
let dataPtr = writeBytes(xmile);
let ep = outPtr();
const project = E.simlin_project_open_xmile(dataPtr, xmile.length, ep);
checkErr(ep, 'open_xmile');
free(ep);
free(dataPtr);

const namePtr = cstr('main');
ep = outPtr();
const model = E.simlin_project_get_model(project, namePtr, ep);
checkErr(ep, 'get_model');
free(ep);
free(namePtr);

const outBuf = outPtr();
const outLen = outPtr();
ep = outPtr();
E.simlin_model_compile_to_wasm(model, outBuf, outLen, ep);
checkErr(ep, 'compile_to_wasm');
const blobPtr = u32(outBuf);
const blobLen = u32(outLen);
const blob = readBytes(blobPtr, blobLen);
free(blobPtr);
free(outBuf);
free(outLen);
free(ep);
console.log(`compiled model -> ${blobLen} bytes of WebAssembly`);

// ── direct-drive: JS instantiates the model blob and calls run() ──────────
const { instance: mi } = await WebAssembly.instantiate(blob, {});
const ME = mi.exports;
const nSlots = ME.n_slots.value;
const nChunks = ME.n_chunks.value;
const resultsOffset = ME.results_offset.value;
console.log(`blob self-describes: n_slots=${nSlots}, n_chunks=${nChunks}, results_offset=${resultsOffset}`);

ME.run();
const blobColumn = (col) => {
  const d = new DataView(ME.memory.buffer);
  const s = new Float64Array(nChunks);
  for (let c = 0; c < nChunks; c++) s[c] = d.getFloat64(resultsOffset + (c * nSlots + col) * 8, true);
  return s;
};
const blobCols = Array.from({ length: nSlots }, (_, c) => blobColumn(c));

// ── VM golden via libsimlin ────────────────────────────────────────────────
ep = outPtr();
const sim = E.simlin_sim_new(model, 0, ep);
checkErr(ep, 'sim_new');
free(ep);
ep = outPtr();
E.simlin_sim_run_to_end(sim, ep);
checkErr(ep, 'run_to_end');
free(ep);

const vmSeries = (name) => {
  const np = cstr(name);
  const rp = malloc(nChunks * 8);
  const wp = outPtr();
  const e = outPtr();
  E.simlin_sim_get_series(sim, np, rp, nChunks, wp, e);
  checkErr(e, `get_series(${name})`);
  const written = u32(wp);
  const s = f64Array(rp, written);
  free(np);
  free(rp);
  free(wp);
  free(e);
  return s;
};

// ── correctness: match every VM variable's series to a blob column ─────────
console.log('\ncorrectness (each VM variable matched to a blob column by value):');
const vars = ['time', 'population', 'births', 'deaths', 'birth_rate', 'average_lifespan'];
let worst = 0;
for (const name of vars) {
  let vm;
  try {
    vm = vmSeries(name);
  } catch (e) {
    console.log(`  ${name.padEnd(18)} (skipped: ${e.message})`);
    continue;
  }
  let best = Infinity;
  let bestCol = -1;
  for (let col = 0; col < nSlots; col++) {
    let m = 0;
    for (let c = 0; c < vm.length; c++) m = Math.max(m, Math.abs(vm[c] - blobCols[col][c]));
    if (m < best) {
      best = m;
      bestCol = col;
    }
  }
  worst = Math.max(worst, best);
  console.log(`  ${name.padEnd(18)} -> blob column ${bestCol}, max|Δ| = ${best.toExponential(2)}`);
}
console.log(`worst mismatch across variables: ${worst.toExponential(2)} -> ${worst < 1e-9 ? 'MATCH' : 'FAIL'}`);

const pop = vmSeries('population');
console.log(`\npopulation: ${pop[0].toFixed(2)} (t=start) ... ${pop[pop.length - 1].toFixed(2)} (t=stop), ${pop.length} steps`);

// ── timing: blob run() vs VM reset+run_to_end (both re-simulate from t0) ───
console.log('\ntiming (each call re-runs the whole simulation):');
const NB = 5000;
let t = performance.now();
for (let i = 0; i < NB; i++) ME.run();
const blobMs = (performance.now() - t) / NB;

const NV = 500;
t = performance.now();
for (let i = 0; i < NV; i++) {
  const e1 = outPtr();
  E.simlin_sim_reset(sim, e1);
  checkErr(e1, 'reset');
  free(e1);
  const e2 = outPtr();
  E.simlin_sim_run_to_end(sim, e2);
  checkErr(e2, 'run_to_end');
  free(e2);
}
const vmMs = (performance.now() - t) / NV;

console.log(`  blob run():           ${blobMs.toFixed(5)} ms/run  (${NB} runs)`);
console.log(`  VM reset+run_to_end:  ${vmMs.toFixed(5)} ms/run  (${NV} runs)`);
console.log(`  blob is ${(vmMs / blobMs).toFixed(1)}x faster per re-simulation`);
