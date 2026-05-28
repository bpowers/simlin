# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 1: Thread LTM into the wasm compile + numeric-series parity harness

**Goal:** A wasm blob compiled with LTM enabled carries the `$⁚ltm⁚*` synthetic series in its `WasmLayout`, and a both-backends Rust harness proves those synthetic equations lower and match the bytecode VM numerically for scalar LTM models, behind a monotonically rising floor of LTM-models-that-lower.

**Architecture:** LTM is enabled purely by flipping the salsa `SourceProject` flags (`ltm_enabled` / `ltm_discovery_mode`) *before* `compile_project_incremental` runs; the wasm codegen has zero LTM awareness. Once the synthetic vars exist in the `CompiledSimulation`, they appear in `WasmLayout.var_offsets` automatically (it is a verbatim copy of `CompiledSimulation.offsets`). The parity harness reuses the existing DLR-FT-interpreter machinery in `tests/test_helpers.rs`, adding LTM-enabled VM and wasm `Results` builders, then diffs the `$⁚ltm⁚*` columns.

**Tech Stack:** Rust, salsa (incremental compile), `wasmgen` (wasm codegen). In-test blob execution uses the DLR-FT wasm interpreter — `wasm-interpreter` + `checked` (`features = ["linker", "interop"]`), both `git = "https://github.com/DLR-FT/wasm-interpreter.git", rev = "64cedbba603edfd64cbb6b5a19f5fa34530bb03a"`, already in `src/simlin-engine/Cargo.toml` `[dev-dependencies]` (lines 74-75).

**Scope:** Phase 1 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC1: LTM-enabled wasm compilation produces a blob carrying the LTM series
- **wasm-ltm.AC1.1 Success:** `compile_datamodel_to_artifact(..., ltm_enabled: true, ...)` on a scalar LTM model produces a `WasmLayout` whose `var_offsets` contains the `$⁚ltm⁚link_score⁚*` and `$⁚ltm⁚loop_score⁚*` entries.
- **wasm-ltm.AC1.2 Success:** the blob, run under the DLR-FT interpreter, writes the `$⁚ltm⁚*` columns; those series match the VM within the engine's existing tolerances.
- **wasm-ltm.AC1.5 Edge:** compiling the same model with `ltm_enabled: false` produces a layout with no `$⁚ltm⁚*` entries (LTM-off behavior unchanged).

### wasm-ltm.AC4: Parity harness with a ratcheting floor
- **wasm-ltm.AC4.1 Success:** the harness runs the LTM corpus through both backends, comparing wasm-vs-VM, and enforces a monotonically rising floor on the count of LTM models that lower; heavy models are `#[ignore]`d to respect the 3-minute cap.

---

## Background: what exists today (verified 2026-05-27)

All paths are relative to repo root `/home/bpowers/src/simlin`.

**The compile entry points (no LTM today):**
- `src/simlin-engine/src/wasmgen/module.rs:114-125` — `pub fn compile_datamodel_to_artifact(datamodel: &crate::datamodel::Project, model_name: &str) -> Result<WasmArtifact, WasmGenError>`. Body: creates a fresh `SimlinDb::default()`, `sync_from_datamodel_incremental(&mut db, datamodel, None)`, `compile_project_incremental(&db, sync.project, model_name)` (mapping failure to `WasmGenError::Unsupported`), then `compile_simulation(&sim)`. **It owns its `db` locally**, so flipping the LTM flags here needs **no reset dance** (unlike `simlin_sim_new`, whose `SourceProject` is shared/persistent — see `simulation.rs:133-138`).
- `src/simlin-engine/src/wasmgen/module.rs:131-136` — `pub fn compile_datamodel_to_wasm(datamodel, model_name) -> Result<Vec<u8>, WasmGenError>`. Thin wrapper returning `compile_datamodel_to_artifact(...)?.wasm`. Re-exported with the above at `src/simlin-engine/src/wasmgen/mod.rs:38`.
- Call sites: `compile_datamodel_to_artifact` is called by the FFI at `src/libsimlin/src/model.rs:157`; `compile_datamodel_to_wasm` is called only by the `#[cfg(test)]` test `compile_datamodel_to_wasm_validates` at `src/simlin-engine/src/wasmgen/module.rs:2524` (call at `:2529`).

