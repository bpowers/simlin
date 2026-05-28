# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 5: Discovery mode (Rust-internal parity)

**Goal:** In discovery mode, the loops discovered (and their per-timestep scores) from a wasm blob's link-score series — fed to `discover_loops_with_graph` over a blob-derived `Results` — match the VM's discovery output.

**Architecture:** Discovery is structurally backend-independent: `discover_loops_with_graph` takes a `&Results` plus salsa-derived structural inputs (`CausalGraph`, stocks, `ltm_vars`, dims) that are identical for both backends. The flag threading from Phase 1 already lets the wasm compile emit discovery-mode synthetic vars (link scores for **all** edges). Phase 5 therefore drives the wasm backend directly in a Rust test (because the production `analyze_model` hard-codes the VM), builds a discovery-mode wasm `Results`, and runs the *same* `discover_loops_with_graph` over both the VM and the wasm `Results`, comparing the discovered loops and their score series. No FFI or TS surface is added.

**Tech Stack:** Rust, salsa, `ltm_finding::discover_loops_with_graph`. In-test blob execution via the DLR-FT interpreter (existing `simlin-engine` dev-dependency). `file_io`-gated harness (loads `.stmx` discovery fixtures).

**Scope:** Phase 5 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC2: Analytic outputs match the VM within tolerance
- **wasm-ltm.AC2.5 Success:** in discovery mode, the discovered loops and their per-timestep scores (via `discover_loops_with_graph` over the blob-derived `Results`) match the VM's discovery output.

---

## Background: what exists today (verified 2026-05-27)

**The discovery entry point (`src/simlin-engine/src/ltm_finding.rs`):**
- `:710-716` — `pub fn discover_loops_with_graph(results: &Results, causal_graph: &CausalGraph, stocks: &[Ident<Canonical>], ltm_vars: &[LtmSyntheticVar], dims: &[datamodel::Dimension]) -> Result<Vec<FoundLoop>>`. It reads link-score series by striding (`results.data[step * results.step_size + offset]`, `:805`); offsets are mapped from `(from,to)` edges via `parse_link_offsets(results, ltm_vars, dims)` (`:717`), expanding A2A link scores into per-element edges.
- `FoundLoop.scores` is per-timestep `Vec<(time, signed_loop_score)>`; **`time` is computed from `results.specs`** (`:797-798`: `results.specs.start + results.specs.save_step * (step as f64)`). This is why the from-series `Results` must carry faithfully-reconstructed `specs` (Phase 2 Task 3 already does this) — for discovery it is **load-bearing**, not cosmetic.

**Why the test drives the backend directly (`src/simlin-engine/src/analysis.rs`):** `analyze_model` (`:72-77`) → `run_ltm_pipeline` (`:135-167`) hard-codes the VM: `compile_project_incremental` → `Vm::new(...).run_to_end()` → `vm.into_results()` (`:135-139`), then `discover_loops_with_graph(&results, ...)` (`:160`). There is no backend parameter, so Phase 5 builds the wasm `Results` itself and calls `discover_loops_with_graph` directly.

**What discovery mode changes (`src/simlin-engine/src/db_ltm.rs`):** the flag is read in `model_ltm_variables` at `:3605` (`let is_discovery_user = project.ltm_discovery_mode(db);`). Discovery (a) sets `loops = None` (`:3723`, `:3809-3811`) so loops are discovered post-hoc, and (b) emits a `$⁚ltm⁚link_score⁚{from}→{to}` for **every causal edge** (`:5154-5170`), vs only loop-participating edges in exhaustive mode. So a discovery-mode wasm blob carries link-score columns for all edges, exactly what `discover_loops_with_graph` consumes.

**The VM discovery-input builder to reuse (`src/simlin-engine/tests/ltm_discovery_large_models.rs`):** `discovery_inputs` (`:190-225`) sets `set_project_ltm_enabled(true)` + `set_project_ltm_discovery_mode(true)`, compiles, runs the VM, `vm.into_results()`, then assembles `causal_graph` (`model_element_causal_edges` + `causal_graph_from_element_edges`), `stocks`, `ltm_vars` (`model_ltm_variables`), `dm_dims`, bundled in a `'static DiscoveryInputs` struct (`:166-172`). These structural inputs are backend-independent.

**Corpus + heavy-model facts (corrected vs the design doc):**
- `arms_race_3party/arms_race.stmx` (1,987 bytes, 57 lines) is **small and fast** — `discovery_arms_race_3party` (`simulate_ltm.rs:394`) is a non-ignored `#[test]`. **This is the Phase 5 parity model.**
- The genuinely heavy discovery models are **C-LEARN** (`clearn_ltm_discovery_compiles`, `ltm_discovery_large_models.rs:660`, 1.4 MB MDL, `#[ignore]`d at `:661`) and **World3** (`world3_discovery_single_timestep`, `:464`, 166-node, `#[ignore]`d at `:465`). The design doc's "arms-race" in the ignore list is wrong — substitute World3.
- `#[ignore]` pattern with a documented reason: `ltm_discovery_large_models.rs:653-661`.

**Phase 1 helper available:** `test_helpers::wasm_results_for_ltm` (sets `ltm_enabled` only). Phase 5 needs a discovery variant that also sets `ltm_discovery_mode`.

