# Salsa Consolidation Phase 2: LTM Parallel Compilation Path

**Goal:** Connect existing LTM tracked functions to the incremental compilation pipeline so LTM synthetic variables compile through `assemble_simulation` instead of the monolithic `with_ltm()` + `compile_simulation()` fallback.

**Architecture:** LTM synthetic variables (link scores, loop scores, relative scores) are pure scalar aux equations using PREVIOUS (now a builtin opcode from Phase 1). A new `compile_ltm_var_fragment` tracked function compiles these equations per-link for caching. `compute_layout` gets a third section for LTM variable offsets. `assemble_module` gets a third pass to compile and concatenate LTM fragments. An `ltm_enabled` flag on `SourceProject` gates this work. The monolithic LTM path in `simlin_sim_new` is removed.

**Tech Stack:** Rust (simlin-engine, libsimlin crates), salsa incremental computation framework

**Scope:** Phase 2 of 6 from original design

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC1: LTM fully incrementalized
- **salsa-consolidation.AC1.1 Success:** `simlin_sim_new` with `enable_ltm=true` produces identical numerical results to the current monolithic `with_ltm()` path for all models in `tests/simulate_ltm.rs`.
- **salsa-consolidation.AC1.2 Success:** Equation edit with unchanged dependency set does not recompile any LTM fragments (verifiable via salsa event logging).
- **salsa-consolidation.AC1.3 Success:** Equation edit with changed dependency set recompiles only affected link score equations and their fragments.
- **salsa-consolidation.AC1.4 Success:** Models with no feedback loops incur zero LTM overhead (no extra layout slots, no extra fragments compiled).
- **salsa-consolidation.AC1.5 Success:** `ltm_enabled=false` skips all LTM layout and assembly work; compilation produces identical bytecode to current non-LTM path.
- **salsa-consolidation.AC1.6 Success:** Discovery mode (`model_ltm_all_link_synthetic_variables`) compiles through the same parallel path with per-link caching.
- **salsa-consolidation.AC1.7 Success:** Stdlib dynamic module composite scores (SMOOTH, DELAY, TREND internal LTM) compile once and are never recomputed.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Add ltm_enabled field to SourceProject

**Verifies:** None (scaffolding; verified by compilation)

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (SourceProject struct at line 101)

**Implementation:**

Add an `ltm_enabled: bool` field to the `SourceProject` salsa input struct:

```rust
#[salsa::input]
pub struct SourceProject {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub sim_specs: SourceSimSpecs,
    #[returns(ref)]
    pub dimensions: Vec<SourceDimension>,
    #[returns(ref)]
    pub units: Vec<SourceUnit>,
    #[returns(ref)]
    pub model_names: Vec<String>,
    #[returns(ref)]
    pub models: HashMap<String, SourceModel>,
    pub ltm_enabled: bool,
    pub ltm_discovery_mode: bool,
}
```

Since this is a `#[salsa::input]`, salsa generates setter methods automatically. Update all call sites that create `SourceProject` to include `ltm_enabled: false` and `ltm_discovery_mode: false` as defaults. Search for `SourceProject::new(` to find all creation sites (likely in `sync_from_datamodel_incremental` and test helpers).

The `ltm_discovery_mode` flag controls whether `assemble_module` uses `model_ltm_all_link_synthetic_variables` (discovery mode -- scores for every causal edge) vs `model_ltm_synthetic_variables` (normal -- scores only for edges in detected loops). Both flags live on `SourceProject` as the single control point for the LTM subsystem.

**Verification:**
Run: `cargo build -p simlin-engine`
Expected: Compiles. Existing tests pass (ltm_enabled defaults to false, no behavior change).

**Commit:** `engine: add ltm_enabled field to SourceProject salsa input`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Extend compute_layout with LTM variable section

**Verifies:** salsa-consolidation.AC1.4, salsa-consolidation.AC1.5

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (compute_layout function at line 2893)

**Implementation:**

After the existing two sections (explicit variables, implicit variables), add a third section for LTM synthetic variables. This section is gated on `project.ltm_enabled(db)`:

```rust
// Section 3: LTM synthetic variables (only when ltm_enabled)
if project.ltm_enabled(db) {
    let ltm_vars = model_ltm_synthetic_variables(db, model, project);
    // Sort LTM var names alphabetically for deterministic layout
    let mut ltm_names: Vec<&str> = ltm_vars.vars.iter().map(|v| v.name.as_str()).collect();
    ltm_names.sort();
    for name in ltm_names {
        // LTM vars are always scalar aux variables: 1 slot each
        layout.insert(name.to_string(), VariableOffset { offset, size: 1 });
        offset += 1;
    }
}
```

The `VariableLayout` struct (in `compiler/symbolic.rs`) needs to accommodate LTM variable offsets the same way it does for explicit and implicit variables.

