# @simlin/engine WebAssembly Simulation Backend — Phase 4: node benchmark

**Goal:** A repeatable Node benchmark comparing VM vs wasm **simulation (eval) time** for fishbanks, WORLD3, and C-LEARN, through the public `Model.simulate({ engine })` API, with explicit warmup and median reporting.

**Architecture:** A gated jest test under `src/engine/tests/` reuses the ts-jest-from-source pipeline (no build step, no new dependency, no `tsx`/`ts-node`). The benchmark splits into a small **pure** stats/harness module (`median`, an adaptive warmup+measure loop) that is always-on unit-tested, and an **imperative** runner that loads the three models, builds one `Sim` per `(model, engine)` once (untimed — for wasm this is the blob compile/instantiate), then times only `runToEnd()` in a loop (with `reset()` between runs, outside the clock). The heavy run is gated behind `RUN_BENCH=1` so it stays out of the default `pnpm test` (respecting the project's per-test time budget) while the pure helpers keep coverage. It mirrors `examples/backend_bench.rs`'s eval-vs-eval methodology and median statistic, adds the explicit warmup the AC requires, prints a markdown table, and (per the project's "no stale benchmark data" rule) checks in the harness only — never a results file.

**Tech Stack:** TypeScript; jest (ts-jest, `testEnvironment: node`); `performance.now()` from `node:perf_hooks`; `fs.readFileSync` + `path.join(__dirname, ...)`; the public `@simlin/engine` API (`Project.open*`, `Model.simulate({ engine })`, `Sim.reset`/`runToEnd`/`getRun`/`dispose`).

**Scope:** Phase 4 of 4. Depends on Phase 2 (the `engine` demux + `Model.simulate({ engine })`) and Phase 1 (the resumable `reset` export so `reset()`+`runToEnd()` re-runs the wasm blob without recompiling). It does not depend on Phase 3.

**Codebase verified:** 2026-05-22

---

## Acceptance Criteria Coverage

### engine-wasm-sim.AC9: Node benchmark
- **engine-wasm-sim.AC9.1 Success:** a node benchmark reports warm-median simulation (eval) time for fishbanks, WORLD3, and C-LEARN on both engines, via `Model.simulate({engine})`, with explicit warmup.

---

## Background: what exists today (verified)

All paths absolute from `/home/bpowers/src/simlin`.

**No TS benchmark exists yet** in `src/engine` (no `bench`/`benchmarks` dir, no `*.bench.ts`, no `examples/`, no `bench` package script). The only Node timing precedent is the throwaway `src/engine/wasm-backend-poc.mjs` (plain `.mjs`, bypasses the package API, mean not median, mismatched iteration counts — **not** the model to copy). The reference methodology is the Rust `src/simlin-engine/examples/backend_bench.rs`.

**`backend_bench.rs` (the methodology to mirror):**
- Models (`MODELS`, `:243-262`), relative to `src/simlin-engine`: fishbanks `../../default_projects/fishbanks/model.xmile` (XMILE), wrld3 `../../test/metasd/WRLD3-03/wrld3-03.mdl` (Vensim), clearn `../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (Vensim). Selected via `BENCH_MODELS` (default all three).
- Eval region (`:481-512`): the `Vm`/wasm instance is created **once in untimed setup**; the timed body is `vm.reset(); time(vm.run_to_end())` (VM) and `time(run())` (wasm). Compile/build/instantiate are timed in **separate** phases, excluded from eval.
- Harness (`bench`, `:165-203`): adaptive — runs `min_iters` (=1), then until `max_iters` (=100) or a per-phase wall-clock `budget_s` (=2.5, `BENCH_TIME_BUDGET`) elapses. Statistic: **median** (`times[iters/2]` after sort), reported with `min` and iteration count. No discrete warmup (the adaptive median + min absorbs it) — but **AC9.1 requires explicit warmup**, so the TS benchmark adds a discard-warmup loop (a deliberate, AC-mandated divergence).
- Report (`print_summary`, `:644-732`): a markdown eval table `| model | VM reset+run | wasm run | wasm/VM | front-end compile (shared) |`, each cell `median (n=…, min …)`. It also `cross_check`s (`:336-363`) that VM and wasm produce matching series, guarding against benchmarking a broken run.

**Project conventions:**
- `docs/dev/benchmarks.md` documents only the Criterion Rust benches (`cargo bench`); it predates `backend_bench.rs` and has no Node section (consider adding one — see the optional doc task). The MEMORY rule "No stale benchmark data": **commit the regenerable harness, not the numbers; show results in-chat.**
- Root `CLAUDE.md` hard rule: individual tests complete in a few seconds on debug; `cargo test --workspace` under a 3-minute cap. An ungated C-LEARN×2-engine benchmark would violate this, so the heavy run **must be gated** out of the default `jest` run (mirroring the `#[ignore]`d heavy Rust tests).

**Public API (current; Phase 2 adds `{ engine }`):**
- `Project.open(xmile, opts?)` (`src/engine/src/project.ts:54`, XMILE — **not** a format sniffer), `Project.openVensim(data, opts?)` (`:103`), `Project.openProtobuf` (`:70`). Each calls `backend.init(opts?.wasm)` internally (idempotent), so **no separate `ready()`/`configureWasm` is needed in Node**; the default WASM source resolves to the bundled `core/libsimlin.wasm` (`internal/wasm.node.ts:39-42`).
- `project.mainModel()` (`project.ts:157`) → the default `Model`.
- `Model.simulate(overrides?, options?)` (`model.ts:430`) — Phase 2 makes `options` `{ enableLtm?; engine?: 'vm' | 'wasm' }`. `Sim.reset` (`sim.ts:127`), `Sim.runToEnd` (`:119`), `Sim.getRun` (`:198`, fetches **all** series + `loops()` + `getLinks()` + `getStepCount` — heavy; exclude from timing), `Sim.dispose` (`:227`).
- High-level usage to mirror: `src/engine/tests/api.test.ts:444-528` (`await Project.open*(bytes); const model = await project.mainModel(); const sim = await model.simulate(...)`).

**Timer + model files:**
- `performance.now()` from `node:perf_hooks` (the only repo precedent: `wasm-backend-poc.mjs:19`). No `process.hrtime` usage anywhere. `performance.now()` returns fractional ms.
- Model files (all plain bytes, no LFS; verified sizes): `default_projects/fishbanks/model.xmile` (7.8 KB), `test/metasd/WRLD3-03/wrld3-03.mdl` (151 KB), `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (1.4 MB). The C-LEARN `.xmile` (393 B) is a header-only stub — **unusable**; use the `.mdl`. From a test at `src/engine/tests/`, the repo root is three levels up: `path.join(__dirname, '..', '..', '..', 'default_projects', 'fishbanks', 'model.xmile')`, etc.
- C-LEARN cost: compile dominates (~3.85s native; happens in untimed setup), eval ~hundreds of ms. Keep its measured iteration count low; the adaptive budget handles this.

**Test pipeline:** `src/engine/jest.config.js` — `preset: ts-jest`, `testEnvironment: node`, `testMatch: tests/**/*.test.ts`, `moduleNameMapper` rewrites `@simlin/engine/internal/*` to the node source. A non-`.test.ts` module under `tests/` is importable but not auto-run. Run a single file: `pnpm -C src/engine exec jest <name>`.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
Subcomponent A: a tested pure stats/harness, then the imperative benchmark runner that uses it.

<!-- START_TASK_1 -->
### Task 1: Pure benchmark harness (`median` + adaptive warmup/measure loop)

**Verifies:** (functional core for AC9.1 — the statistic and warmup/iteration policy)

**Files:**
- Create: `src/engine/tests/bench-stats.ts` (pure, non-`.test.ts`, importable)
- Test: `src/engine/tests/bench-stats.test.ts` (always-on unit tests)

**Implementation:**
A pure, side-effect-free harness (the timing body is injected, so it is deterministically testable):
- `interface Stat { medianMs: number; minMs: number; iters: number }`.
- `median(times: number[]): number` — sort a copy ascending, return `times[times.length >> 1]` (matching `backend_bench.rs:181`'s `times[iters/2]`); `NaN` for empty input.
- `interface BenchOpts { warmup: number; minIters: number; maxIters: number; budgetMs: number }`.
- `runTimed(opts: BenchOpts, body: () => number, now: () => number = () => performance.now()): Stat` — run `opts.warmup` iterations and **discard** them (the explicit warmup AC9.1 requires); then collect timings: while `times.length < maxIters && (times.length < minIters || now() - start < budgetMs)`, push `body()`. Return `{ medianMs: median(times), minMs: Math.min(...times), iters: times.length }`. (`now` is injectable so tests don't depend on wall-clock.)
- `runTimedAsync(opts: BenchOpts, asyncBody: () => Promise<number>, now: () => number = () => performance.now()): Promise<Stat>` — the **async** twin of `runTimed` (the benchmark's eval is `async`): identical warmup-discard + adaptive-median policy, but `await`s `asyncBody()` each iteration. This keeps the warmup/median policy in the tested core so Task 2's timed region is single-sourced and tested (not an ad-hoc inline loop).

`body()` returns the elapsed ms of one measured run; `runTimed` does not itself call `performance.now()` around `body` — the caller times the precise region (see Task 2), exactly as `backend_bench.rs` has the closure return `ms_since(t0)`.

**Testing** (deterministic, no models, no WASM):
- `median`: odd/even lengths, unsorted input, single element, empty (`NaN`).
- `runTimed`: with a `body` returning a fixed sequence and a fake `now`, assert (a) exactly `warmup` calls are discarded before measurement, (b) it stops at `maxIters`, (c) it stops early when the injected `now` exceeds `budgetMs` after `minIters`, (d) `minIters` is honored even if the budget is already exceeded, (e) the returned `medianMs`/`minMs`/`iters` are correct.
- `runTimedAsync`: with a fake **async** `body` (resolving a fixed sequence) and a fake `now`, assert the same five properties (a)–(e) as `runTimed`, confirming the async twin shares the warmup-discard + median policy.

**Verification:**
Run: `pnpm -C src/engine exec jest tests/bench-stats.test.ts 2>&1 | tail -20`
Expected: all unit tests pass.

**Commit:** `engine: pure benchmark harness (median + adaptive warmup/measure)`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: VM-vs-wasm eval benchmark runner (gated) over fishbanks / WORLD3 / C-LEARN

**Verifies:** engine-wasm-sim.AC9.1

**Files:**
- Create: `src/engine/tests/backend-bench.ts` (the imperative runner; exports `runBenchmark`)
- Create: `src/engine/tests/backend-bench.test.ts` (gated benchmark + cross-check assertions)

**Implementation:**
`backend-bench.ts` — the imperative shell, using the public API and the Task-1 harness:
- A model table mapping `'fishbanks' | 'wrld3' | 'clearn'` to `{ path, open }`, where `path` uses `path.join(__dirname, '..', '..', '..', ...)` (repo root is three levels up from `tests/`) and `open` is `Project.open` (fishbanks XMILE) or `Project.openVensim` (WORLD3, C-LEARN `.mdl`). Default to all three; honor a `BENCH_MODELS` comma-list env (mirroring `backend_bench.rs`).
- `async function timeEngine(model, engine: 'vm' | 'wasm', opts): Promise<Stat>`:
  1. `const sim = await model.simulate({ engine });` — **untimed setup** (for wasm this compiles + instantiates the blob once; for vm it creates the libsimlin sim).
  2. `const stat = await runTimedAsync(opts, async () => { await sim.reset(); const t0 = performance.now(); await sim.runToEnd(); return performance.now() - t0; });` — `reset()` runs inside the async body but **before** the clock is sampled (it is setup, not part of the measured region); only `runToEnd()` is timed. `runTimedAsync` (defined and unit-tested in Task 1) keeps the warmup-discard + median policy in the tested core, so the timed region is single-sourced — do **not** hand-roll an inline measure loop here. Do **not** time `getRun()`/`getSeries()`.
  3. `await sim.dispose()` after measuring.
- `async function crossCheck(model): Promise<void>`: outside any timing, run the model on both engines to completion and compare a representative variable's series (via `getRun()` or `getSeries`) within the engine's existing tolerance — a sanity guard that both engines computed the same thing (mirrors `backend_bench.rs:336-363`). Throw on mismatch.
- `async function runBenchmark(opts): Promise<BenchTable>`: for each selected model, load + `mainModel()`, run `crossCheck`, then `timeEngine` for `'vm'` and `'wasm'`; collect `{ model, vm: Stat, wasm: Stat, ratio: vm.medianMs / wasm.medianMs }`. Return the rows (so the test can assert) and `console.log` a markdown table `| model | VM eval (median ms) | wasm eval (median ms) | wasm/VM | iters |`, plus a one-line note that this is eval-only (compile excluded) and that absolute numbers include the async public-API overhead (the VM/wasm ratio is the meaningful figure).

`backend-bench.test.ts`:
- Always-on: nothing heavy (the pure harness is covered by Task 1).
- Gated heavy run: `const RUN = process.env.RUN_BENCH === '1'; (RUN ? it : it.skip)('benchmarks VM vs wasm eval (fishbanks/WORLD3/C-LEARN)', async () => { const rows = await runBenchmark({ warmup: 3, minIters: 3, maxIters: 100, budgetMs: 2500 }); for (const r of rows) { expect(Number.isFinite(r.vm.medianMs)).toBe(true); expect(r.vm.medianMs).toBeGreaterThan(0); expect(Number.isFinite(r.wasm.medianMs)).toBe(true); expect(r.wasm.medianMs).toBeGreaterThan(0); } }, 300_000);` — the cross-check inside `runBenchmark` asserts correctness; this asserts every model ran on both engines with a positive finite median. The 5-minute timeout covers C-LEARN's compile-heavy setup.

> **Why gated:** C-LEARN's compile (~3.85s) × 2 engines, plus iterated evals, exceeds the few-seconds-per-test budget. `it.skip` keeps it out of the default `pnpm test`; `RUN_BENCH=1` opts in. **Do not** commit a results file — report the printed numbers in the PR/chat per the "no stale benchmark data" rule.

**Verification:**
Run (the gated benchmark, all three models, both engines): `RUN_BENCH=1 pnpm -C src/engine exec jest backend-bench 2>&1 | tail -40`
Expected: it loads fishbanks/WORLD3/C-LEARN, runs each on `vm` and `wasm`, the cross-check passes for each, the assertions pass, and a markdown table of warm-median eval times + wasm/VM ratios is printed.
Run (confirm it stays out of the default suite): `pnpm -C src/engine test 2>&1 | tail -15`
Expected: green, with the benchmark `it` skipped (and the fast `bench-stats` unit tests passing).

**Commit:** `engine: node VM-vs-wasm eval benchmark for fishbanks/WORLD3/C-LEARN`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (task 3) -->
Subcomponent B: keep the benchmarks doc current.

<!-- START_TASK_3 -->
### Task 3: Document the node benchmark in `docs/dev/benchmarks.md`

**Verifies:** (documentation; no AC — supports AC9.1 discoverability)

**Files:**
- Modify: `docs/dev/benchmarks.md`

**Implementation:**
Add a short "Node VM-vs-wasm eval benchmark" section: how to run it (`RUN_BENCH=1 pnpm -C src/engine exec jest backend-bench`, optional `BENCH_MODELS=fishbanks,wrld3`), what it measures (eval-only, compile excluded), the median+warmup policy, and the reminder that results are reported in-chat/PR and **not** checked in (per the "no stale benchmark data" rule). Cross-reference `src/simlin-engine/examples/backend_bench.rs` as the Rust counterpart. Do not add a "Last updated" line (per repo CLAUDE.md).

**Verification:**
Run: `pnpm format 2>&1 | tail -5` (or the repo's markdown/format check)
Expected: the doc passes formatting. No `docs/README.md` index change is needed: `docs/CLAUDE.md` requires updating the index only when **adding/moving/renaming** a docs file, and this task **modifies** an already-indexed file (`docs/dev/benchmarks.md` is already listed). Do not edit the index.

**Commit:** `doc: document the node VM-vs-wasm eval benchmark`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_B -->

---

## Phase 4 Done When

- A gated node benchmark runs fishbanks, WORLD3, and C-LEARN on both engines via `Model.simulate({ engine })`, with an explicit warmup phase, and reports a warm **median** eval (simulation) time per engine plus the wasm/VM ratio in a markdown table (AC9.1).
- The eval timing excludes blob compile/instantiate and result-extraction (`getRun`/`getSeries`); a cross-check confirms both engines produce matching series before the numbers are trusted.
- The pure harness (`median`, adaptive warmup/measure) is unit-tested and always-on; the heavy benchmark is `RUN_BENCH`-gated so `pnpm -C src/engine test` stays within the time budget. Only the harness is committed — no results file.
- `docs/dev/benchmarks.md` documents how to run it.
