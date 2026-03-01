# Salsa Consolidation Phase 3: Error Accumulator Consolidation

**Goal:** Make `CompilationDiagnostic` the sole error source, eliminating struct-field errors. All compilation errors (parse, compilation, assembly, unit) surface through the salsa accumulator with specific error codes and severity levels.

**Architecture:** The codebase currently maintains dual error paths: struct fields on `engine::Project` / `ModelStage1` / `Variable` AND the `CompilationDiagnostic` salsa accumulator. This phase extends the accumulator to cover all error types, makes `compile_var_fragment` accumulate specific error codes instead of silently returning `None`, adds assembly-level error accumulation, and moves unit inference into the salsa pipeline as a tracked function. The struct-field error path is not removed yet (Phase 6 does that) but becomes redundant.

**Tech Stack:** Rust (simlin-engine crate, libsimlin for format_diagnostic update), salsa accumulator pattern

**Scope:** Phase 3 of 6 from original design (independent of Phases 1-2)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC2: Patch error checking uses incremental path only
- **salsa-consolidation.AC2.1 Success:** `apply_patch` with a valid equation edit produces identical accept/reject decisions as the current dual-path implementation.
- **salsa-consolidation.AC2.2 Failure:** `apply_patch` with a `BadTable` error (mismatched x/y lengths) surfaces the specific `BadTable` error code, not generic `NotSimulatable`.
- **salsa-consolidation.AC2.3 Failure:** `apply_patch` with `EmptyEquation` (stock with no equation) surfaces `EmptyEquation` error code.
- **salsa-consolidation.AC2.4 Failure:** `apply_patch` with `MismatchedDimensions` surfaces the specific error code.
- **salsa-consolidation.AC2.5 Success:** New unit warnings introduced by a patch are detected and cause rejection, matching current behavior.
- **salsa-consolidation.AC2.7 Success:** VM bytecode validation (`Vm::new`) errors are detected during `apply_patch` and cause rejection.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Extend DiagnosticError with Assembly variant and severity level

**Verifies:** None (type-level scaffolding; verified by compilation)

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (DiagnosticError enum at line 44, Diagnostic struct at line 38)

**Implementation:**

Extend the `DiagnosticError` enum with a new variant for assembly-level errors:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
    Assembly(String),  // new: simulation-level errors (circular deps, missing models, etc.)
}
```

Add a severity level to `Diagnostic`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub error: DiagnosticError,
    pub severity: DiagnosticSeverity,
}
```

Update all existing call sites that create `Diagnostic` to include `severity: DiagnosticSeverity::Error` (equation errors) or `severity: DiagnosticSeverity::Warning` (unit warnings). Search for `CompilationDiagnostic(Diagnostic {` to find all creation sites.

The `format_diagnostic` function in `src/libsimlin/src/errors.rs` (line 326) also needs updating to handle the new `Assembly` variant and severity field.

**Verification:**
Run: `cargo build -p simlin-engine && cargo build -p libsimlin`
Expected: Compiles. Existing tests pass with the new severity field defaulting to Error.

**Commit:** `engine: extend DiagnosticError with Assembly variant and severity level`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Make compile_var_fragment accumulate specific error codes

**Verifies:** salsa-consolidation.AC2.2, salsa-consolidation.AC2.3, salsa-consolidation.AC2.4

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (compile_var_fragment at line 3205)

**Implementation:**

Currently `compile_var_fragment` returns `None` silently when a variable has equation errors (line 3220-3225):

```rust
if parsed.variable.equation_errors().is_some_and(|e| !e.is_empty()) {
    return None;
}
```

Change this to accumulate each error before returning `None`:

```rust
if let Some(errors) = parsed.variable.equation_errors() {
    for err in errors {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var.ident(db).clone()),
            error: DiagnosticError::Equation(err.clone()),
            severity: DiagnosticSeverity::Error,
        }).accumulate(db);
    }
    if !errors.is_empty() {
        return None;
    }
}
```

This preserves the `None` return (so `assemble_module` still detects the failure) while also recording the specific error code through the accumulator. Error codes like `BadTable`, `EmptyEquation`, `MismatchedDimensions` are variants of `ErrorCode` inside `EquationError` and are now accessible through the accumulator path (AC2.2, AC2.3, AC2.4).

Check if there are other error paths in `compile_var_fragment` that silently return `None` and add accumulation for those too.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass. Error accumulation is additive -- it doesn't change the function's return behavior.

**Commit:** `engine: accumulate specific error codes in compile_var_fragment`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Accumulate assembly-level errors in assemble_module and assemble_simulation

**Verifies:** salsa-consolidation.AC2.7

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (assemble_module at line 4602, assemble_simulation at line 4870)

**Implementation:**

Both functions currently return `Err(String)` for assembly failures. Add error accumulation BEFORE returning the error, so the accumulator captures the details even though the function still returns `Err`:

In `assemble_module` (line 4629):
```rust
// Before: return Err(format!("model '{}' has circular dependencies", ...));
// After:
let msg = format!("model '{}' has circular dependencies", model_name);
CompilationDiagnostic(Diagnostic {
    model: model.name(db).clone(),
    variable: None,
    error: DiagnosticError::Assembly(msg.clone()),
    severity: DiagnosticSeverity::Error,
}).accumulate(db);
return Err(msg);
```

Apply the same pattern to all `Err` paths in both functions:
- `assemble_module`: circular dependencies (line 4629), missing fragments (line 4720), concatenation failures (line 4745)
- `assemble_simulation`: model not found (line 4885), module enumeration failures (line 4890), assemble_module failures (line 4922)

