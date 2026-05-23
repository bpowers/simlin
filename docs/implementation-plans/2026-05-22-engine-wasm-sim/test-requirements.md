# @simlin/engine WebAssembly Simulation Backend — Test Requirements

This document maps every acceptance criterion of the `@simlin/engine` WebAssembly
simulation backend (WasmSim) feature to its verification. The authoritative AC list is the
"Acceptance Criteria" section of the design plan
([2026-05-22-engine-wasm-sim.md](/docs/design-plans/2026-05-22-engine-wasm-sim.md)); the
verifying tests and their exact file paths come from the four implementation phase plans in
this directory (`phase_01.md` .. `phase_04.md`).

There are **25 acceptance criteria** across AC1..AC9, identified with their fully-scoped
names `engine-wasm-sim.AC1.1` .. `engine-wasm-sim.AC9.1`. **All 25 are covered by automated
tests; none require human verification.** The phase plans were written so that every
criterion is automatable, and the mapping below confirms this end to end.

## Coverage model (read this first)

A few cross-cutting decisions in the phase plans shape how the rows below read; they are
called out here so each AC row stays terse.

- **Two-level coverage for AC2 / AC3 / AC5.** These are proven *twice*. Phase 1 (Rust)
  proves the emitted wasm blob matches the bytecode VM at the **blob/VM-parity level**
  (driving the blob's `run` / `run_to` / `run_initials` / `reset` / `set_value` exports via
  the DLR-FT `wasm-interpreter` test oracle, with the VM as the correctness oracle). Phase 2
  (TypeScript) proves the same behaviors again at the **`@simlin/engine` facade level**
  through `DirectBackend` (`Model`/`Sim` driving the blob as an in-process
  `WebAssembly.Instance`). Both covering tests are listed where they apply.