**The salsa LTM setters and compile (the mechanism we reuse):**
- `src/simlin-engine/src/db.rs:5552` — `pub fn set_project_ltm_enabled(db: &mut SimlinDb, project: SourceProject, enabled: bool)`; `db.rs:5563` — `pub fn set_project_ltm_discovery_mode(...)`. Both use the guarded salsa pattern `use salsa::Setter; if project.ltm_enabled(db) != enabled { project.set_ltm_enabled(db).to(enabled); }`.
- `src/simlin-engine/src/db.rs:5575-5593` — `pub fn compile_project_incremental(db, project, main_model_name) -> crate::Result<CompiledSimulation>`. It reads the LTM flags transitively through salsa-tracked queries; the gated reads are at `db.rs:2542` (layout pass) and `db.rs:4469` (runlist pass), with synthetic-var generation in `src/simlin-engine/src/db_ltm.rs`.
- `src/libsimlin/src/simulation.rs:64-138` (`simlin_sim_new`) is the canonical set→compile→snapshot→**reset** dance for the *shared* project; the reset (`:133-138`) exists only because that `SourceProject` is persistent. `tests/simulate_ltm.rs:44-56` (`compile_ltm_incremental_with_partitions`) is the *local-db* pattern (set, compile, no reset) we mirror.

**Why the layout carries LTM automatically:**
- `WasmLayout` is defined at `src/simlin-engine/src/wasmgen/module.rs:154-169` (fields `n_slots`, `n_chunks`, `results_offset`, `gf_directory_offset`, `gf_data_offset`, `var_offsets: Vec<(String, usize)>`).
- `var_offsets` is a verbatim copy of `CompiledSimulation.offsets` at `module.rs:824-828`:
  ```rust
  let var_offsets = sim
      .offsets
      .iter()
      .map(|(k, v)| (k.as_str().to_string(), *v))
      .collect();
  ```
  No name filter — so once `$⁚ltm⁚*` vars exist in `sim.offsets`, they appear in `var_offsets`. (Serialization is hand-rolled little-endian at `module.rs:193-206` / deserialize `:218-253`; the two GF offsets are intentionally not serialized.)

**Canonical LTM synthetic-var names (for the test assertions):**
- Link score: `link_score_var_name` in `src/simlin-engine/src/ltm_augment.rs:1126-1135` → `"$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}"` (i.e. `$⁚ltm⁚link_score⁚{from}→{to}`).
- Loop score: `loop_score_ident` in `src/simlin-engine/src/ltm_post.rs:35-38` → `"$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}"`. The shared prefix to match on is `"$\u{205A}ltm\u{205A}"`.

**The existing harness machinery we extend:**
- `src/simlin-engine/tests/test_helpers.rs` is shared by `mod test_helpers;` + `use test_helpers::{...}` in each `tests/*.rs` file (e.g. `simulate.rs:5,18-21`). Public items: `WasmRunOutcome` (`:172`, an enum `Ran` / `Skipped(String)`), `wasm_results_for(datamodel: &Project, model_name: &str) -> Result<Results, String>` (`:224`), `ensure_results` (`:66`), `ensure_results_excluding` (`:75`), `ensure_wasm_matches` (`:302`). **`wasm_results_from_slab` (`:190`) is private** — do not call it from a sibling test; go through a `pub` helper.
- `wasm_results_for` builds a local `SimlinDb`, syncs, `compile_project_incremental` (LTM **off** today), `compile_simulation`, runs under the interpreter, and reshapes the step-major slab into a `Results`. It does no file I/O.
- `src/simlin-engine/tests/simulate_ltm.rs` is the VM-side LTM oracle. `compile_ltm_incremental_with_partitions` (`:44-56`) sets `set_project_ltm_enabled(true)` then compiles; `ensure_ltm_results` (`:127-256`) compares rel-loop-score series via `ltm_post::compute_rel_loop_scores` (call at `:138`) with `LTM_TOLERANCE = 0.05` (`:30`). It is `file_io`-gated at the target level: `src/simlin-engine/Cargo.toml:81-83`:
  ```toml
  [[test]]
  name = "simulate_ltm"
  required-features = ["file_io"]
  ```
  Model files load via `simulate_ltm_path` (`:258-261`): `File::open` → `xmile::project_from_reader` → `datamodel::Project`.

