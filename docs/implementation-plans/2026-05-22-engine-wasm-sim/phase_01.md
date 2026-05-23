# @simlin/engine WebAssembly Simulation Backend — Phase 1: wasm resumable run ABI (Rust)

**Goal:** Extend the emitted wasm "blob" so a run can be advanced incrementally — `run_initials()`, `run_to(time)`, and a resumable `reset()` mirroring the bytecode VM's `run_initials`/`run_to`/`reset` — backed by a persistent step cursor stored in mutable wasm globals. This unblocks the TypeScript engine's `runTo(time)` in Phase 2.

**Architecture:** The `wasmgen` backend lowers one `CompiledSimulation` into a self-contained `wasm-encoder`-built module. Today its `run` driver (`emit_run_simulation`) computes the whole simulation in one self-resetting call, holding its step cursor in **function locals**. This phase promotes the cursor (`saved`, `step_accum`, `did_initials`) to **mutable wasm globals** so it survives across separate exported calls, factors the per-step loop into a shared driver that both `run` and the new `run_to(target)` use, makes `run_initials` idempotent, and extends `reset` to clear the cursor while keeping constant overrides. The bytecode VM (`vm.rs`) is the correctness oracle: every new behavior is parity-tested against the VM within the engine's existing comparator tolerances. No FFI signature changes; only the blob's exported function set grows.

**Tech Stack:** Rust; `wasm-encoder` v0.244 (regular dep, emits the module — hand-driven `Function`/`Instruction` builder, not walrus); DLR-FT `wasm-interpreter` + `checked` (pinned git dev-deps, rev `64cedbba603edfd64cbb6b5a19f5fa34530bb03a`, used to execute the blob in tests); `simlin_engine::test_common::TestProject` (in-memory fixtures, no `file_io`).

**Scope:** Phase 1 of 4 (wasm resumable run ABI). Phases 2–4 (TS node core, browser/worker, benchmark) follow.

**Codebase verified:** 2026-05-22

---

## Acceptance Criteria Coverage

This phase implements and verifies the following at the **blob/VM-parity level** (the public-API/facade realizations of AC2/AC3/AC5 land in Phase 2; this phase proves the blob itself matches the VM):

### engine-wasm-sim.AC2: `runToEnd`/`runTo` parity (resumable)
- **engine-wasm-sim.AC2.1 Success:** `runToEnd()` (wasm) series equal the VM within tolerance.
- **engine-wasm-sim.AC2.2 Success:** `runTo(t)` then `getValue(name)` (wasm) equals the VM's value at `t`.
- **engine-wasm-sim.AC2.3 Success:** segmented `runTo(t1)` then `runTo(t2)` (`t1<t2`) equals a single `runTo(t2)` and the VM.
- **engine-wasm-sim.AC2.4 Edge:** `runTo(t)` past FINAL_TIME clamps to the end, matching the VM.

### engine-wasm-sim.AC3: `reset` parity
- **engine-wasm-sim.AC3.1 Success:** `reset()` then `runToEnd()` (wasm) reproduces the compiled-default results, matching the VM.
- **engine-wasm-sim.AC3.2 Success:** `reset()` preserves constant overrides set via `setValue` (matching VM reset semantics).

### engine-wasm-sim.AC5: `setValue` (constants) + mid-run + reuse
- **engine-wasm-sim.AC5.1 Success:** `setValue(const, v)` then run (wasm) matches the VM under the same override.
- **engine-wasm-sim.AC5.2 Failure:** `setValue(nonConstant, v)` (wasm) throws, matching the VM's constants-only rejection.
- **engine-wasm-sim.AC5.3 Success:** `runTo(t1)`, `setValue(const, v)`, `runTo(t2)` affects only steps after `t1` (incremental semantics, matches VM).
- **engine-wasm-sim.AC5.4 Success:** `setValue`/`reset`/re-run on an existing wasm `Sim` reuses the same blob instance (no recompile).

> **Blob-level scoping note.** AC5.2 at the blob level means the blob's `set_value` export *returns a nonzero error code* for a non-overridable offset (the TS facade turns that into a thrown error in Phase 2). AC5.4 at the blob level means the *same instantiated module* can be driven through `set_value`/`reset`/re-run without re-instantiation (the `DirectBackend` instance-reuse policy is realized in Phase 2). AC2.2's "`getValue`" is read at the blob level as "the value in the live `curr` chunk (base 0) after `run_to(t)`," since the blob has no `getValue` export — the host reads that variable's slot directly from linear-memory base 0 (matching the VM's `get_value_now`, which reads the current chunk).