- **Parity tolerance.** Every "matches the VM" / "matches node" assertion uses the **engine's
  existing comparators** (`ensure_results` / `ensure_results_excluding` on the Rust side; the
  engine's existing tolerance on the TS side), **not a separate threshold**. The wasm backend
  is intentionally not bit-identical to the VM's libm; agreement is judged within the same
  tolerance that gates the wasm backend's own corpus parity. The Phase 4 benchmark instead
  uses an exact-or-within-tolerance series cross-check (see AC9.1).
- **The Phase 4 benchmark (AC9.1) is gated.** Its heavy run is behind `RUN_BENCH=1` and is
  **not part of the default `pnpm test` suite** (it `it.skip`s otherwise). Its *correctness*
  is guarded by an in-benchmark `crossCheck` (both engines must produce matching series
  before any number is trusted); the deliverable is the **warm-median eval-time reporting**,
  which is reported in-chat/PR and **not committed** (per the "no stale benchmark data"
  rule). The always-on pure stats/harness unit tests keep coverage in the default suite.

Test-type legend: **rust-unit** = `#[cfg(test)]` module inside a crate source file;
**rust-integration** = a file under `tests/`; **jest-unit** = a pure-function jest test;
**jest-integration** = a jest test driving `DirectBackend` / `WorkerBackend` / the public
`Model`/`Sim` API end to end.

---

## AC1: Engine selection via `Model.simulate`/`run`

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC1.1** Success: `Model.simulate({engine:'wasm'})` returns a blob-driven `Sim`; `simulate()` / `{engine:'vm'}` returns the VM-backed `Sim`. | jest-integration: `src/engine/tests/wasm-model.test.ts` (primary, public API); jest-integration: `src/engine/tests/wasm-backend.test.ts` (backend-level: the `'sim'` entry records `engine:'wasm'` and holds an instance). | Through `Model.simulate`, `{engine:'wasm'}` yields a `Sim` whose `runToEnd()`+`getSeries()` match the VM; `simulate()` and `{engine:'vm'}` yield VM-backed sims. At the backend, `simNew(modelHandle, false, 'wasm')` creates a sim handle whose entry records the wasm engine + instance, while `simNew(...)` / `simNew(..., 'vm')` still create VM sims. | Phase 2 Task 5 (facade) + Phase 2 Task 3 (backend creation). |
| **engine-wasm-sim.AC1.2** Success: `Model.run({engine:'wasm'})` returns a `Run` whose series match `Model.run({engine:'vm'})` within tolerance. | jest-integration: `src/engine/tests/wasm-model.test.ts`. | `model.run({engine:'wasm'})` series equal `model.run({engine:'vm'})` series within the engine's existing tolerance. | Phase 2 Task 5. |
| **engine-wasm-sim.AC1.3** Success: existing callers passing no `engine` get today's VM behavior (default unchanged). | jest-integration: `src/engine/tests/wasm-model.test.ts` (explicit default-path assertion); plus the **existing** engine suite run green (`pnpm -C src/engine test`) as a regression guard. | `model.simulate(overrides)` / `model.run(overrides)` with no `engine` behave exactly as before (VM); a representative existing test path is unaffected. | Phase 2 Task 5. |

## AC2: `runToEnd`/`runTo` parity (resumable) — two-level

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC2.1** Success: `runToEnd()` (wasm) series equal the VM within tolerance. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` `#[cfg(test)]` — `compile_simulation_run_to_matches_run_and_vm`; rust-integration (real models, `#[ignore]`d, `--release`): `src/simlin-engine/tests/simulate.rs` — `simulates_wrld3_03_wasm`, `simulates_clearn_wasm`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simRunToEnd`+`simGetSeries` vs VM). | Blob level: the full series from `run` and from `run_initials`+`run_to(stop)` both equal `Vm::run_to_end` within `ensure_results` tolerance (triple agreement), incl. on WORLD3 and C-LEARN. Facade level: `simRunToEnd` then `simGetSeries(name)` (wasm) equals the VM for the model's variables. | Phase 1 Tasks 1 & 6 (blob); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC2.2** Success: `runTo(t)` then `getValue(name)` (wasm) equals the VM's value at `t`. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` `#[cfg(test)]` — `compile_simulation_run_to_matches_run_and_vm` (foundation) + `run_to_at_save_and_between_save_points`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simRunTo(t)`+`simGetValue(name)` vs VM after the same `simRunTo(t)`). | Blob level: after `run_to(t)`, the strided value at the chunk for time `t` equals the VM's value at `t`; saved-row count after `run_to(t)` matches the VM for `t` on and between save points. Facade level: `simGetValue` reads the live `curr` chunk (linear-memory base 0), mirroring `vm.get_value_now`, and equals the VM. | Phase 1 Tasks 1 & 2 (blob); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC2.3** Success: segmented `runTo(t1)` then `runTo(t2)` (`t1<t2`) equals a single `runTo(t2)` and the VM. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `run_to_segmented_matches_single_and_vm`; rust-integration (FFI compile path): `src/libsimlin/tests/wasm.rs` — `compile_to_wasm_blob_supports_resumable_run`; rust-integration (real models): `src/simlin-engine/tests/simulate.rs` — `simulates_wrld3_03_wasm`, `simulates_clearn_wasm` (segmented-equals-single); jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simRunTo(t1)`+`simRunTo(t2)` vs single `simRunTo(t2)` and VM). | Blob level: `run_initials; run_to(t1); run_to(t2)` produces a slab whose rows ≤ t2 equal both single `run_to(t2)` and the VM driven `run_to(t1); run_to(t2)`; holds across the `simlin_model_compile_to_wasm` FFI path and on WORLD3/C-LEARN (two-segment split near midpoint). Facade level: same equality via `DirectBackend`. | Phase 1 Tasks 2, 5 & 6 (blob/FFI); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC2.4** Edge: `runTo(t)` past FINAL_TIME clamps to the end, matching the VM. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `run_to_past_final_time_clamps`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simRunTo(stop*2)` vs `simRunToEnd` and VM). | Blob level: `run_to(stop*2.0)` equals `run_to(stop)` and `Vm::run_to_end`, and exactly `n_chunks` rows are saved (the saved-row exhaustion break clamps, like the VM's ring exhaustion). Facade level: `simRunTo(stop*2)` equals `simRunToEnd` and the VM. | Phase 1 Task 2 (blob); Phase 2 Task 4 (facade). |

## AC3: `reset` parity — two-level

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC3.1** Success: `reset()` then `runToEnd()` (wasm) reproduces compiled-default results, matching the VM. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `reset_then_run_reproduces_defaults`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simReset` then re-run reproduces defaults vs VM). | Blob level: on one instance, `run` (series A); `reset`; `run` (series B); A == B and both equal `Vm::run_to_end` (fresh-then-`reset` VM). Facade level: `simReset` then re-run reproduces defaults, matching the VM. | Phase 1 Task 3 (blob); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC3.2** Success: `reset()` preserves constant overrides set via `setValue` (matching VM reset semantics). | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `reset_preserves_overrides`; rust-integration (FFI): `src/libsimlin/tests/wasm.rs` — `compile_to_wasm_blob_supports_resumable_run` (reset-across-FFI assertion); jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (reset after `simSetValue(const)` preserves override vs VM). | Blob level: `set_value(const, v)`, `run` (A); `reset`, `run` (B); A == B (override survived reset, since `emit_reset` clears the cursor but not the constants region) and both equal the VM run with the same override + a `reset` between; also holds across the FFI compile path. Facade level: `simReset` preserves overrides, matching VM `reset` semantics. | Phase 1 Tasks 3 & 5 (blob/FFI); Phase 2 Task 4 (facade). |

## AC4: By-name reads parity + single allocation

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC4.1** Success: `getSeries(name)` (wasm) equals the VM's series for every variable in the layout, within tolerance. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simGetSeries` for every var vs VM); supported by jest-unit: `src/engine/tests/canonicalize.test.ts` (name→canonical-key resolution) and `src/engine/tests/wasmgen.test.ts` (strided read). | For every variable in the layout, `simGetSeries(name)` (wasm) equals the VM's series within the engine's existing tolerance, with caller names resolved to the layout's canonical keys via the Rust-faithful `canonicalizeIdent`. | Phase 2 Task 4 (parity), Tasks 1 & 2 (functional core). |
| **engine-wasm-sim.AC4.2** Success: `getVarNames()` and `getStepCount()` (wasm) match the VM. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (exact-array equality of `simGetVarNames`; `simGetStepCount` equals VM). | `simGetVarNames()` equals the VM's names exactly — keys from `varOffsets`, filtering **only** `$`-prefixed names (reserved `time`/`dt`/`initial_time`/`final_time` are *kept*, matching the VM's `is_internal_var`), sorted by Unicode code point (Rust byte order). `simGetStepCount()` = `nChunks` = the VM's saved-row count. | Phase 2 Task 4. |
| **engine-wasm-sim.AC4.3** Success: `getSeries` returns one `Float64Array` of length `n_chunks`, read strided from linear memory with no intermediate arrays. | jest-unit: `src/engine/tests/wasmgen.test.ts` (`readStridedSeries` allocates exactly one `Float64Array(nChunks)`, extracts a known column exactly, no intermediates); jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simGetSeries` is `instanceof Float64Array`, `.length === stepCount`). | The pure `readStridedSeries` fills one `Float64Array(nChunks)` via a single strided `DataView.getFloat64` loop (stride `n_slots`, offset `results_offset + (c*n_slots+slot)*8`) with no intermediate arrays; the backend op returns that single `Float64Array` of length `nChunks`. | Phase 2 Task 1 (unit) + Task 4 (integration). |
| **engine-wasm-sim.AC4.4** Failure: `getSeries(unknownName)` errors the same way as the VM path. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simGetSeries('definitely_not_a_var')` throws like the VM); supported by jest-unit: `src/engine/tests/canonicalize.test.ts`. | An unknown name fails to resolve in `varOffsets` and throws the same not-found error as the VM path. | Phase 2 Task 4. |

