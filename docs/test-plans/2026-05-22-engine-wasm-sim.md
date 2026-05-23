# @simlin/engine WebAssembly Simulation Backend (WasmSim) — Human Test Plan

This plan covers manual verification of the selectable wasm simulation engine
(`Model.simulate({ engine: 'wasm' })` / `Model.run(..., { engine: 'wasm' })`).
All 25 acceptance criteria already have automated coverage (see
`docs/implementation-plans/2026-05-22-engine-wasm-sim/test-requirements.md`); this
plan exists to (a) re-run the automated gates as a release checklist, (b) drive
the gated/ignored heavy tests that are out of the default suite, and (c)
exercise end-to-end behavior a human can judge — interactive parameter scrubbing
feel and the published VM-vs-wasm benchmark numbers — that the automated suite
intentionally does not gate.

The bytecode VM is the correctness oracle throughout; the wasm path is held to
VM parity within the engine's existing tolerances (it is intentionally not
bit-identical to the VM's libm).

## Prerequisites

- Run `./scripts/dev-init.sh` from the repo root (idempotent).
- A current WASM build of libsimlin so the engine tests can load
  `src/engine/core/libsimlin.wasm`: `pnpm build` (or the WASM build step).
- These default suites pass (the regression baseline):
  - Rust blob parity (default, fast): `cargo test -p simlin-engine`
  - libsimlin FFI: `cargo test -p simlin --test wasm` (the crate is named
    `simlin`, not `libsimlin`)
  - TS engine suite: `pnpm -C src/engine test`
- Model files present (used below):
  - `src/pysimlin/tests/fixtures/teacup.stmx` (scalar Euler, wasm-supported)
  - `default_projects/fishbanks/model.xmile`
  - `test/metasd/WRLD3-03/wrld3-03.mdl`
  - `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` and the sibling `Ref.vdf`