---

## Background: what exists today (verified)

All paths absolute from repo root `/home/bpowers/src/simlin`.

**The blob's exports (`src/simlin-engine/src/wasmgen/module.rs:1819-1828`), verbatim today:**
```rust
let mut exports = ExportSection::new();
exports.export("run", ExportKind::Func, run_fn_index);
exports.export("set_value", ExportKind::Func, set_value_fn_index);
exports.export("reset", ExportKind::Func, reset_fn_index);
exports.export("clear_values", ExportKind::Func, clear_values_fn_index);
exports.export("memory", ExportKind::Memory, 0);
exports.export("n_slots", ExportKind::Global, G_N_SLOTS);
exports.export("n_chunks", ExportKind::Global, G_N_CHUNKS);
exports.export("results_offset", ExportKind::Global, G_RESULTS_OFFSET);
wasm.section(&exports);
```

**Global index constants (`module.rs:78-81`):** `G_N_SLOTS = 0`, `G_N_CHUNKS = 1`, `G_RESULTS_OFFSET = 2`, `G_USE_PREV_FALLBACK = 3`. The geometry globals are immutable; `G_USE_PREV_FALLBACK` is the **only** mutable global today (declared `mutable: true` at `module.rs:1809-1816`, comment at `module.rs:1807` literally says "the only mutable global"). It is internal (not exported) and gates `PREVIOUS()` fallback: armed to `1` at run start (`module.rs:1047-1048`), cleared to `0` after the first prev-snapshot (`emit_prev_snapshot`, `module.rs:1121-1122`). It is the inverse of the VM's `prev_values_valid`.

**The run driver `emit_run_simulation` (`module.rs:1014-1097`):** its cursor lives in **function locals**, declared `let mut f = Function::new([(3, ValType::I32), (2, ValType::F64)]);` (`module.rs:1022`): `L_SAVED = 0`, `L_STEP_ACCUM = 1`, `L_DST = 2` (i32) and two f64 RK scratch locals `L_SAVED_TIME = 3`, `L_RK_S = 4` (`module.rs:84-86`). Its body:
1. Seed reserved slots into `curr` chunk (base 0): `TIME_OFF=start`, `DT_OFF=dt`, `INITIAL_TIME_OFF=start`, `FINAL_TIME_OFF=stop` (`module.rs:1034-1037`; reserved-slot constants `TIME_OFF=0`/`DT_OFF=1`/`INITIAL_TIME_OFF=2`/`FINAL_TIME_OFF=3` at `module.rs:60-63`).
2. Arm `G_USE_PREV_FALLBACK = 1`, call root `initials` with `module_off=0` (`module.rs:1047-1050`).
3. `emit_copy_chunk(curr -> initial_values_base, n_slots)` — capture `initial_values := curr` once (`module.rs:1056-1061`).
4. `Block $break { Loop $continue { ... } }` (`module.rs:1063-1095`):
   - **Guard:** `if curr[TIME] > stop { br $break }` (`module.rs:1066-1071`).
   - **Step:** `emit_euler_step` / `emit_rk4_step` / `emit_rk2_step` (`module.rs:1076-1084`; helpers at `:1101`, `:1415`, `:1544`; `emit_eval_step` `:1108`, `emit_prev_snapshot` `:1119`).
   - **Save + advance:** `emit_save_advance` (`module.rs:1090`, body `:1129-1210`): save condition `(step_accum == save_every) | (saved==0 & curr[TIME]==start)` (`:1144-1155`); write `curr[slot]` to `results[saved*stride + slot*8]` for every slot (`:1158-1172`); `saved += 1; step_accum = 0` (`:1174-1180`); `if saved >= n_chunks { br 2 }` (`:1183-1186`); copy `next -> curr` (`:1193-1201`); `curr[TIME] += dt` (`:1203-1209`).
   - `Br $continue`.

**`save_every` is computed identically in both backends** as `max(1, round(save_step/dt))`: wasmgen at `module.rs:590`, VM at `vm.rs:653`. `n_chunks`/`n_slots`/`dt`/`method` come from the single `Specs` value on `CompiledSimulation.specs` (`results.rs:22-32`, `n_chunks` derived in `Specs::from` at `results.rs:35-72`), so the two backends cannot disagree on geometry.