When `ltm_enabled` is false, this section is skipped entirely -- zero overhead (AC1.4, AC1.5).

When the model has no feedback loops, `model_ltm_synthetic_variables` returns an empty list -- zero overhead (AC1.4).

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass. With ltm_enabled=false (default), layout is unchanged.

**Commit:** `engine: extend compute_layout with LTM variable offset section`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Implement compile_ltm_var_fragment tracked function

**Verifies:** salsa-consolidation.AC1.2, salsa-consolidation.AC1.3, salsa-consolidation.AC1.7

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (add new tracked function near the existing LTM functions around line 1700)

**Implementation:**

Add a new salsa tracked function that compiles a single LTM synthetic variable's equation to a `CompiledVarFragment`:

```rust
#[salsa::tracked(returns(ref))]
pub fn compile_ltm_var_fragment(
    db: &dyn Db,
    link_id: LtmLinkId<'_>,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult>
```

The function:
1. Calls `link_score_equation_text(db, link_id, model, project)` to get the `LtmSyntheticVar` (name + equation string). This is already tracked and cached per-link.
2. Parses the equation string using the engine's parser.
3. Runs the equation through `BuiltinVisitor` (PREVIOUS is now a builtin from Phase 1, so no module expansion).
4. Lowers the expression to the compiler's `Expr` representation.
5. Compiles to symbolic bytecode via the same codegen path used by `compile_var_fragment`.
6. Returns a `VarFragmentResult` wrapping the `CompiledVarFragment`.

Since the equation text is cached per `(LtmLinkId, model)` by `link_score_equation_text`, and this function is tracked, salsa automatically caches the compiled fragment per-link. Equation edits that don't change the dependency set won't invalidate `link_score_equation_text`, so the fragment isn't recompiled (AC1.2). Changed dependencies only invalidate affected links (AC1.3).

Add analogous tracked functions for loop score and relative loop score variables:
- `compile_ltm_loop_score_fragment` -- keyed on `(loop_id: usize, SourceModel, SourceProject)` for per-loop aggregate scores. The loop_id comes from the deterministic loop assignment in `assign_deterministic_loop_ids()`.
- `compile_ltm_relative_score_fragment` -- keyed on `(loop_id: usize, SourceModel, SourceProject)` for relative loop scores. Same loop_id key as loop scores.

For stdlib dynamic module composite scores (SMOOTH, DELAY, TREND internal LTM via `module_ilink_equation_text`), the same tracked function pattern applies. Since module equations don't change, these fragments compile once and are never recomputed (AC1.7).

**Key implementation note:** LTM vars are NOT `SourceVariable` salsa inputs. They don't go through `compile_var_fragment`. The new function creates a transient parse/lower/compile pipeline just for the LTM equation string. Study how `compile_var_fragment` works (db.rs line 3206) and replicate the relevant subset for scalar aux equations (no stocks, no arrays, no modules).

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: Compiles. Existing tests pass (function exists but not called yet from assemble_module).

**Commit:** `engine: implement compile_ltm_var_fragment tracked function`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Extend assemble_module with LTM third pass

**Verifies:** salsa-consolidation.AC1.1, salsa-consolidation.AC1.4, salsa-consolidation.AC1.5, salsa-consolidation.AC1.6

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (assemble_module function at line 4602)

**Implementation:**

After the existing two passes (explicit variables at line 4642, implicit variables at line 4655), add a third pass for LTM synthetic variables:

```rust
// Pass 3: LTM synthetic variables (only when ltm_enabled)
if project.ltm_enabled(db) {
    let ltm_vars = model_ltm_synthetic_variables(db, model, project);
    for ltm_var in &ltm_vars.vars {
        // Get the LtmLinkId for this variable
        let link_id = LtmLinkId::new(db, ltm_var.link_from.clone(), ltm_var.link_to.clone());
        if let Some(fragment_result) = compile_ltm_var_fragment(db, link_id, model, project) {
            let fragment = &fragment_result.fragment;
            // LTM vars are scalar auxes: only flow_bytecodes, no initial or stock
            if let Some(flow_bc) = &fragment.flow_bytecodes {
                all_fragments.insert(ltm_var.name.clone(), fragment.clone());
                // Append to runlist_flows (LTM vars have no ordering constraints)
                runlist_flows.push(ltm_var.name.clone());
            }
        }
    }
}
```

Discovery mode (AC1.6): Use `project.ltm_discovery_mode(db)` (added to `SourceProject` in Task 1) to select the LTM variable source:
```rust
let ltm_vars = if project.ltm_discovery_mode(db) {
    model_ltm_all_link_synthetic_variables(db, model, project)
} else {
    model_ltm_synthetic_variables(db, model, project)
};
```

