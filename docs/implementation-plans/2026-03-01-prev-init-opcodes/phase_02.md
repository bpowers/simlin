# Activate PREVIOUS/INIT Opcodes -- Phase 2

**Goal:** Remove PREVIOUS-specific module handling from LTM compilation, simplifying the code now that 1-arg PREVIOUS compiles to LoadPrev.

**Architecture:** After Phase 1, LTM equations (which only use 1-arg PREVIOUS) produce no PREVIOUS module instances -- `parsed.implicit_vars` is empty for PREVIOUS. The PREVIOUS-specific code in `compile_ltm_equation_fragment()` is dead. Remove it, keeping SMOOTH/DELAY module support intact.

**Tech Stack:** Rust (simlin-engine)

**Scope:** Design Phase 3

**Codebase verified:** 2026-03-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### prev-init-opcodes.AC4: LTM uses direct opcodes
- **prev-init-opcodes.AC4.1 Success:** LTM link-score equations with `PREVIOUS(y)` compile to `LoadPrev` and produce correct loop scores matching reference data
- **prev-init-opcodes.AC4.2 Success:** PREVIOUS-specific module handling removed from `compile_ltm_equation_fragment()` without affecting SMOOTH/DELAY module support

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Remove PREVIOUS-specific code from compile_ltm_equation_fragment

**Verifies:** prev-init-opcodes.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs:358-725`

**Implementation:**

The changes are in `compile_ltm_equation_fragment()` (line 191). After Phase 1, LTM equations with `PREVIOUS(y)` compile to `LoadPrev` instead of creating `stdlib⁚previous` module instances. The following PREVIOUS-specific code paths are now dead and should be removed.

**Remove 1: `implicit_module_vars` declaration (lines 361-362)**

Delete:
```rust
let mut implicit_module_vars: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> =
    Vec::new();
```

This vec was only populated by the PREVIOUS-in-parsed-implicit-vars path (lines 463). The SMOOTH/DELAY path uses `dep_variables` instead.

**Remove 2: `ltm_implicit_info` fetch (line 358)**

Delete:
```rust
let ltm_implicit_info = model_ltm_implicit_var_info(db, model, project);
```

This was only used by the cross-LTM-equation PREVIOUS path (line 536). After Phase 1, `model_ltm_implicit_var_info()` returns an empty HashMap since no PREVIOUS modules exist in LTM equations.

**Remove 3: PREVIOUS-in-parsed-implicit-vars block (lines 432-479)**

Delete the entire block that searches `parsed.implicit_vars` for PREVIOUS module instances:
```rust
// Check if this is an implicit module from the LTM equation's own
// PREVIOUS expansion
let mut found_in_parsed = false;
for implicit_dm_var in &parsed.implicit_vars {
    // ... 45 lines of PREVIOUS module instance construction ...
}
```

**Remove 4: Restructure the `if !found_in_parsed` wrapper (lines 481-606)**

The current structure is:
```rust
if !found_in_parsed {
    // SMOOTH/DELAY path (481-534)
    if let Some(im_meta) = implicit_info.get(module_var_name) { ... }
    // Cross-LTM PREVIOUS path (535-605)
    else if let Some(ltm_im_meta) = ltm_implicit_info.get(module_var_name) { ... }
}
```

After removing the `found_in_parsed` variable and PREVIOUS block, replace with:
```rust
// Check model implicit vars (SMOOTH, DELAY, etc.)
if let Some(im_meta) = implicit_info.get(module_var_name)
    && im_meta.is_module
    && let Some(im_model_name) = im_meta.model_name.as_deref()
{
    // ... existing SMOOTH/DELAY code from lines 487-534 ...
}
```

The `else if` branch (lines 535-605) for cross-LTM PREVIOUS deps is deleted entirely.

**Remove 5: implicit_module_vars metadata insertion (lines 679-692)**

Delete:
```rust
// Add implicit module vars from PREVIOUS expansion
for (im_ident, im_var, im_size) in &implicit_module_vars {
    if !mini_metadata.contains_key(im_ident) {
        mini_metadata.insert(
            im_ident.clone(),
            crate::compiler::VariableMetadata {
                offset: mini_offset,
                size: *im_size,
                var: im_var,
            },
        );
        mini_offset += im_size;
    }
}
```

The `dep_variables` metadata insertion (lines 664-677) stays -- it handles SMOOTH/DELAY modules and explicit model deps.

**Keep: implicit_module_refs merge (lines 717-725)**

This section merges `implicit_module_refs` into `module_models`. Keep it -- the SMOOTH/DELAY path (line 528-529) still populates `implicit_module_refs`. After PREVIOUS removal, this will only contain SMOOTH/DELAY entries.

**Keep: implicit_submodels metadata (lines 701-704)**

Keep this -- `implicit_submodels` is still populated by the SMOOTH/DELAY path (line 531-533) and explicit model modules (line 426).

**Summary of preserved data structures:**
- `dep_variables` -- KEPT (handles SMOOTH/DELAY modules and model deps)
- `implicit_module_refs` -- KEPT (populated by SMOOTH/DELAY path)
- `implicit_submodels` -- KEPT (populated by SMOOTH/DELAY and explicit modules)
- `implicit_module_vars` -- REMOVED (was PREVIOUS-only)
- `ltm_implicit_info` -- REMOVED (was PREVIOUS-only)

**Verification:**

Run: `cargo test -p simlin-engine simulate_ltm`
Expected: All LTM tests pass (simulate_ltm.rs tests including simulates_population_ltm and all discovery tests).

**Commit:** Do not commit yet (combine with Task 2)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Run tests and commit

**Verifies:** prev-init-opcodes.AC4.1

**Files:** None (verification only)

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All tests pass. Key LTM tests to watch:
- `simulates_population_ltm` -- runs LTM simulation on logistic growth model, compares against `ltm_results.tsv` reference data
- `discovery_logistic_growth_finds_both_loops`
- `discovery_arms_race_3party`
- `test_smooth_with_initial_value_ltm` -- SMOOTH + LTM together (verifies SMOOTH module support preserved)
- `test_smooth_goal_seeking_ltm` -- cross-validates interpreter vs VM
- `test_multiple_smooth_instances`

These tests confirm AC4.1: LTM link-score equations with PREVIOUS(y) compile to LoadPrev and produce correct loop scores.

**Commit:**
```bash
git add src/simlin-engine/src/db_ltm.rs
git commit -m "engine: remove PREVIOUS module handling from LTM compilation"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