**Memory is sized once at compile time and never grows** (`compile_simulation`, `module.rs:423-587`; memory `maximum: None` but no `memory.grow` is ever emitted). Result slab is step-major: `n_chunks` rows of `n_slots` f64, time in column 0, at byte offset `results_offset` (= `results_base = stride*2`, `stride = n_slots*8`).

**Constant overrides** live in a constants-override region (`const_region_base`, f64 by absolute slot offset) plus a validity region (`const_valid_base`, one byte per slot; `module.rs:558-567`), initialized at instantiation from `collect_overridable_defaults` (`module.rs:569`, debug-asserted equal to the VM's overridable set at `:575-585`). The key mid-run mechanism (`lower.rs:1332-1366`): for an overridable offset, generated `AssignConstCurr` code **loads the value from the override region every time it executes** rather than baking in an immediate. **Consequence:** a `set_value` between `run_to` calls already affects only subsequent steps — AC5.3 needs no new mechanism beyond making the run resumable.

- `emit_set_value` (`module.rs:1252-1288`): returns `1` if `offset < 0 || offset >= n_slots`, returns `1` if `valid[offset] == 0` (not overridable), else writes `const_region[offset]` and returns `0`.
- `emit_clear_values` (`module.rs:1313-1324`): straight-line restore of compiled-default constants.
- `emit_reset` (`module.rs:1298-1304`): **today only** sets `G_USE_PREV_FALLBACK = 1`. It deliberately does not touch the constants region.

**The VM oracle (`src/simlin-engine/src/vm.rs`):** persistent cursor fields on `struct Vm` (`vm.rs:239-287`): `curr_chunk` (`:248`), `next_chunk` (`:249`), `did_initials` (`:251`), `step_accum` (`:253`), `prev_values_valid` (`:286`). Initialized in `Vm::new` (`vm.rs:619-634`). Behaviors to mirror:
- `run_to_end` (`vm.rs:638-641`): `self.run_to(self.specs.stop)`.
- `run_to(end)` (`vm.rs:644-856`): calls `run_initials()?` first; loops stepping until `curr[TIME] > end` (strict `>`); `use_prev_fallback: !self.prev_values_valid` per call (`:681`); clamping past FINAL_TIME is implicit (the saved-row exhaustion break stops it).
- `run_initials` (`vm.rs:1079-1148`): idempotent via `if self.did_initials { return Ok(()); }` (`:1080-1082`); seeds reserved slots, runs `eval_initials`, captures `initial_values`, sets `did_initials = true; step_accum = 0`.
- `reset` (`vm.rs:989-1002`): sets `curr_chunk=0; next_chunk=1; did_initials=false; step_accum=0; prev_values_valid=false`, clears scratch — **keeps constant overrides** (does not call `clear_values`, does not restore literals).
- `set_value` (`vm.rs:1026-1047`): rejects a non-constant target with `ErrorCode::BadOverride` ("cannot set value of '{}': not a simple constant"); `is_constant(off)` = `constant_info.contains_key(&off)` (`vm.rs:907-909`).
- `set_value_by_offset` (`vm.rs:1050-1065`), `clear_values` (`vm.rs:1068-1075`).

> **Cursor mapping (important — diverges from the design's framing).** The VM stores results in a chunk-ring of `n_chunks + 2` chunks and advances `curr_chunk`/`next_chunk` through it. The blob has a different layout: one fixed `curr` (base 0), one fixed `next` (base `stride`), and a separate `results` region. So `curr_chunk`/`next_chunk` do **not** translate to the blob. The blob's persistent cursor is `{ saved, step_accum, did_initials }` (today the locals `L_SAVED`/`L_STEP_ACCUM` plus an implicit "initials done" notion); "current time" already persists in `curr[TIME]` linear memory; `prev_values_valid` is already represented by `G_USE_PREV_FALLBACK` (its inverse). Frame the new globals around `saved`/`step_accum`/`did_initials`.

> **`lower.rs` needs little or no change (diverges from the design's hint).** The design lists `lower.rs` as a Phase 1 component for "loop restructuring," but `lower.rs` contains no whole-simulation stepping loop — its loops are per-opcode runtime constructs (`emit_pulse`, array `BeginIter` unrolling). The entire stepping loop is in `module.rs::emit_run_simulation`. The per-opcode `initials`/`flows`/`stocks` programs are already stateless and re-callable. Do not modify `lower.rs` unless a concrete need surfaces during implementation; if so, document why in the commit.

**Test harness (verified templates):**
- Engine-internal `#[cfg(test)]` in `module.rs`: `compile_sim` (`:2205`), `run_artifact_results` (`:2213-2243`), `run_artifact_with_overrides` (`:4089-4144`), `set_value_rc` (`:4149`), `layout_offset` (`:4167`), `vm_results_with_override` (`:4202`), and the existing override parity test `compile_simulation_set_value_override_matches_vm` (`:4220`, already covers AC5.1).
- DLR-FT execution helper `run_wasm_results(wasm, layout) -> Vec<f64>` (`src/simlin-engine/tests/test_helpers.rs:285`): `validate(wasm)` → `Store::new(())` → `module_instantiate` → `instance_export("run").as_func()` → `store.invoke_simple_typed::<(), ()>(run, ())` → read `n_chunks*n_slots` f64 from `memory` at `results_offset` via `mem_access_mut_slice`. Typed invocation supports args: `store.invoke_simple_typed::<(f64,), ()>(run_to, (target,))`, `::<(), ()>` for `run_initials`/`reset`, `::<(i32, f64), i32>` for `set_value`.
- Single-variable stride read `run_and_stride(wasm, layout, off)` (`src/libsimlin/tests/wasm.rs:281`): `f64 @ results_offset + (c*n_slots + off)*8` for `c in 0..n_chunks`.
- Comparators (same tolerance for wasm-vs-VM and VM-vs-reference): `ensure_results` / `ensure_results_excluding` (`test_helpers.rs:66`/`:75`).
- The fast in-memory fixture `simple_model()` (`src/libsimlin/tests/wasm.rs:28`): `inflow_rate = 2` (constant), `level` stock fed by `inflow = inflow_rate`, sim 0..10 dt 1 (11 chunks). `TestProject` builder in `src/simlin-engine/src/test_common.rs` (`new`/`with_sim_time`/`scalar_const`/`flow`/`stock`/`build_datamodel`/`compile_incremental`).
- libsimlin FFI: `simlin_model_compile_to_wasm` signature at `src/libsimlin/src/model.rs:117` (six args, returns void, malloc-return convention — **must not change**); VM-side resumable FFI for the oracle: `simlin_sim_run_to` (`simulation.rs:204`), `simlin_sim_run_to_end` (`:233`), `simlin_sim_reset` (`:303`), `simlin_sim_set_value` (`:468`), `simlin_sim_get_series` (`:817`).

**Test-budget rules (must follow):** individual unit tests complete in a few seconds on a debug build; `cargo test --workspace` is under a 3-minute cap. Use tiny in-memory fixtures, not large model files. Whole-model parity twins are `#[ignore]`d and run under `--release`. The fast tasks below use `TestProject`/`simple_model` (no `file_io`); only Task 6 touches the `#[ignore]`d whole-model tests.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
Subcomponent A: the resumable run ABI in the blob, plus engine-level VM-parity tests, all in `src/simlin-engine/src/wasmgen/module.rs` and `src/simlin-engine/tests/test_helpers.rs`.

<!-- START_TASK_1 -->
### Task 1: Resumable run ABI core (mutable-global cursor + `run_initials`/`run_to` exports + resumable `reset`)

**Verifies:** engine-wasm-sim.AC2.1, engine-wasm-sim.AC2.2 (the foundation the rest build on)

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (globals `:78-81`/`:1798-1817`, run driver `:1014-1097`, save/advance `:1129-1210`, reset `:1298-1304`, type section `:1751-1766`, function indices `:1782-1785`, exports `:1819-1828`)
- Add test helper + test: `src/simlin-engine/tests/test_helpers.rs` (near `run_wasm_results` `:285`) and a `#[cfg(test)]` test in `module.rs` (near `:4220`)

**Implementation:**

1. **Add three mutable i32 globals** for the persistent cursor. Add index constants after `G_USE_PREV_FALLBACK = 3` (`module.rs:78-81`):
   - `G_SAVED = 4` — saved-row counter (was local `L_SAVED`).
   - `G_STEP_ACCUM = 5` — save-cadence accumulator (was local `L_STEP_ACCUM`).
   - `G_DID_INITIALS = 6` — `0` until initials have run (the blob analogue of `Vm::did_initials`).

   Declare each in the global section (`module.rs:1809-1816` pattern) initialized to `0`:
   ```rust
   globals.global(
       GlobalType { val_type: ValType::I32, mutable: true, shared: false },
       &ConstExpr::i32_const(0),
   );
   ```
   Update the stale "the only mutable global" comment (`module.rs:1807`) to reflect that the cursor globals are now also mutable. These three are **internal** — do not add them to the export section.

2. **Factor the stepping loop into a `run_to(target: f64)` driver** that reads/writes the cursor from the new globals instead of locals:
   - New emitter `emit_run_to` builds a function of type `(f64) -> ()` (its f64 param is the local-0 `target`). Body: `call run_initials` (idempotent), then the `Block $break { Loop $continue { ... } }` from today's `emit_run_simulation:1063-1095`, with two changes: (a) the guard compares `curr[TIME] > target` (the param) instead of `> stop`; (b) `emit_save_advance` reads/writes `G_SAVED`/`G_STEP_ACCUM` via `GlobalGet`/`GlobalSet` instead of `LocalGet`/`LocalSet` on `L_SAVED`/`L_STEP_ACCUM`. `L_DST` and the RK scratch f64 locals stay function-local (per-step transients). Keep the `if saved >= n_chunks { br 2 }` exhaustion break (now reading `G_SAVED`) — this is what makes `run_to(target)` past FINAL_TIME clamp to the end (AC2.4), exactly like the VM's ring exhaustion.
   - New emitter `emit_run_initials` builds a `() -> ()` function: `if G_DID_INITIALS != 0 { return }` (idempotency, mirroring `vm.rs:1080-1082`); else seed the reserved slots (`module.rs:1034-1037` logic), arm `G_USE_PREV_FALLBACK = 1`, call root `initials` (`module_off=0`), `emit_copy_chunk(curr -> initial_values_base)` (`:1056-1061`), set `G_SAVED = 0`, `G_STEP_ACCUM = 0`, `G_DID_INITIALS = 1`. (It does **not** save chunk 0; the first save happens in `run_to`'s loop, matching today's behavior and the VM.)
   - **Re-express `run()` to delegate** (DRY, mandated): emit `run` as `call reset; f64.const <stop>; call run_to`, so there is exactly one stepping-loop implementation shared by `run` and `run_to`. **Invariant (the linchpin): `run()` must produce a full from-`t0` simulation on *every* call to a reused instance.** The delegation path satisfies this for free — `reset` clears `G_DID_INITIALS`/`G_SAVED`/`G_STEP_ACCUM` and re-arms `G_USE_PREV_FALLBACK=1`, then `run_to`→`run_initials` (which no longer short-circuits, since `reset` cleared `G_DID_INITIALS`) re-seeds the reserved time slots and runs initials. Use this path. A fallback (keep `emit_run_simulation` as `run`'s body, switched to the global cursor) is acceptable **only if** the encoder makes delegation infeasible — and then `run`'s body MUST, at its top, itself reset `G_DID_INITIALS`/`G_SAVED`/`G_STEP_ACCUM` to 0 and re-arm `G_USE_PREV_FALLBACK=1`, because the now-idempotent `run_initials` (`if G_DID_INITIALS != 0 return`) would otherwise silently skip initials on a second `run`, double-count saves, or resume from stale cursor state. Either way, the existing `wasm_parity_hook` corpus parity (`run` vs VM) plus the new triple-agreement test below catch a faithless re-expression; document the chosen path in the commit message.

3. **Extend `emit_reset`** (`module.rs:1298-1304`) to clear the cursor: set `G_SAVED = 0`, `G_STEP_ACCUM = 0`, `G_DID_INITIALS = 0`, and keep `G_USE_PREV_FALLBACK = 1`. Do **not** touch the constants-override region (overrides survive reset — AC3.2), mirroring `vm.rs:989-1002`.

4. **Register the two new functions and exports:**
   - Add a function type `(f64) -> ()` to the type section (`module.rs:1751-1766`) for `run_to`; reuse the existing `() -> ()` type (`TYPE_RUN_FN`) for `run_initials`.
   - Assign `run_to_fn_index` and `run_initials_fn_index` in the function-index assignment (`module.rs:1782-1785`); emit their bodies. Note `run`'s delegating body references `run_to_fn_index`/`reset_fn_index`, so assign indices before emitting `run`'s body (the existing flow declares indices before bodies).
   - Add to the export section (`module.rs:1819-1828`):
     ```rust
     exports.export("run_to", ExportKind::Func, run_to_fn_index);
     exports.export("run_initials", ExportKind::Func, run_initials_fn_index);
     ```

**Testing:**
Add a DLR-FT helper that drives the resumable exports, then a parity test:
- Helper `run_wasm_results_segmented(wasm, &layout, targets: &[f64]) -> Vec<f64>` in `test_helpers.rs` (sibling to `run_wasm_results:285`): instantiate, `invoke_simple_typed::<(), ()>(run_initials, ())`, then for each `t` in `targets` `invoke_simple_typed::<(f64,), ()>(run_to, (t,))`, then read the whole slab (`n_chunks*n_slots` f64 at `results_offset`).
- Test `compile_simulation_run_to_matches_run_and_vm` (in `module.rs` `#[cfg(test)]`): build a small `TestProject` (stock + constant flow, ~11 chunks), compile to artifact, and assert all three agree within `ensure_results`-equivalent tolerance: the full series from (a) the `run` export, (b) `run_initials` + `run_to(stop)`, and (c) the VM (`Vm::run_to_end` over the same `CompiledSimulation`). This proves the re-expressed `run` is faithful and that `run_initials`+`run_to(stop)` equals the VM (AC2.1), and that the per-`t` last-saved value equals the VM at `t` (AC2.2 foundation).

Tests must verify:
- engine-wasm-sim.AC2.1: wasm `run_to(stop)` (and `run`) full series equals the VM within tolerance.
- engine-wasm-sim.AC2.2: the value at the chunk for time `t` after `run_to(t)` equals the VM's value at `t` (strided read).

**Verification:**
Run: `cargo test -p simlin-engine wasmgen::module 2>&1 | tail -30`
Expected: the new test passes; all pre-existing `wasmgen` tests still pass (confirms the `run` re-expression is faithful).
Run: `cargo test -p simlin-engine 2>&1 | tail -20`
Expected: green.

**Commit:** `engine: add resumable run_to/run_initials ABI to wasmgen blob`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Segmented + clamp parity (`run_to` resumes correctly; past-FINAL_TIME clamps)

**Verifies:** engine-wasm-sim.AC2.2, engine-wasm-sim.AC2.3, engine-wasm-sim.AC2.4

**Files:**
- Add tests: `src/simlin-engine/src/wasmgen/module.rs` (`#[cfg(test)]`, near Task 1's test)
- Possible fix: `src/simlin-engine/src/wasmgen/module.rs` (only if a parity gap surfaces)

**Implementation:**
No new production code is expected — these tests exercise Task 1's driver. If any assertion fails, the cursor/guard logic from Task 1 is wrong; fix it here and explain in the commit.

**Testing:**
Use a fixture with several save points (e.g. `simple_model`-style, sim 0..10 dt 1, save_step 1 → 11 chunks) so segment boundaries land on and between save points. Drive the VM oracle with the matching `Vm::run_to` segments.
- `run_to_segmented_matches_single_and_vm`: choose `t1 < t2 < stop` (e.g. 3 and 7). Assert `run_initials; run_to(t1); run_to(t2)` produces a slab whose first rows (≤ t2) equal both `run_initials; run_to(t2)` (single) and the VM driven `run_to(t1); run_to(t2)`. (AC2.3)
- `run_to_at_save_and_between_save_points`: assert the saved-row count after `run_to(t)` matches the VM's saved-row count for the same `t`, for `t` exactly on a save point and `t` between save points. (AC2.2)
- `run_to_past_final_time_clamps`: `run_to(stop * 2.0)` equals `run_to(stop)` and `Vm::run_to_end`, and exactly `n_chunks` rows are saved. (AC2.4)

**Verification:**
Run: `cargo test -p simlin-engine wasmgen::module 2>&1 | tail -20`
Expected: all three new tests pass.

**Commit:** `engine: parity-test segmented and clamped wasm run_to vs VM`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `reset` parity (defaults reproduced; overrides preserved; instance reused)

**Verifies:** engine-wasm-sim.AC3.1, engine-wasm-sim.AC3.2, engine-wasm-sim.AC5.4 (blob-level instance reuse)

**Files:**
- Add tests: `src/simlin-engine/src/wasmgen/module.rs` (`#[cfg(test)]`)
- Possible fix: `src/simlin-engine/src/wasmgen/module.rs` (only if a gap surfaces in `emit_reset`)

**Implementation:**
No new production code expected beyond Task 1's `emit_reset` extension. Fix here only if a test reveals a gap.

**Testing:**
All tests reuse a **single instantiated module** (one `Store`/instance) across calls, which is itself the blob-level demonstration of AC5.4 (no re-instantiation needed between `reset`/`set_value`/re-run).
- `reset_then_run_reproduces_defaults`: on one instance, `run` → capture series A; `reset` → `run` → capture series B; assert A == B and both equal `Vm::run_to_end` (with a fresh-then-`reset` VM). (AC3.1)
- `reset_preserves_overrides`: on one instance, `set_value(const_offset, v)` (use `layout_offset:4167`/`set_value_rc:4149` helpers), `run` → series A; `reset` → `run` → series B; assert A == B (the override survived reset) and both equal the VM run with the same override applied and a `reset` in between. (AC3.2, AC5.4)

**Verification:**
Run: `cargo test -p simlin-engine wasmgen::module 2>&1 | tail -20`
Expected: both new tests pass.

**Commit:** `engine: parity-test wasm reset (defaults + override-preserving) vs VM`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Mid-run `set_value` parity + non-constant rejection

**Verifies:** engine-wasm-sim.AC5.1 (reference existing), engine-wasm-sim.AC5.2 (blob-level), engine-wasm-sim.AC5.3

**Files:**
- Add tests: `src/simlin-engine/src/wasmgen/module.rs` (`#[cfg(test)]`)

**Implementation:**
No new production code — `emit_set_value` (`module.rs:1252-1288`) already returns nonzero for non-overridable offsets, and overridable constants are re-read each step (`lower.rs:1332-1366`), so a mid-run override already affects only later steps. These tests lock in that behavior against the VM.

**Testing:**
- `mid_run_set_value_matches_vm`: on one instance with a constant `inflow_rate`, `run_initials; run_to(t1)` (e.g. t1=5); `set_value(offset_of(inflow_rate), v2)` (assert it returns `0`); `run_to(stop)`. Compare the slab to the VM driven identically (`Vm::run_to(t1)`, `Vm::set_value("inflow_rate", v2)`, `Vm::run_to(stop)`). Assert rows at times `≤ t1` are unchanged from a no-override baseline and rows after reflect `v2`. (AC5.3)
- `set_value_nonconstant_returns_error`: pick a non-constant variable's slot (e.g. the stock `level` or the computed `inflow`) and assert `set_value(that_offset, v)` returns `1`; assert `set_value(offset_of(inflow_rate), v)` returns `0`. This is the blob-level peer of the VM's `BadOverride` rejection (`vm.rs:1036-1044`). (AC5.2)
- AC5.1 is already covered by `compile_simulation_set_value_override_matches_vm` (`module.rs:4220`); reference it in a comment rather than duplicating.

**Verification:**
Run: `cargo test -p simlin-engine wasmgen::module 2>&1 | tail -20`
Expected: both new tests pass; `compile_simulation_set_value_override_matches_vm` still passes.

**Commit:** `engine: parity-test mid-run wasm set_value and non-constant rejection`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (task 5) -->
Subcomponent B: prove the new exports survive the libsimlin FFI compile path.

<!-- START_TASK_5 -->
### Task 5: libsimlin FFI — resumable exports on the compiled blob

**Verifies:** engine-wasm-sim.AC2.3, engine-wasm-sim.AC5.3 (across the `simlin_model_compile_to_wasm` path)

**Files:**
- Add test: `src/libsimlin/tests/wasm.rs` (near `compile_to_wasm_returns_blob_and_layout:84`, reuse `simple_model:28`, `parse_layout:48`, `run_and_stride:281`, `vm_series:312`)

**Implementation:**
No production change. `simlin_model_compile_to_wasm` (`src/libsimlin/src/model.rs:117`) returns the blob bytes + serialized layout; the resumable ABI is reached via the blob's own exports, so the FFI signature is unchanged. This test asserts the compiled-via-FFI blob carries and honors `run_initials`/`run_to`/`reset`.

**Testing:**
- `compile_to_wasm_blob_supports_resumable_run`: call `simlin_model_compile_to_wasm(simple_model())`, `validate` the blob, parse the layout, instantiate under DLR-FT, and drive `run_initials` → `run_to(t1)` → `set_value(layout offset of inflow_rate, v)` → `run_to(stop)` → read `level`'s strided series. Drive the VM oracle through the **FFI** the same way: `simlin_sim_new` → `simlin_sim_run_to(t1)` → `simlin_sim_set_value("inflow_rate", v)` → `simlin_sim_run_to_end` → `simlin_sim_get_series("level")`. Assert the series match within tolerance. (AC2.3, AC5.3 across the FFI compile path)
- Add a `reset`-across-FFI assertion in the same test (or a sibling): after the above, call the blob's `reset`, re-run, and confirm a fresh full run reproduces the override-applied defaults — peer of `simlin_sim_reset` (`simulation.rs:303`).

Also assert the blob still has the original exports (`run`, `set_value`, `reset`, `clear_values`, `memory`, `n_slots`, `n_chunks`, `results_offset`) so the export-set growth is purely additive.

**Verification:**
Run: `cargo test -p libsimlin --test wasm 2>&1 | tail -20`
Expected: the new test passes; `compile_to_wasm_returns_blob_and_layout` and `compile_to_wasm_unsupported_model_surfaces_error` still pass (FFI signature unchanged).

**Commit:** `libsimlin: exercise resumable wasm exports across the FFI compile path`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (task 6) -->
Subcomponent C: real-model segmented coverage without breaking the test-time budget.

<!-- START_TASK_6 -->
### Task 6: Real-model segmented `run_to` parity in the `#[ignore]`d whole-model twins

**Verifies:** engine-wasm-sim.AC2.1, engine-wasm-sim.AC2.3 (on WORLD3 and C-LEARN)

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (`simulates_wrld3_03_wasm:1867`, `simulates_clearn_wasm:2085`)
- Add helper: `src/simlin-engine/tests/test_helpers.rs` (reuse the `run_wasm_results_segmented` added in Task 1, or add a thin `wasm_results_for_segmented` mirroring `wasm_results_for:224`)

**Implementation:**
These two tests are `#[ignore]`d and run under `--release`, so adding a segmented pass does not affect the 3-minute debug `cargo test` budget. Extend each twin: in addition to the existing single-`run` parity check, run the same model's blob through a two-segment `run_to` (split near the midpoint of `[start, stop]`) and assert the resulting series equals the single-`run` wasm series (and therefore the existing oracle). Do **not** add a segmented pass to the per-corpus hook (`wasm_parity_hook:1047`) — that would double wasm work across the whole corpus and risk the budget.

**Testing:**
- Extend `simulates_wrld3_03_wasm`: assert `run_wasm_results_segmented(wasm, layout, &[mid, stop])` equals the existing `run`-export wasm series (which is already compared to the VM via `ensure_results`). (AC2.1, AC2.3 on WORLD3)
- Extend `simulates_clearn_wasm`: same segmented-equals-single assertion (the single run is already checked against the VDF oracle via `ensure_vdf_results_excluding`). (AC2.1, AC2.3 on C-LEARN)

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_wrld3_03_wasm simulates_clearn_wasm 2>&1 | tail -30`
Expected: both ignored tests pass (single and segmented agree, and the single still matches its oracle).
Run (budget guard): `cargo test -p simlin-engine 2>&1 | tail -5`
Expected: green and well under budget (the segmented pass is `#[ignore]`d, so the default run is unaffected).

**Commit:** `engine: segmented run_to parity for WORLD3 and C-LEARN wasm twins`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase 1 Done When

- The blob exports `run_to(f64)->()` and `run_initials()->()` in addition to the unchanged `run`/`set_value`/`reset`/`clear_values`/`memory`/`n_slots`/`n_chunks`/`results_offset`; the cursor (`saved`/`step_accum`/`did_initials`) is held in mutable wasm globals; `reset` clears the cursor while keeping constant overrides.
- Segmented `run_to`, clamped `run_to`, `reset` (defaults + override-preserving), and mid-run `set_value` all match the VM within the engine's existing comparator tolerances (Tasks 1–4), including across the libsimlin FFI compile path (Task 5) and on the real WORLD3/C-LEARN models (Task 6).
- `simlin_model_compile_to_wasm`'s signature and the `WasmLayout` wire format are unchanged (the export-set growth is purely additive).
- `cargo test -p simlin-engine` and `cargo test -p libsimlin --test wasm` pass; the default (debug) `cargo test` stays within the 3-minute budget (heavy real-model coverage stays `#[ignore]`d).
