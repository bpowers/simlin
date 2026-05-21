# WebAssembly Simulation Backend — Phase 7: Modules + host interface (FFI, layout, override/reset)

**Goal:** Run submodels in the blob (`EvalModule`/`LoadModuleInput`), give the blob `set_value`/`reset` override semantics matching the VM, and surface the blob plus its name→offset layout through a libsimlin FFI so a host can drive the model and read one variable's series by name.

**Architecture:** Each unique module instance `(model, input_set)` in `CompiledSimulation.modules` becomes its own set of three wasm functions (initials/flows/stocks), each taking a runtime `module_off: i32` (a shared `CompiledModule` may run at several base offsets) plus its `n_inputs` f64 inputs as parameters. `EvalModule { id }` resolves the declaration to a child `ModuleKey`, computes `child_module_off = module_off + decl.off`, and emits a `call` to the child's function for the current phase, passing the popped inputs as args; `LoadModuleInput { input }` reads the corresponding input parameter. Overridable constants are sourced from a mutable constants region (initialized to defaults) so an exported `set_value(offset, val)` + `reset()` reproduce the VM's "override a constant, reset, re-run from t0." The `WasmLayout` (already built in Phase 1) is serialized and returned alongside the blob through `simlin_model_compile_to_wasm`, and a host reads one variable's `n_chunks`-long series by striding the results region.

**Tech Stack:** `wasm-encoder` (multi-function modules, `call`, mutable globals/regions, exported functions); the VM `EvalModule`/`LoadModuleInput`/`set_value`/`reset` as spec; libsimlin's malloc-return convention.