**LTM corpus model fixtures present in `test/`:** `logistic_growth_ltm/logistic_growth.stmx` (+ `ltm_results.tsv`, the only numeric reference), `arms_race_3party/arms_race.stmx`, `decoupled_stocks/decoupled.stmx`, `hero_culture_ltm/hero_culture.sd.json` — all scalar. (`arrayed_population_ltm/`, `cross_element_ltm/` are arrayed; deferred to Phase 4.)

**The FFI we extend (`outputs/return unchanged`, two new params):**
- `src/libsimlin/src/model.rs:117` — `simlin_model_compile_to_wasm(model, out_wasm, out_wasm_len, out_layout, out_layout_len, out_error)`. It calls `compile_datamodel_to_artifact(&datamodel, model_ref.model_name.as_str())` at `:157`.

> **ABI note (drives Task 2).** The design's C contract inserts the two new `bool` params **between `model` and `out_wasm`**. Inserting params mid-signature shifts every positional argument, so the TypeScript wrapper that calls the WASM export (`src/engine/src/internal/wasmgen.ts:92-138`, currently a 6-arg call) **must be updated in lockstep** or its pointer args land in the wrong slots at runtime. Phase 1 therefore makes a minimal *keep-green* edit to that wrapper (pass `0, 0` for the two flags, wrapper's external signature unchanged); the real `enableLtm` threading lands in Phase 3. The pre-commit hook rebuilds the WASM and runs the TS suite, so this keep-green edit is mandatory for Phase 1 to be green.

**Divergence from the design doc:** the design references `wasmgen/module.rs:824` as the `WasmLayout` location — `:824` is the `var_offsets` *assignment*; the struct is at `:154`. The "LTM (VM-only)" out-of-scope note the design says to update lives in `src/simlin-engine/CLAUDE.md:29` (there is **no** `wasmgen/CLAUDE.md`); that doc edit is deferred to Phase 6.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Thread `ltm_enabled` / `ltm_discovery_mode` through the engine-level wasm compile