## Phase 1: Run the automated gates (release checklist)

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo test -p simlin-engine` | Green. Includes the blob/VM-parity unit tests in `wasmgen/module.rs` (resumable ABI, reset, set_value, mid-run override) and the always-on corpus `wasm_parity_hook` (every VM-simulated model also runs through the wasm backend and matches). |
| 2 | `cargo test -p simlin --test wasm` | Green. `compile_to_wasm_returns_blob_and_layout`, `compile_to_wasm_blob_supports_resumable_run`, `compile_to_wasm_unsupported_model_surfaces_error`, `compile_to_wasm_null_outputs_error` pass — the FFI surfaces a `SimlinError` (never a panic) and the resumable exports survive the FFI compile path. |
| 3 | `pnpm -C src/engine test` | Green. Includes `wasm-backend.test.ts`, `wasm-model.test.ts`, `worker-wasm.test.ts`, `wasmgen.test.ts`, `canonicalize.test.ts`, `bench-stats.test.ts`. The gated `backend-bench` suite `it.skip`s (correct — heavy run is opt-in). |
| 4 | `pnpm -C src/engine exec tsc --noEmit` (or the package's typecheck script) | No type errors — confirms the widened `simNew(modelHandle, enableLtm, engine?)` literal typechecks with no new worker message types. |

## Phase 2: Drive the heavy gated/ignored automated tests

These are real automated tests excluded from the default suites only for runtime
class. Run them on demand to exercise the wasm path on production-scale models.

| Step | Action | Expected |
|------|--------|----------|
| 5 | `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_wrld3_03_wasm` | Pass. WORLD3 compiles to wasm (a `WasmGenError::Unsupported` here is a hard failure), single-`run` matches the VM element-for-element, and a two-segment `run_to(mid); run_to(stop)` matches the single-`run` series. |
| 6 | `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn_wasm` | Pass. C-LEARN compiles to wasm and clears the same hard 1% `Ref.vdf` gate (with the documented `EXPECTED_VDF_RESIDUAL` carve-out) that the VM clears; the two-segment resumable run matches single-`run`. Takes seconds (non-JIT interpreter on a ~53k-line model) — this is expected. |
| 7 | `RUN_BENCH=1 pnpm -C src/engine exec jest backend-bench` | Pass. Runs fishbanks, WORLD3, C-LEARN on both engines through `Model.simulate({ engine })`. The in-runner `crossCheck` asserts VM-vs-wasm series agreement before any timing is trusted; the test asserts a positive finite warm median per `(model, engine)`. Inspect the printed markdown table for the warm-median eval times and the `wasm/VM` ratio. Per the "no stale benchmark data" rule, these numbers are reported, not committed — record them in the PR/chat, not in a file. |

## End-to-End: Interactive parameter scrubbing (the design's motivating use case)

Purpose: validate that the wasm engine delivers the design's intended workflow —
instantiate a blob once, then re-simulate repeatedly under changing constants and
partial `runTo` advances — and that results stay VM-faithful. This spans engine
selection (AC1), resumable runTo (AC2), reset (AC3), by-name reads (AC4),
constant `setValue` + reuse (AC5).

Setup: a tiny Node script (or a Node REPL) against the built `@simlin/engine`,
configured with `src/engine/core/libsimlin.wasm`, opening
`src/pysimlin/tests/fixtures/teacup.stmx`.

Steps and expected results:
1. Open the project, get the main model. Create two sims for the same model:
   `vm = model.simulate({}, { engine: 'vm' })` and
   `wasm = model.simulate({}, { engine: 'wasm' })`.
   - Expected: both are `Sim` instances; no throw.
2. `await wasm.runToEnd()` and `await vm.runToEnd()`. For each name in
   `await wasm.getVarNames()`, compare `await wasm.getSeries(name)` to the VM's.
   - Expected: arrays equal length; every element within ~1e-9. Confirms AC2.1 +
     AC4.1 on the supported scalar model.
3. Reuse the SAME wasm sim: `await wasm.setValue('room temperature', 40)` then
   `await wasm.runToEnd()`. Read `getSeries('teacup_temperature')`.
   - Expected: the cooling curve changes vs step 2 (warmer asymptote). No
     recompile occurs (the blob instance is reused) — the call returns promptly.
4. Scrub the constant several times in a row on the same sim: for v in
   [50, 60, 70], `await wasm.reset(); await wasm.setValue('room temperature', v);
   await wasm.runToEnd()`. After each, also run a fresh VM sim with the same
   override and compare every variable's series.
   - Expected: each wasm run matches its VM twin within ~1e-9. Confirms reuse
     (AC5.4) + reset-preserves-then-overrides (AC3.2/AC5.1) feel instantaneous
     and stay correct across repeated scrubs.
5. Incremental advance: on a fresh wasm sim, `await wasm.runTo(10)`, read
   `getValue('teacup_temperature')`; then `await wasm.setValue('room temperature',
   30)`; `await wasm.runToEnd()`; read the full `getSeries`.
   - Expected: the value at t=10 equals a VM `runTo(10)` getValue for the stock;
     the post-t=10 trajectory bends toward the new room temperature, and the whole
     series matches a VM driven through the identical sequence within ~1e-9.
     Confirms AC2.2 + AC5.3.

## End-to-End: Default-path safety (no behavior change for existing callers)

Purpose: confirm AC1.3 — code that never passes `engine` is byte-for-byte
unchanged.

Steps:
1. In the same script, `const a = await model.run();` and
   `const b = await model.run({}, { engine: 'vm' });`.
   - Expected: `a.varNames` deep-equals `b.varNames`; every `a.getSeries(name)`
     equals `b.getSeries(name)`.
2. `const c = await model.run({}, { engine: 'wasm' });`
   - Expected: `c` is a `Run`; `c.links` is `[]` (LTM is wasm-unsupported, links
     are empty by design — #626); `c.varNames` equals `a.varNames`; series match
     within ~1e-9. No throw despite getLinks being unsupported on wasm.

## End-to-End: Explicit-error behavior (no silent fallback)

Purpose: confirm AC6/AC7 surface clear errors rather than quietly using the VM.

Steps (script):
1. `await model.simulate({}, { enableLtm: true, engine: 'wasm' })`.
   - Expected: rejects with a message matching /not supported on the wasm engine/i.
2. On a wasm sim, `await wasmSim.getLinks()`.
   - Expected: rejects with /not supported on the wasm engine/i. (The VM sim's
     `getLinks()` resolves to an array.)
3. Build a wasm-UNSUPPORTED model: an XMILE with a dynamic view range
   `summed = SUM(source[lo:hi])` where `lo`/`hi` are scalar auxes (the
   `ViewRangeDynamic` case, GH #612). Open it and call
   `model.simulate({}, { engine: 'wasm' })`.
   - Expected: rejects (the compile error surfaces; no VM fallback).
4. Open the SAME unsupported model and `model.simulate({}, { engine: 'vm' })`,
   then `runToEnd` and `getSeries('summed')`.
   - Expected: succeeds; `summed[0] ≈ 6` (1+2+3). Proves the error in step 3 was
     wasm-specific, not a broken model.

## Human Verification Required

None of the 25 acceptance criteria require human judgment — all are asserted by
deterministic Rust/jest tests. The two items below are human-value checks layered
on top of the automated gates, not uncovered ACs.

| Item | Why a human looks at it | Steps |
|------|-------------------------|-------|
| Scrubbing responsiveness | The design's purpose is fast repeated re-simulation for interactive scrubbing; "fast enough to feel live" is a human judgment the suite does not gate. | After Phase 1, run the scrubbing E2E above and confirm each `reset; setValue; runToEnd` cycle returns without a perceptible stall on teacup (and, optionally, on a larger model). |
| Benchmark numbers | The warm-median VM-vs-wasm eval times are an intentionally non-committed, in-chat/PR deliverable; the automated test only asserts they are positive and finite and that the engines agree. | Run step 7, read the printed table, and record the `wasm/VM` ratios per model in the PR. Sanity-check the direction matches expectations (wasm competitive-to-faster under V8 for the eval region). |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | `wasm-model.test.ts`, `wasm-backend.test.ts` | Phase 1 step 3; Scrubbing E2E step 1–2 |
| AC1.2 | `wasm-model.test.ts` | Phase 1 step 3; Default-path E2E step 2 |
| AC1.3 | `wasm-model.test.ts` + green engine suite | Phase 1 step 3; Default-path E2E step 1 |
| AC2.1 | `module.rs::compile_simulation_run_to_matches_run_and_vm`; `simulate.rs::{simulates_wrld3_03_wasm,simulates_clearn_wasm}` + `wasm_parity_hook`; `wasm-backend.test.ts` | Phase 1 step 1; Phase 2 steps 5–6; Scrubbing E2E step 2 |
| AC2.2 | `module.rs::{compile_simulation_run_to_matches_run_and_vm, run_to_at_save_and_between_save_points}`; `wasm-backend.test.ts` | Phase 1 steps 1,3; Scrubbing E2E step 5 |
| AC2.3 | `module.rs::run_to_segmented_matches_single_and_vm`; `wasm.rs::compile_to_wasm_blob_supports_resumable_run`; real-model twins; `wasm-backend.test.ts` | Phase 1 steps 1–3; Phase 2 steps 5–6 |
| AC2.4 | `module.rs::run_to_past_final_time_clamps`; `wasm-backend.test.ts` | Phase 1 steps 1,3 |
| AC3.1 | `module.rs::reset_then_run_reproduces_defaults`; `wasm-backend.test.ts` | Phase 1 steps 1,3; Scrubbing E2E step 4 |
| AC3.2 | `module.rs::reset_preserves_overrides`; `wasm.rs`; `wasm-backend.test.ts` | Phase 1 steps 1–3; Scrubbing E2E step 4 |
| AC4.1 | `wasm-backend.test.ts` (arrayed E2E suite); `canonicalize.test.ts`; `wasmgen.test.ts` | Phase 1 steps 1,3; Scrubbing E2E step 2 |
| AC4.2 | `wasm-backend.test.ts` | Phase 1 step 3 |
| AC4.3 | `wasmgen.test.ts`; `wasm-backend.test.ts` | Phase 1 step 3 |
| AC4.4 | `wasm-backend.test.ts`; `canonicalize.test.ts` | Phase 1 step 3 |
| AC5.1 | `module.rs::compile_simulation_set_value_override_matches_vm`; `wasm-backend.test.ts` | Phase 1 steps 1,3; Scrubbing E2E step 4 |
| AC5.2 | `module.rs::set_value_nonconstant_returns_error`; `wasm-backend.test.ts` | Phase 1 steps 1,3 |
| AC5.3 | `module.rs::mid_run_set_value_matches_vm`; `wasm.rs`; `wasm-backend.test.ts` | Phase 1 steps 1–3; Scrubbing E2E step 5 |
| AC5.4 | `module.rs` (reuse one Store); `wasm-backend.test.ts` (instance-once white-box) | Phase 1 steps 1,3; Scrubbing E2E steps 3–4 |
| AC6.1 | `wasm-backend.test.ts`; `worker-wasm.test.ts` | Phase 1 step 3; Error E2E step 2 |
| AC6.2 | `wasm-backend.test.ts`, `wasm-model.test.ts`, `worker-wasm.test.ts` | Phase 1 step 3; Error E2E step 1 |
| AC6.3 | `wasm-model.test.ts` | Phase 1 step 3; Default-path E2E step 2 |
| AC7.1 | `wasm-backend.test.ts`, `worker-wasm.test.ts`; `wasm.rs::compile_to_wasm_unsupported_model_surfaces_error` | Phase 1 steps 2–3; Error E2E step 3 |
| AC7.2 | `wasm-backend.test.ts`, `worker-wasm.test.ts` | Phase 1 step 3; Error E2E step 4 |
| AC8.1 | `worker-wasm.test.ts` | Phase 1 step 3 |
| AC8.2 | `worker-wasm.test.ts`; existing `worker-backend.test.ts`/`worker-server.test.ts`; `tsc --noEmit` | Phase 1 steps 3–4 |
| AC9.1 | `bench-stats.test.ts` (always-on); `backend-bench.test.ts` (gated) + `backend-bench.ts` | Phase 2 step 7; Benchmark-numbers human check |