**Scope:** Phase 7 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` at those tests' existing tolerances.

### wasm-backend.AC4
- **wasm-backend.AC4.1 Success:** The blob exports `n_slots`/`n_chunks`/`results_offset` and writes step-major snapshots; a host locates and strides the results with no external metadata.
- **wasm-backend.AC4.2 Success:** Reading one variable's series via the name→offset layout copies only that variable's `n_chunks` values (never the whole `n_chunks × n_slots` slab) and equals the VM's series for that variable.

### wasm-backend.AC5
- **wasm-backend.AC5.1 Success:** Overriding a constant via `set_value`, then `reset`, then `run`, yields the same series the VM produces under the same override (matching `simlin_sim_set_value`/`reset` semantics).
- **wasm-backend.AC5.2 Success:** `reset` with no override restores the compiled-default results.

### wasm-backend.AC6
- **wasm-backend.AC6.1 Success:** `simlin_model_compile_to_wasm` returns a valid wasm blob plus the name→offset layout via the malloc-return convention; both buffers are freeable with `simlin_free`; it works before any `SimlinSim` exists.
- **wasm-backend.AC6.2 Failure:** A model that cannot be compiled to wasm surfaces a `SimlinError` rather than panicking across the FFI boundary.

---

## Notes for the implementer (read first)

- **Opcodes** (`bytecode.rs`): `LoadModuleInput { input: ModuleInputOffset(u16) }` (`vm.rs:1376-1378`: push `module_inputs[input]`); `EvalModule { id: ModuleId(u16), n_inputs: u8 }` (`vm.rs:1379-1443`). **There is no `ModuleInput` opcode** (`Expr::ModuleInput` lowers to `LoadModuleInput`). `EvalModule` stack effect `(n_inputs, 0)`; `LoadModuleInput` `(0,1)`.
- **`ModuleDeclaration`** (`bytecode.rs:1505-1514`), the element type of `ByteCodeContext.modules`: `{ model_name: Ident<Canonical>, input_set: BTreeSet<Ident<Canonical>>, off: usize }`.
- **`EvalModule` VM dispatch** (`vm.rs:1379-1443`): pop `n_inputs` values into `module_inputs` **in reverse** (`for j in (0..n_inputs).rev() { module_inputs[j] = pop() }`); `child_module_off = module_off + context.modules[id].off`; resolve the child via `make_module_key(&decl.model_name, &decl.input_set)` (`vm.rs:27-32`) → the child `CompiledModule`; recurse phase-aware (Initials→child initials, Flows/Stocks→child `eval` with `part`). **The wasm backend does not need `CompiledSlicedSimulation`/`child_targets`** — resolve `EvalModule` to the child's wasm function index directly from `CompiledSimulation.modules` keyed by `make_module_key`.
- **Single slab**: the root `n_slots` includes all nested module slots; a child reads/writes at `module_off + off` (`LoadVar`/`AssignCurr`/`AssignNext`), while `LoadGlobalVar` is absolute (TIME/DT/INITIAL_TIME/FINAL_TIME). This is the addressing the emitter has used since Phase 1 (`module_off` is a function parameter).
- **Inputs as wasm params (clean approach):** each instance's three functions have signature `(module_off: i32, in_0: f64, …, in_{k-1}: f64) -> ()` where `k = n_inputs` for that `(model, input_set)`. `LoadModuleInput { input }` → `local.get(input + 1)` (param 0 is `module_off`). `EvalModule { id, n_inputs }`: pop the `n_inputs` operands into scratch locals (reverse, matching the VM), then push `child_module_off` (= `local.get(module_off) + decl.off`) followed by the input locals in order, and `call` the child's function for the current `StepPart`. (The root's functions are `(i32)->()`, 0 inputs.) This avoids any module-inputs memory scratch.
- **Phase-aware child function resolution:** build a map `(ModuleKey, StepPart) → wasm function index` during assembly; an `EvalModule` site in the initials/flows/stocks program calls the child's initials/flows/stocks function respectively (the `StepPart` is compile-time per program). The module instantiation graph is acyclic, so the wasm call graph is well-founded.
- **Per-instance side tables:** generalize Phase 3's GF directory and Phase 5's temp region to **per-instance** `ByteCodeContext`s — each instance has its own `graphical_functions`/`static_views`/`temp_offsets`/`temp_total_size`. The temp regions can be disjoint per instance (sum the sizes) or shared with care; disjoint is simplest. Generalize Phase 4's stock-offset collection to recurse through `EvalModule` declarations adding `decl.off` cumulatively (mirroring `collect_stock_offsets`, `vm.rs:512-543`) so the RK stage math covers nested stocks.
- **`set_value`/`reset`** (`vm.rs:976-1062`): `set_value(off, v)` is valid only when `is_constant_offset(off)` (`vm.rs:167`) — an offset with an `AssignConstCurr` in the **flows** phase (`cached_constant_info`, `collect_constant_info` `vm.rs:426-507`). The VM mutates the bytecode literal(s) at those locations (so flows re-assigns the override each step) and the override **persists across `reset`** (which only re-runs initials). `clear_values` restores defaults. The libsimlin wrappers `simlin_sim_set_value`/`simlin_sim_reset`/`simlin_sim_clear_values` (`simulation.rs:303-556`) record overrides in `SimState.overrides` and re-apply on reset.
- **`Results` has no `get_series`**; by-name retrieval strides the slab: `Vm::get_series(ident)` (`vm.rs:1140-1160`) does `off = offsets[ident]; for c in 0..n_steps { data[c*n_slots + off] }`. The host mirrors this over the blob's results region using `WasmLayout.var_offsets` — copying only `n_chunks` values.
- **libsimlin** (`src/libsimlin/`): `write_bytes_to_ffi_output` (`model.rs:65-86`), `simlin_malloc`/`simlin_free` (`memory.rs:30-71`), the `out_error: *mut *mut SimlinError` + `clear_out_error`/`store_error`/`store_anyhow_error` convention (`lib.rs:384-421`), `require_model` (`lib.rs:512`). The current POC `simlin_model_compile_to_wasm` (`model.rs:101-149`) returns only the blob; this phase changes it to also return the serialized layout.
- **Memory-layout addition:** a constants override region (a mutable region holding, per overridable offset, its current value, initialized to the compiled default). Append to the layout; grow `pages`.
- `pub(crate)`/`pub` latitude per the repo owner. TDD; corpus tests gated on `file_io`.

---

<!-- START_SUBCOMPONENT_A (tasks 1) -->
<!-- START_TASK_1 -->
### Task 1: Per-instance module functions + EvalModule/LoadModuleInput

**Verifies:** wasm-backend.AC1.1.

**Files:** Modify `wasmgen/module.rs` (emit one function-triple per instance; the `(ModuleKey,StepPart)→fn index` map; per-instance GF directory + temp regions; recursive stock-offset collection), `wasmgen/lower.rs` (`EvalModule`/`LoadModuleInput` arms). Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
1. Enumerate `sim.modules` (every `(model, input_set)` instance). For each, emit initials/flows/stocks functions with signature `(module_off: i32, in_0..in_{k-1}: f64) -> ()` (k = the instance's module-input count). Record `(ModuleKey, StepPart) → fn_index`.
2. `LoadModuleInput { input }` → `local.get(input + 1)`.
3. `EvalModule { id, n_inputs }`: pop the `n_inputs` operands into scratch f64 locals (reverse); resolve `decl = current_instance.context.modules[id]`, `child_key = make_module_key(&decl.model_name, &decl.input_set)`; push `child_module_off = (local.get module_off) + (decl.off as i32)`; push the input locals in order; `call (child_key, current_part)`.
4. The root `run` calls the root's initials/flows/stocks with `module_off=0` and no inputs. Generalize GF directory + temp regions to per-instance, and the RK stock-offset list to recurse through `EvalModule` (adding `decl.off`).

**Testing:** module-bearing models (a model instantiating a submodel; SMOOTH/DELAY stdlib macros expand to implicit module stocks — exercise one) and the same `(model,input_set)` instantiated at two offsets: assert wasm matches the VM. Confirm `LoadModuleInput` reads the right input.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen per-instance module functions (EvalModule/LoadModuleInput)`
<!-- END_TASK_1 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 2-4) -->
<!-- START_TASK_2 -->
### Task 2: `set_value` / `reset` override mechanism