Note: `assemble_module` and `assemble_simulation` are NOT tracked functions (by design -- the comment at line 4604 explains this). However, they can still call `.accumulate(db)` because the accumulator is associated with the outermost tracked function in the call stack. The caller (`compile_project_incremental` or the caller's tracked context) picks up the accumulated diagnostics.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass. Error accumulation is additive.

**Commit:** `engine: accumulate assembly-level errors in assemble_module and assemble_simulation`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add check_model_units tracked function

**Verifies:** salsa-consolidation.AC2.5

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (add new tracked function near model_all_diagnostics around line 1315)

**Implementation:**

Create a new salsa tracked function that performs unit inference and checking, accumulating unit warnings/errors:

```rust
#[salsa::tracked]
pub fn check_model_units(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) {
    // Replaces the run_default_model_checks callback logic from project.rs:69-101
    // 1. Build the ModelStage1 representations needed for unit inference
    // 2. Call units_infer::infer()
    // 3. Call units_check::check()
    // 4. Accumulate unit errors as DiagnosticSeverity::Warning
}
```

The existing `model_all_diagnostics` function (line 1315) already accumulates equation and model errors. Extend it to call `check_model_units` so unit diagnostics are captured in the same accumulation scope.

Alternatively, make `model_all_diagnostics` call `check_model_units(db, model, project)` which triggers unit checking and accumulates unit warnings.

Unit warnings use `severity: DiagnosticSeverity::Warning`. The downstream `apply_patch` rejection logic (Phase 4) can then distinguish blocking errors from unit warnings based on severity, matching the current behavior where unit warnings cause patch rejection.

**Key challenge:** The current `units_infer::infer` and `units_check::check` functions take `&ModelStage1` and `&HashMap<Ident<Canonical>, &ModelStage1>`. The salsa path has `SourceModel` / `SourceVariable` instead. The implementation needs to either:
- Build temporary `ModelStage1` from salsa data (wasteful but straightforward), OR
- Create tracked counterparts of the unit inference functions that work with `SourceVariable` directly (cleaner but more work)

Study how `model_all_diagnostics` currently bridges between salsa types and engine types to determine the best approach for this project.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass. Unit warnings are now accumulated through the salsa path.

**Commit:** `engine: add check_model_units tracked function for unit inference`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: Ensure model_all_diagnostics triggers all accumulation sources

**Verifies:** salsa-consolidation.AC2.1, salsa-consolidation.AC2.2, salsa-consolidation.AC2.3, salsa-consolidation.AC2.4, salsa-consolidation.AC2.5

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (model_all_diagnostics at line 1315, collect_all_diagnostics at line 2167)

**Implementation:**

Ensure `model_all_diagnostics` triggers all the tracked functions needed to populate the accumulator:
1. `parse_source_variable` (equation errors) -- already triggered
2. `compile_var_fragment` (compilation errors from Task 2) -- add call if not already triggered
3. `check_model_units` (unit errors from Task 4) -- add call

After this task, `collect_all_diagnostics` returns the complete set of per-variable and per-model diagnostics WITHOUT needing to invoke `compile_project_incremental` or `assemble_simulation`. The assembly-level error accumulation (from Task 3) happens separately when compilation is actually invoked.

This task does NOT create `collect_all_diagnostics_with_compilation` -- that higher-level function belongs in Phase 4 where `apply_patch` (its consumer) is rewritten. Phase 3 focuses only on making the per-variable and per-model diagnostics complete.

Add a unit test in `db_tests.rs` that verifies: create a TestProject with known errors (bad equation, unit mismatch), sync to salsa DB, call `collect_all_diagnostics`, and assert the returned diagnostics contain the expected error codes with correct severity levels.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass. The accumulator path covers per-variable and per-model error types.

**Commit:** `engine: ensure model_all_diagnostics triggers all accumulation sources`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Verification tests for error accumulator consolidation

**Verifies:** salsa-consolidation.AC2.1, salsa-consolidation.AC2.2, salsa-consolidation.AC2.3, salsa-consolidation.AC2.4, salsa-consolidation.AC2.5, salsa-consolidation.AC2.7

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (add new test functions)
- Test reference: `src/libsimlin/src/tests_incremental.rs` (existing FFI acceptance tests)

**Testing:**

Tests must verify each AC listed above:
- **salsa-consolidation.AC2.1:** Create a TestProject, compile it via both paths (old struct-field collection and new accumulator collection). Verify both produce the same set of error codes for valid and invalid equations.

- **salsa-consolidation.AC2.2:** Create a TestProject with a graphical function that has mismatched x/y table lengths (BadTable error). Compile and verify the accumulator contains a diagnostic with `ErrorCode::BadTable`, not a generic `NotSimulatable`.

- **salsa-consolidation.AC2.3:** Create a TestProject with a stock that has no equation (EmptyEquation). Compile and verify `ErrorCode::EmptyEquation` is accumulated.

- **salsa-consolidation.AC2.4:** Create a TestProject with array variables that have mismatched dimensions. Compile and verify `ErrorCode::MismatchedDimensions` is accumulated.

- **salsa-consolidation.AC2.5:** Create a TestProject with explicit unit declarations. Add an equation with a unit mismatch. Compile and verify a `DiagnosticError::Unit` warning is accumulated with `severity: DiagnosticSeverity::Warning`.

- **salsa-consolidation.AC2.7:** Create a TestProject that compiles successfully but produces invalid bytecode (if possible to construct). Alternatively, verify that `Vm::new` failures are caught and reported. If constructing an invalid-bytecode scenario is difficult, verify the error path exists by checking that VM validation is called and failures would be reported.

Use the `TestProject` builder from `test_common.rs`. For each test, use the salsa DB path: create a `SimlinDb`, sync the project, compile, and collect diagnostics.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All new and existing tests pass.

**Commit:** `engine: add verification tests for error accumulator consolidation`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_C -->