LTM vars have no dt-phase ordering constraints with regular variables because `LoadPrev` reads from `curr[]` (previous timestep's committed values). They can be appended to the end of `runlist_flows`.

When `ltm_enabled` is false, this pass is skipped entirely (AC1.5). When there are no feedback loops, `model_ltm_synthetic_variables` returns empty (AC1.4).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass. With ltm_enabled=false (default), assembly is unchanged.

**Commit:** `engine: extend assemble_module with LTM compilation third pass`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: Update simlin_sim_new to use incremental path for LTM

**Verifies:** salsa-consolidation.AC1.1

**Files:**
- Modify: `src/libsimlin/src/simulation.rs` (simlin_sim_new at line 38, lines 80-102 monolithic fallback)

**Implementation:**

Remove the monolithic LTM fallback (lines 80-102) in `simlin_sim_new`. Instead, when `enable_ltm` is true:

1. Set `ltm_enabled` on the `SourceProject` salsa input:
   ```rust
   sync.project.set_ltm_enabled(&mut db).to(enable_ltm);
   ```
   This must happen AFTER `sync_from_datamodel_incremental` returns the sync state but BEFORE `compile_project_incremental`.

2. Use the same incremental compilation path as non-LTM:
   ```rust
   let compiled = engine::db::compile_project_incremental(&db, sync.project, &model_ref.model_name)?;
   let vm = Vm::new(compiled)?;
   ```

3. The `compute_layout` and `assemble_module` extensions from Tasks 2 and 4 handle the LTM compilation automatically when `ltm_enabled` is true.

4. Delete the `cloned_project`, `project.with_ltm()`, and `compile_simulation(&project_variant, ...)` code block entirely.

**Note:** The `SimlinProject` struct in `src/libsimlin/src/lib.rs` currently holds `project: Mutex<engine::Project>`. This task does NOT change that field yet (Phase 4 handles that). The `db` and `sync` are obtained the same way as the non-LTM path.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass, including all LTM tests in `tests/simulate_ltm.rs`. This is the critical AC1.1 verification.

**Commit:** `engine: use incremental compilation path for LTM in simlin_sim_new`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Verification tests for LTM incremental compilation

**Verifies:** salsa-consolidation.AC1.1, salsa-consolidation.AC1.2, salsa-consolidation.AC1.3, salsa-consolidation.AC1.4, salsa-consolidation.AC1.5, salsa-consolidation.AC1.6, salsa-consolidation.AC1.7

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (add new test functions)
- Test reference: `src/simlin-engine/tests/simulate_ltm.rs` (existing LTM integration tests)

**Testing:**

Tests must verify each AC listed above:
- **salsa-consolidation.AC1.1:** The existing LTM integration tests in `tests/simulate_ltm.rs` cover this via `simulate_ltm_path()` which cross-validates interpreter vs VM results against reference `ltm_results.tsv`. Running the full test suite verifies this. Add an explicit unit test in `db_tests.rs` that creates a TestProject with feedback loops, sets `ltm_enabled=true`, compiles incrementally, and verifies LTM synthetic variables appear in the compiled output with correct offsets.

- **salsa-consolidation.AC1.2:** Add a salsa event logging test in `db_tests.rs`. Create a model with LTM, compile once. Then change an equation that does NOT affect the causal graph (e.g., change a coefficient in a flow equation). Recompile and verify via salsa's `DidValidateMemoizedValue` events that `compile_ltm_var_fragment` was NOT re-executed.

- **salsa-consolidation.AC1.3:** Similar to AC1.2 but change an equation that DOES affect the dependency set (add/remove a variable reference). Verify that only affected link score fragments are recompiled.

- **salsa-consolidation.AC1.4:** Create a TestProject with no feedback loops (e.g., linear chain: aux -> flow -> stock). Set `ltm_enabled=true`, compile. Verify the layout has zero LTM variable slots and no LTM fragments were compiled.

- **salsa-consolidation.AC1.5:** Create a TestProject with feedback loops. Compile twice: once with `ltm_enabled=false` and once with `ltm_enabled=true`. Verify the `ltm_enabled=false` compilation produces identical bytecode to a compilation without any LTM infrastructure.

- **salsa-consolidation.AC1.6:** Create a TestProject with multiple causal links. Use `model_ltm_all_link_synthetic_variables` (discovery mode) and verify all links get score variables, not just those in loops.

- **salsa-consolidation.AC1.7:** Create a TestProject with a SMOOTH module that has internal LTM variables. Compile once, then edit an unrelated equation. Verify the SMOOTH's internal LTM fragments are NOT recompiled.

Follow the `TestProject` builder pattern from `test_common.rs` and the salsa event logging patterns from existing `db_tests.rs` tests.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All new and existing tests pass.

**Commit:** `engine: add verification tests for LTM incremental compilation path`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_C -->