**Verifies:** wasm-backend.AC5.1, wasm-backend.AC5.2.

**Files:** Modify `wasmgen/module.rs` (constants region + `set_value`/`reset` exports), `wasmgen/lower.rs` (source overridable constants from the region). Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
1. Identify the **full set** of overridable offsets: `CompiledSimulation::is_constant_offset(off)` (`vm.rs:167`, `pub fn`) only answers one offset at a time, and the set itself lives in the private `cached_constant_info` map — so expose its keys (widen `cached_constant_info`'s visibility, or add a `pub(crate) fn constant_offsets(&self) -> impl Iterator<Item = usize>` accessor on `CompiledSimulation`) and initialize the constants region from that key set. Add a constants override region holding each overridable offset's current value, initialized (data segment or init code) to the compiled-default literal.
2. Redirect the value source for the overridable constant-assignment pattern (`LoadConstant{id}; AssignCurr{off}` where `off` is a constant offset, the un-fused form of `AssignConstCurr`): instead of `f64.const literal`, emit `f64.load const_region[off]`. This makes the override take effect every flows step, exactly like the VM mutating the literal.
3. Export `set_value(offset: i32, val: f64) -> i32` (return 0 ok / nonzero if `offset` is not overridable — validate against the overridable set) writing `const_region[offset]=val`; and `reset()` resetting run state (chunk/step counters, `use_prev_fallback=1`, `did_initials`-equivalent) **without** clearing the constants region (overrides persist across reset, matching the VM). Optionally `clear_values()` to restore defaults. The next `run` re-runs initials and the loop, picking up the override.

**Testing:**
- AC5.1: `set_value(off_of_a_constant, v); reset(); run();` and compare the full series to the VM run with `vm.set_value(ident, v)` under the same override.
- AC5.2: `reset(); run()` with no override reproduces the compiled-default series.
- `set_value` on a non-constant offset returns the error code (no write).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen blob set_value/reset override semantics`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: libsimlin FFI returning blob + layout; by-name series retrieval

**Verifies:** wasm-backend.AC4.1, wasm-backend.AC4.2, wasm-backend.AC6.1, wasm-backend.AC6.2.

**Files:** Modify `src/libsimlin/src/model.rs` (`simlin_model_compile_to_wasm` to also return the serialized layout); add a `WasmLayout` serializer (in `wasmgen` or libsimlin). Test: a Rust integration test in `src/libsimlin/` (and/or a `wasmgen` test for the by-name read).

**Implementation:**
1. Add a `WasmLayout` serializer: a length-prefixed encoding — `n_slots`, `n_chunks`, `results_offset` (as u64 LE), then `count` (u32), then per entry `name_len` (u32) + UTF-8 name bytes + `offset` (u64). (Avoids a protobuf dependency; matches the libsimlin "Pattern A" malloc-return convention.)
2. Change `simlin_model_compile_to_wasm` to:
   ```rust
   pub unsafe extern "C" fn simlin_model_compile_to_wasm(
       model: *mut SimlinModel,
       out_wasm: *mut *mut u8, out_wasm_len: *mut usize,
       out_layout: *mut *mut u8, out_layout_len: *mut usize,
       out_error: *mut *mut SimlinError,
   )
   ```
   Build the `CompiledSimulation` from the model's datamodel (sync + `compile_project_incremental`), call `compile_simulation` to get the `WasmArtifact`, then `write_bytes_to_ffi_output` the `artifact.wasm` and the serialized `artifact.layout` into the two buffer pairs. Follow the FFI prologue (`clear_out_error`, null-checks, `require_model`). On any compile/codegen error, `store_error`/`store_anyhow_error` (AC6.2 — never panic across the boundary); the function works before any `SimlinSim` exists (it takes a `SimlinModel`).
3. A host reads one variable's series by name: locate `off` from the layout, then for `c in 0..n_chunks` read `results[results_offset + (c*n_slots + off)*8]` — copying only `n_chunks` values.

**Testing:**
- AC6.1: FFI test — compile a model to wasm + layout, assert the wasm validates, the layout deserializes to the expected geometry + name→offset map, and both buffers free with `simlin_free`. Works with only a `SimlinModel` (no `SimlinSim`).
- AC6.2: a model that fails codegen (an unsupported construct, if any remain) surfaces a `SimlinError` (the out_error is set), no panic.
- AC4.2: a `wasmgen`/libsimlin test that reads one variable's `n_chunks`-long series via the layout (striding the slab) and asserts it equals the VM's `get_series` for that variable, and that it copied only `n_chunks` values (not the whole slab).
- AC4.1 (reaffirm): geometry read from the exported globals matches the layout.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen` and `cargo test -p libsimlin`

**Commit:** `libsimlin: simlin_model_compile_to_wasm returns blob + WasmLayout`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Raise floor; module + systems-format + metasd corpus parity

**Verifies:** wasm-backend.AC1.1.

**Files:** Modify `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`); add the wasm hook to `src/simlin-engine/tests/simulate_systems.rs`.

**Implementation:**
Module-bearing models (including SMOOTH/DELAY stdlib expansions) now run through wasm. Add the `ensure_wasm_matches` hook to `simulate_systems.rs` (systems-format models become stdlib-module instances, so they exercise modules). Re-observe the `Ran` counts and raise `WASM_SUPPORTED_FLOOR` (and add a systems floor if appropriate). Heavy/`#[ignore]` models still defer their wasm twins to Phase 8.

**Testing:** the raised floor gates (simulate + simulate_systems); note which module/systems models flipped to `Ran`.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate` and `--test simulate_systems`

**Commit:** `engine: raise wasm parity floor after modules + systems format`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

---

## Phase 7 Done When
- Module-bearing, systems-format, and metasd-simulation models match the VM through wasm.
- Override-then-reset-then-run matches the VM under the same override; reset with no override restores defaults.
- A by-name series read copies only `n_chunks` values and equals the VM's series; the FFI returns blob + layout (both `simlin_free`-able) and surfaces errors without panicking.
- The floor gate(s) are raised.