## AC5: `setValue` (constants) + mid-run + reuse — two-level

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC5.1** Success: `setValue(const, v)` then run (wasm) matches the VM under the same override. | rust-unit (blob, existing): `src/simlin-engine/src/wasmgen/module.rs` — `compile_simulation_set_value_override_matches_vm` (referenced, not duplicated); jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simSetValue(const, v)` then run vs VM). | Blob level: `set_value` on an overridable constant then `run` matches the VM under the same override. Facade level: `simSetValue(const, v)` (rc 0) then run equals the VM under the same override. | Phase 1 Task 4 (references existing blob test); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC5.2** Failure: `setValue(nonConstant, v)` (wasm) throws, matching the VM's constants-only rejection. | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `set_value_nonconstant_returns_error`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (`simSetValue(nonConstant, v)` throws). | Blob level: `set_value` on a non-constant slot (e.g. a stock or computed flow) returns rc `1`, while an overridable constant returns `0` — the blob-level peer of the VM's `BadOverride`. Facade level: rc `1` is turned into a thrown `SimlinError`/`Error` ("cannot set value of '<name>': not a simple constant"), matching the VM's constants-only rejection. | Phase 1 Task 4 (blob); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC5.3** Success: `runTo(t1)`, `setValue(const, v)`, `runTo(t2)` affects only steps after `t1` (incremental, matches VM). | rust-unit (blob): `src/simlin-engine/src/wasmgen/module.rs` — `mid_run_set_value_matches_vm`; rust-integration (FFI): `src/libsimlin/tests/wasm.rs` — `compile_to_wasm_blob_supports_resumable_run`; jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (mid-run `simSetValue` affects only post-`t1` steps vs VM). | Blob level: `run_initials; run_to(t1); set_value(off, v2)` (rc 0); `run_to(stop)`: rows at times ≤ t1 unchanged from baseline, rows after reflect `v2`, matching the VM driven identically; works because overridable constants are re-read from the override region each step (no new mechanism needed). Holds across the FFI compile path. Facade level: same via `DirectBackend`. | Phase 1 Tasks 4 & 5 (blob/FFI); Phase 2 Task 4 (facade). |
| **engine-wasm-sim.AC5.4** Success: `setValue`/`reset`/re-run on an existing wasm `Sim` reuses the same blob instance (no recompile). | rust-unit (blob-level reuse): `src/simlin-engine/src/wasmgen/module.rs` — `reset_then_run_reproduces_defaults`, `reset_preserves_overrides` (all reuse one instantiated module/`Store` across calls); jest-integration (facade): `src/engine/tests/wasm-backend.test.ts` (instance created exactly once and stored on the entry; later `simReset`/`simSetValue`/re-run reuse it). | Blob level: the same instantiated module is driven through `set_value`/`reset`/re-run with no re-instantiation. Facade level: `DirectBackend.simNew` creates the `WebAssembly.Instance` once and stores it on the `'sim'` entry; subsequent ops reuse it (no recompile). | Phase 1 Task 3 (blob reuse); Phase 2 Task 3 (creation-once) + Task 4 (reuse across ops). |

## AC6: `getLinks`/LTM explicit errors

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC6.1** Failure: `getLinks()` on a wasm sim throws an explicit "not supported on the wasm engine" error. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simGetLinks` on a wasm sim throws); reinforced cross-worker by jest-integration: `src/engine/tests/worker-wasm.test.ts`. | `simGetLinks` for a wasm entry throws a clear `Error` ("getLinks is not supported on the wasm engine; use engine:'vm'") — rejected by the `DirectBackend` demux, with no silent fallback. | Phase 2 Task 4; Phase 3 Task 2 (worker reinforcement). |
| **engine-wasm-sim.AC6.2** Failure: `Model.simulate({engine:'wasm', enableLtm:true})` is rejected up front with a clear error. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simNew(modelHandle, true, 'wasm')` throws and creates no sim); cross-worker by jest-integration: `src/engine/tests/worker-wasm.test.ts` (rejection serializes across the worker boundary). | `DirectBackend.simNew` rejects `enableLtm` for the wasm engine *before any compile* with a clear `Error` ("LTM is not supported on the wasm engine; use engine:'vm'") and creates no sim; the same rejection propagates through the worker. | Phase 2 Task 3 (authoritative); Phase 3 Task 2 (worker). |
| **engine-wasm-sim.AC6.3** Success: `Model.run({engine:'wasm'})` succeeds and returns a `Run` with empty `links`. | jest-integration: `src/engine/tests/wasm-model.test.ts` (`model.run({engine:'wasm'})` resolves to a `Run` with `links === []`, no throw). | Because `getRun` gates link-fetching on `ltmEnabled` (a wasm sim never enables LTM), `getLinks` is never called on the wasm sim; `Model.run({engine:'wasm'})` resolves with empty `links`. | Phase 2 Task 5. |

## AC7: Unsupported model → explicit error (no fallback)

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC7.1** Failure: `Model.simulate({engine:'wasm'})` on a wasm-unsupported model throws the explicit `WasmGenError`, never silently using the VM. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (a wasm-unsupported model — a runtime view range `[start:end]` / `ViewRangeDynamic` — makes `simNew(modelHandle, false, 'wasm')` throw a `SimlinError`/`Error`, no VM fallback); reinforced cross-worker by jest-integration: `src/engine/tests/worker-wasm.test.ts`. | `simlin_model_compile_to_wasm` reports the unsupported construct via the error out-ptr; the wrapper throws `SimlinError` and `simNew` surfaces it with no silent VM fallback. The libsimlin FFI surfacing is itself guarded by `src/libsimlin/tests/wasm.rs` `compile_to_wasm_unsupported_model_surfaces_error` (kept passing). | Phase 2 Task 3. |
| **engine-wasm-sim.AC7.2** Success: that same model runs fine via `engine:'vm'`. | jest-integration: `src/engine/tests/wasm-backend.test.ts` (`simNew(modelHandle, false, 'vm')` on the same unsupported-for-wasm model succeeds). | The exact model that fails for wasm still creates and runs a VM sim — confirming the error is wasm-specific, not a broken model. | Phase 2 Task 3. |

## AC8: Browser/worker parity + minimal protocol

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC8.1** Success: through `WorkerBackend`, `engine:'wasm'` produces series matching node `DirectBackend` (and the VM). | jest-integration: `src/engine/tests/worker-wasm.test.ts` (real postMessage loopback via `createTestPair`). | A `WorkerBackend`-driven wasm sim (`simNew(modelHandle, false, 'wasm')` → `simRunToEnd` → `simGetSeries`) produces a series **exactly** equal to a node `DirectBackend` wasm series, and equal to the VM series within the engine's tolerance — through the real request/response serialization, FIFO queue, and `handleResponse`/`deserializeError`. | Phase 3 Task 2. |
| **engine-wasm-sim.AC8.2** Success: the protocol delta is exactly one optional `engine` field on the existing `simNew` message — no new message types/response shapes; `getSeries` still transfers zero-copy. | jest-integration: `src/engine/tests/worker-wasm.test.ts` (transfer + protocol-shape assertions); regression: existing `src/engine/tests/worker-backend.test.ts` / `worker-server.test.ts` pass unchanged; static: `tsc --noEmit` (the widened `simNew` literal typechecks, no new types). | A wasm-sim `simGetSeries` round-trips a `Float64Array` and adds exactly one one-element transfer entry (`[ArrayBuffer]`) — zero-copy preserved; the served `simNew` request carries `engine:'wasm'` with no new message `type` or response shape. `VALID_REQUEST_TYPES`/`isValidRequest` are untouched (field-agnostic). | Phase 3 Tasks 1 & 2. |

## AC9: Node benchmark

| AC | Automated test(s) | Asserts | Owner |
|----|-------------------|---------|-------|
| **engine-wasm-sim.AC9.1** Success: a node benchmark reports warm-median simulation (eval) time for fishbanks, WORLD3, and C-LEARN on both engines, via `Model.simulate({engine})`, with explicit warmup. | jest-unit (always-on, functional core): `src/engine/tests/bench-stats.test.ts` (`median` + `runTimed`/`runTimedAsync` warmup-discard and adaptive-median policy); jest-integration (**`RUN_BENCH`-gated**, not in the default suite): `src/engine/tests/backend-bench.test.ts` (runs all three models on both engines, asserts every model produced a positive finite median on each engine; the in-`runBenchmark` `crossCheck` asserts series agreement before any number is trusted). | Functional core: explicit `warmup` iterations are discarded, then a median over an adaptive measure loop (min/max iters + wall-clock budget) is returned — single-sourced and deterministically unit-tested with injected `now`/`body`. Gated runner: through `Model.simulate({ engine })`, with the blob compile/instantiate and `getRun`/`getSeries` excluded from the clock and `reset()` outside the measured region, each `(model, engine)` yields a warm **median** eval time; the `crossCheck` guards correctness. **The warm-median numbers are the deliverable and are reported in-chat/PR, not committed** (per "no stale benchmark data"); the harness is the only thing checked in. | Phase 4 Task 1 (functional core, always-on) + Task 2 (gated runner). |

---

## Human verification

**None required — all 25 criteria are covered by automated tests.** Every behavioral and
failure-mode AC is asserted by a deterministic Rust or jest test, with VM-vs-wasm parity
judged by the engine's existing comparators and the lone benchmark AC (AC9.1) split into an
always-on unit-tested functional core plus a gated runner whose correctness is guarded by an
in-benchmark cross-check (the only un-asserted-in-CI output is the warm-median *reporting*,
which is an intentionally non-committed, in-chat/PR deliverable rather than a pass/fail gate).