**Divergence from the design doc:** (1) arms-race is the parity model, not an ignored heavy model; C-LEARN and World3 are the `#[ignore]`d ones. (2) The design says "reconstruct a `Results`" — for discovery this `Results` must carry valid `specs` (Phase 2 Task 3's reconstruction), since `discover_loops_with_graph` reads `results.specs` for the time axis.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Discovery-mode wasm `Results` helper + shared discovery-input builder

**Verifies:** (test-support for AC2.5)

**Files:**
- Modify: `src/simlin-engine/tests/test_helpers.rs` (add helpers)
- Modify: `src/simlin-engine/tests/ltm_discovery_large_models.rs` (dedupe `discovery_inputs` onto the shared helper)

**Implementation:**
1. Add `pub fn wasm_results_for_ltm_discovery(datamodel: &simlin_engine::datamodel::Project, model_name: &str) -> Result<Results, String>`: same as `wasm_results_for_ltm` (Phase 1 Task 3) but set **both** `set_project_ltm_enabled(true)` and `set_project_ltm_discovery_mode(true)` before `compile_project_incremental`. The reconstructed `Results` must include `specs` (the existing slab-reshape path already sets specs via the local compile — confirm `wasm_results_from_slab` / the helper carries specs; if it currently defaults specs, populate them from the same compile).
2. Extract **one** shared builder into `test_helpers.rs` (commit to this outcome — do not leave an "if incompatible, keep it thin" branch for the executing engineer):
   - `pub struct LtmDiscoveryInputs { pub vm_results: Results, pub causal_graph: CausalGraph, pub stocks: Vec<Ident<Canonical>>, pub ltm_vars: LtmVariables, pub dims: Vec<datamodel::Dimension> }` (match the actual field types in `ltm_discovery_large_models.rs:166-172`).
   - `pub fn ltm_discovery_inputs(datamodel: &Project, model_name: &str) -> LtmDiscoveryInputs` containing the input-building body of `discovery_inputs` (`:190-225`), returning the struct **by value**. All fields are owned data (a `Results`, a `CausalGraph`, `Vec`s), so the returned value is naturally `'static` (no borrows) — no `Box::leak` is needed to produce it.
   - Make `ltm_discovery_large_models.rs::discovery_inputs` a thin wrapper that calls `test_helpers::ltm_discovery_inputs(datamodel, "main")` (it currently hardcodes the `"main"` model; the new `model_name` param generalizes it). If — and only if — that file currently obtains `&'static` references by leaking the struct, the wrapper performs that same `Box::leak` on the value the shared builder returns; that is a mechanical wrapping of the shared builder's output, **not** a re-implementation of the body. The body lives in exactly one place (`test_helpers::ltm_discovery_inputs`), satisfying the AC5.1 anti-divergence principle at the harness level.

**Testing:** exercised by Task 2; `ltm_discovery_large_models.rs` must still compile and its (ignored) tests still build.

**Verification:**
Run: `cargo build -p simlin-engine --tests --features file_io`
Expected: compiles; `ltm_discovery_large_models` builds.

**Commit:** `engine: add discovery-mode wasm Results helper and share discovery inputs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Discovery parity test (wasm vs VM) on the small corpus

**Verifies:** wasm-ltm.AC2.5

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs` (add a discovery section; it is already `file_io`-gated from Phase 1)

**Implementation:**
1. `fn assert_discovery_matches(model_rel_path: &str)`:
   - Load the project; `let inputs = ltm_discovery_inputs(&project, "main");` (gives the VM `Results` + structural inputs).
   - `let wasm = wasm_results_for_ltm_discovery(&project, "main").expect("discovery model should lower");`.
   - `let vm_loops = discover_loops_with_graph(&inputs.vm_results, &inputs.causal_graph, &inputs.stocks, &inputs.ltm_vars.vars, &inputs.dims).unwrap();`
   - `let wasm_loops = discover_loops_with_graph(&wasm, &inputs.causal_graph, &inputs.stocks, &inputs.ltm_vars.vars, &inputs.dims).unwrap();`
   - Assert the discovered loop **sets** are identical: same count, and matching by a stable key (the loop's ordered link/stock identity — use whatever `FoundLoop` exposes as its identity, e.g. `loop_info`'s links). For each matched loop, assert the per-timestep `scores` series (the `(time, score)` pairs) are equal within a tolerance: times exactly equal (specs reconstructed), scores within a relative/absolute closeness (`1e-6`, since both feed `discover_loops_with_graph` the same structural inputs and near-identical series).
2. `#[test] fn discovery_arms_race_matches_vm()` → `assert_discovery_matches("arms_race_3party/arms_race.stmx")` (small, not ignored).
3. Add `#[test] #[ignore]` twins for the heavy discovery models (C-LEARN, World3) with a documented reason (mirror `ltm_discovery_large_models.rs:653-661`), so they can be run explicitly but do not count against the 3-minute cap.

**Testing:** `discovery_arms_race_matches_vm` is the AC2.5 deliverable: the loops + per-timestep scores discovered over the wasm `Results` equal the VM's.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm discovery_arms_race_matches_vm`
Expected: passes; fast (arms_race is tiny). Heavy twins remain `#[ignore]`d.

**Commit:** `engine: verify wasm discovery-mode loops match the VM`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase 5 Done When

- A discovery-mode wasm blob lowers and its link-score series drive `discover_loops_with_graph` to the **same discovered loops and per-timestep scores** as the VM for the small discovery corpus (arms_race), within tolerance (**wasm-ltm.AC2.5**).
- The heavy discovery models (C-LEARN, World3) have `#[ignore]`d wasm-discovery twins (runnable explicitly), respecting the 3-minute `cargo test` cap.
- No FFI or TypeScript surface was added; the wasm backend is driven directly in-test.
- `cargo test -p simlin-engine --features file_io` and `cargo test --workspace` are green.