**Verifies:** (enables AC1.1, AC1.5; behavior proven by Task 3)

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs:114-125` (`compile_datamodel_to_artifact`)
- Modify: `src/simlin-engine/src/wasmgen/module.rs:131-136` (`compile_datamodel_to_wasm`)
- Modify: `src/simlin-engine/src/wasmgen/module.rs:2524-2529` (the `compile_datamodel_to_wasm_validates` test caller)

**Implementation:**
1. Change the signature to `pub fn compile_datamodel_to_artifact(datamodel: &crate::datamodel::Project, model_name: &str, ltm_enabled: bool, ltm_discovery_mode: bool) -> Result<WasmArtifact, WasmGenError>`.
2. After `sync_from_datamodel_incremental(&mut db, datamodel, None)` produces `sync`, and **before** `compile_project_incremental(&db, sync.project, model_name)`, set the flags on the freshly-synced project:
   ```rust
   crate::db::set_project_ltm_enabled(&mut db, sync.project, ltm_enabled);
   crate::db::set_project_ltm_discovery_mode(&mut db, sync.project, ltm_discovery_mode);
   ```
   No reset is needed: `db` is a local owned by this function and dropped at return (state the *why* in a one-line comment, since the contrast with `simlin_sim_new`'s reset is non-obvious).
3. Update `compile_datamodel_to_wasm` to take and forward the same two params: `compile_datamodel_to_artifact(datamodel, model_name, ltm_enabled, ltm_discovery_mode)?.wasm`.
4. Update the `compile_datamodel_to_wasm_validates` test caller at `:2529` to pass `false, false`.

**Testing:** No new test in this task (signature/threading change). Behavior is proven by Task 3 (layout assertions) and Task 5 (numeric parity). The change must keep the crate compiling and all existing `simlin-engine` unit tests green.

**Verification:**
Run: `cargo build -p simlin-engine && cargo test -p simlin-engine --lib wasmgen`
Expected: builds; `compile_datamodel_to_wasm_validates` and other wasmgen unit tests pass.

**Commit:** `engine: thread LTM flags through the wasm compile entry points`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add the two flag params to the `simlin_model_compile_to_wasm` FFI (keep TS + header in sync)

**Verifies:** (enables AC1.1 at the FFI layer; full use in Phase 2/3)

**Files:**
- Modify: `src/libsimlin/src/model.rs:117` (`simlin_model_compile_to_wasm` signature) and `:157` (the `compile_datamodel_to_artifact` call)
- Modify: `src/libsimlin/tests/wasm.rs` (every call site of `simlin_model_compile_to_wasm`)
- Modify: `src/engine/src/internal/wasmgen.ts:92-138` (keep-green: pass `0, 0` to the WASM export so the call arity/order matches the new ABI; the wrapper's *external* TS signature is unchanged)
- Regenerate/Update: the libsimlin C header (cbindgen). Locate the header and its generation command via `src/libsimlin/build.rs` and/or `src/libsimlin/cbindgen.toml`; run the documented header-gen step and commit the regenerated header.

**Implementation:**
1. New C signature, flags after `model`, outputs/return unchanged:
   ```rust
   pub unsafe extern "C" fn simlin_model_compile_to_wasm(
       model: *mut SimlinModel,
       ltm_enabled: bool,
       ltm_discovery_mode: bool,
       out_wasm: *mut *mut u8,
       out_wasm_len: *mut usize,
       out_layout: *mut *mut u8,
       out_layout_len: *mut usize,
       out_error: *mut *mut SimlinError,
   )
   ```
2. Forward the flags into `compile_datamodel_to_artifact(&datamodel, model_ref.model_name.as_str(), ltm_enabled, ltm_discovery_mode)` at `:157`.
3. In `src/engine/src/internal/wasmgen.ts`, update the typed `fn` signature to 8 args and the `fn(...)` call to pass `0, 0` (booleans as `i32` 0) for the two flags, immediately after `model`. Add a one-line comment that real threading lands in Phase 3.
4. Update `src/libsimlin/tests/wasm.rs` call sites to pass `false, false` (these tests exercise the non-LTM path; the LTM FFI parity test is added in Phase 2).
5. Grep the repo for any other caller of `simlin_model_compile_to_wasm` (CGo headers, pysimlin); the wasm compile is consumed only by `@simlin/engine` today — update any stragglers to the new arity.

**Testing:** No new behavioral test here; correctness is "everything still builds and the existing wasm FFI + TS tests pass with LTM off."

**Verification:**
Run: `cargo build -p libsimlin && cargo test -p libsimlin --test wasm`
Then the WASM + TS gate the pre-commit hook runs: `pnpm --filter @simlin/engine build && pnpm --filter @simlin/engine test`
Expected: libsimlin wasm FFI tests pass; the WASM module rebuilds; `@simlin/engine`'s `wasm-model` / `wasm-backend` tests pass (LTM still off end-to-end).

**Commit:** `libsimlin: add LTM flags to simlin_model_compile_to_wasm`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-6) -->

<!-- START_TASK_3 -->
### Task 3: LTM-enabled VM and wasm `Results` builders in `test_helpers.rs`

**Verifies:** (test-support for AC1.1, AC1.2, AC1.5, AC4.1)

**Files:**
- Modify: `src/simlin-engine/tests/test_helpers.rs` (add two `pub` helpers near `wasm_results_for` at `:224`)

**Implementation:**
1. Add `pub fn vm_results_for_ltm(datamodel: &simlin_engine::datamodel::Project, model_name: &str) -> Results`: build a local `SimlinDb`, `sync_from_datamodel_incremental(..., None)`, `set_project_ltm_enabled(&mut db, sync.project, true)`, `compile_project_incremental(&db, sync.project, model_name)`, `Vm::new(compiled)?.run_to_end()`, `vm.into_results()`. (Mirror `simulate_ltm.rs:44-56`.)
2. Add `pub fn wasm_results_for_ltm(datamodel: &simlin_engine::datamodel::Project, model_name: &str) -> Result<Results, String>`: identical to the private `wasm_results_from_slab` path used by `wasm_results_for` (`:224-245`), but with `set_project_ltm_enabled(&mut db, sync.project, true)` inserted before `compile_project_incremental`. Reuse the existing private `wasm_results_from_slab(&layout, slab, specs)` reshaper (it is in the same module, so it is reachable from the new `pub` fn). On a wasm-codegen failure, return `Err(format!("{e}"))` so the caller can classify `Unsupported` vs lowered. Carry the `#[allow(dead_code)]` the sibling helpers use.
3. Add a `pub const LTM_SERIES_TOLERANCE: f64 = 1e-6;` (the `$⁚ltm⁚*` columns are computed by identical bytecode on both backends, so they should match to floating-point round-off, far tighter than the 0.05 rel-loop-score tolerance; if a model needs looser, document why inline).
4. Add `pub fn assert_ltm_slabs_match(vm: &Results, wasm: &Results)`: assert `vm.step_size == wasm.step_size` and `vm.step_count == wasm.step_count`, then compare the **entire data slab element-wise** (`for i in 0..vm.step_count*vm.step_size`) within `LTM_SERIES_TOLERANCE` (relative-or-absolute, matching `ensure_results`' style). Because both `Results` are built from the *same* `CompiledSimulation.offsets`/`var_offsets`, slot `i` denotes the identical variable+element in both — so a full-slab compare covers every `$⁚ltm⁚*` column **including each element of an arrayed/cross-element LTM var** (whose elements occupy contiguous slots), with no per-var slot-span bookkeeping. This is the single comparator Phase 4 reuses for arrayed models (carrying AC2.4). Document inline *why* this is a whole-slab compare (arrayed LTM vars are strided, not separately named).

**Testing:** exercised by Tasks 4-6; no standalone test.

**Verification:**
Run: `cargo build -p simlin-engine --tests --features file_io`
Expected: compiles (the helpers are used by the new harness in the next tasks).

**Commit:** `engine: add LTM-enabled VM and wasm Results test helpers`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: New `simulate_ltm_wasm` harness — layout carries (and omits) the LTM series

**Verifies:** wasm-ltm.AC1.1, wasm-ltm.AC1.5

**Files:**
- Add: `src/simlin-engine/tests/simulate_ltm_wasm.rs`
- Modify: `src/simlin-engine/Cargo.toml` (add a `[[test]]` target with `required-features = ["file_io"]`, mirroring `:81-83`)

**Implementation:**
1. Register the target:
   ```toml
   [[test]]
   name = "simulate_ltm_wasm"
   required-features = ["file_io"]
   ```
2. In `simulate_ltm_wasm.rs`: `mod test_helpers;` + `use test_helpers::{...};` (mirror `simulate.rs:5,18-21`). Add a small `fn load(model_rel_path: &str) -> datamodel::Project` that opens the file under `test/` and parses via `xmile::project_from_reader` (mirror `simulate_ltm.rs:258-261`).
3. Write `fn layout_has_ltm_series(layout: &WasmLayout) -> (bool, bool)` returning `(has_link_scores, has_loop_scores)` by scanning `layout.var_offsets` for names starting with `"$\u{205A}ltm\u{205A}link_score\u{205A}"` and `"$\u{205A}ltm\u{205A}loop_score\u{205A}"`.
4. Test `layout_carries_ltm_series_when_enabled` (AC1.1): load `logistic_growth_ltm/logistic_growth.stmx`, call `compile_datamodel_to_artifact(&project, "main", true, false)`, assert both `has_link_scores` and `has_loop_scores`.
5. Test `layout_omits_ltm_series_when_disabled` (AC1.5): same model, `compile_datamodel_to_artifact(&project, "main", false, false)`, assert `var_offsets` contains **no** name starting with `"$\u{205A}ltm\u{205A}"`.

**Testing:** The two tests above ARE the deliverables. They assert against the real `WasmLayout` produced by the engine entry point — testing observable output (layout contents), not internal wiring.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm layout_`
Expected: both layout tests pass.

**Commit:** `engine: assert wasm layout carries LTM series when enabled`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: `$⁚ltm⁚*` numeric-series parity (wasm vs VM) for scalar corpus

**Verifies:** wasm-ltm.AC1.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs`

**Implementation:**
1. Define the scalar corpus as exactly these three XMILE models (relative paths under `test/`, all loadable via `xmile::project_from_reader`): `logistic_growth_ltm/logistic_growth.stmx`, `arms_race_3party/arms_race.stmx`, `decoupled_stocks/decoupled.stmx`. **`hero_culture_ltm/hero_culture.sd.json` is intentionally excluded** from this phase: it is a `.sd.json` model needing a different loader, so it is a deliberate follow-up, not part of the Phase 1 corpus or floor. Do not make the corpus conditional on a JSON loader.
2. Write `fn assert_ltm_series_match(model_rel_path: &str)`: load the project, compute `vm = vm_results_for_ltm(&project, "main")` and `wasm = wasm_results_for_ltm(&project, "main").expect("scalar LTM model should lower")`. First assert the wasm side actually carries LTM (`wasm.offsets` contains at least one key starting with `"$\u{205A}ltm\u{205A}"`) — this guards against silently comparing two LTM-off runs. Then call `assert_ltm_slabs_match(&vm, &wasm)` (the whole-slab comparator from Task 3), which verifies every `$⁚ltm⁚*` column (and every other variable) matches within `LTM_SERIES_TOLERANCE`.
3. One `#[test]` per scalar model calling `assert_ltm_series_match(...)` (small models → fast; no `#[ignore]` needed).

**Testing:** These per-model tests verify AC1.2: the blob's `$⁚ltm⁚*` columns match the VM's within tolerance.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm series_`
Expected: scalar LTM series-parity tests pass.

**Commit:** `engine: verify wasm LTM series match the VM for scalar models`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Ratcheting floor gate over the LTM corpus

**Verifies:** wasm-ltm.AC4.1

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs`

**Implementation:**
1. Add a single source-of-truth floor constant with a comment explaining the ratchet contract:
   ```rust
   /// Monotonically rising floor on the count of LTM corpus models that lower to
   /// wasm. Phase 1 covers the scalar corpus; Phase 4 ratchets this up to include
   /// arrayed models. A regression that drops a previously-supported model below
   /// this floor must fail the suite (wasm-ltm.AC4.2).
   const MIN_LTM_MODELS_LOWERED: usize = 3;
   ```
   The value is `3` — the three scalar `.stmx` corpus models, all of which are expected to lower. Confirm via TDD (write the gate, run it, see all three lower); if any unexpectedly does not lower, investigate rather than lowering the constant.
2. Write `#[test] fn ltm_corpus_floor_gate()`: iterate the corpus list; for each, attempt `wasm_results_for_ltm(&project, "main")`. Count `Ok` (lowered). Collect `Err` messages (e.g. `WasmGenError::Unsupported` rendered) and `eprintln!` them (skip-not-fail during rollout). Assert `lowered >= MIN_LTM_MODELS_LOWERED`.
3. `#[ignore]` any heavy model (none in the scalar set today; the attribute pattern with a documented reason — see `tests/ltm_discovery_large_models.rs:653-661` — is reserved for Phase 4/5 arrayed/discovery heavy models like C-LEARN / World3).

**Testing:** The floor gate is the AC4.1 deliverable: it runs the corpus through both backends (wasm via `wasm_results_for_ltm`, VM available via `vm_results_for_ltm`) and enforces the rising floor.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm ltm_corpus_floor_gate`
Then the full target: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm`
Expected: floor gate passes; whole `simulate_ltm_wasm` target green; runtime well under the per-test budget.

**Commit:** `engine: add ratcheting floor gate for LTM-on-wasm corpus`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

---

## Phase 1 Done When

- `compile_datamodel_to_artifact` / `compile_datamodel_to_wasm` and the `simlin_model_compile_to_wasm` FFI take `ltm_enabled` / `ltm_discovery_mode`, and setting `ltm_enabled: true` puts `$⁚ltm⁚link_score⁚*` and `$⁚ltm⁚loop_score⁚*` into the `WasmLayout.var_offsets` (**wasm-ltm.AC1.1**); `false` produces none (**wasm-ltm.AC1.5**).
- The scalar LTM corpus lowers to wasm and its `$⁚ltm⁚*` series match the VM within `LTM_SERIES_TOLERANCE` (**wasm-ltm.AC1.2**).
- The `simulate_ltm_wasm` harness enforces `MIN_LTM_MODELS_LOWERED`, reports any `Unsupported`, and `#[ignore]`s nothing heavy yet (**wasm-ltm.AC4.1**).
- The TS wrapper passes the two new flags (as `0,0`) so the WASM ABI stays in sync; `cargo test --workspace` and the `@simlin/engine` suite are green with LTM still off end-to-end.
