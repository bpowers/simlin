# C-LEARN Residual — Phase 3: Correct user-macro INITIAL recurrences (#591-c1)

**Goal:** An element-wise `INITIAL` recurrence expressed through a trivial passthrough macro (`:MACRO: INIT(x) = INITIAL(x)`) produces correct values at every saved step, matching the proven bare-`INITIAL` opcode path — no drop to `0` at t≥1 and no spurious `:NA:`.

**Architecture:** The importer renames Vensim `INITIAL`/`active initial`/`reinitial` to `INIT` (`xmile_compat.rs:545-547`). C-LEARN also defines `:MACRO: INIT(x) = INITIAL(x)` (which, after the rename, reads `INIT = INIT(x)`). Under macro-shadows-everything precedence (`builtins_visitor.rs:626-638`), every `INITIAL` call therefore collides with the macro and expands to a per-element synthetic module whose value is mis-ordered/mis-propagated. The fix collapses a *genuine passthrough macro* (single parameter, single primary-output body that is exactly `out = BUILTIN(param)` where `BUILTIN` is a renamed-builtin self-call) directly to the proven opcode (`LoadInitial`) at the call site, bypassing `expand_module_function`. This generalizes the existing #554 self-call exception from inside the macro body to the call site.

**Key constraint discovered in investigation:** the macro body AST is **not** reachable at the call site — `MacroRegistry`/`ModuleFunctionDescriptor` deliberately store only name/ports/output, and the body is parsed transiently in `MacroRegistry::build` and discarded. So the collapse requires computing a body-derived "passthrough" classification in `MacroRegistry::build` (where each body is already parsed) and threading it onto `ModuleFunctionDescriptor`; the call site then reads that flag. This is a registry/descriptor change plus a call-site branch, not a pure call-site change.

**Tech Stack:** Rust (`simlin-engine` crate). Tests use the inline-MDL harness (`tests/simulate.rs::run_inline_mdl` + `element_series`) and a small file fixture mirroring the proven `helper_recurrence.mdl`.

**Scope:** Phase 3 of 5 from `docs/design-plans/2026-05-19-clearn-residual.md`.

**Codebase verified:** 2026-05-20 (branch `clearn-residual`, off `main`@`2ed93950`).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### clearn-residual.AC3: User-macro `INITIAL` recurrences produce correct multi-step values
- **clearn-residual.AC3.1 Success:** A model with `:MACRO: INIT(x) = INITIAL(x)` and a scalar `INITIAL`-captured value holds that value constant across all saved steps (matching the opcode path).
- **clearn-residual.AC3.2 Success:** An element-wise `INITIAL` recurrence routed through the passthrough macro produces correct per-element values at t0 and at every subsequent step (no drop to `0` at t≥1, no spurious `:NA:`).
- **clearn-residual.AC3.3 Success (no regression):** A bare `INITIAL(expr)` with no user macro still compiles to `LoadInitial` and behaves as today (`helper_recurrence.mdl` stays green).
- **clearn-residual.AC3.4 Edge:** The passthrough collapse fires only for a genuine `out = BUILTIN(param)` body; a non-passthrough macro that merely shares a builtin name still expands as a macro (not mis-collapsed to the opcode).
- **clearn-residual.AC3.5 Success (downstream):** A `SAMPLE UNTIL` fed by a corrected `INITIAL` value samples until the correct time without any dedicated `SAMPLE UNTIL` change.

---

## Verified ground truth (read before starting)

Confirmed by investigation on 2026-05-20.

- **INITIAL→INIT rename:** `src/simlin-engine/src/mdl/xmile_compat.rs:545-547` (`"active initial"`/`"initial"`/`"reinitial"` → `"INIT"`).
- **Macro expansion → synthetic module:** `expand_module_function` (`src/simlin-engine/src/builtins_visitor.rs:480-579`). Receives only a `ModuleFunctionDescriptor` (model_name, parameter_ports, primary_output, additional_outputs, is_macro) — **NOT the body AST**.
- **#554 self-call exception (the pattern to generalize):** predicate `is_enclosing_macro_renamed_builtin_self_call` at `builtins_visitor.rs:254-261`; its use gates the macro-resolution branch at `builtins_visitor.rs:623-638`:
  ```rust
  let is_renamed_builtin_self_call = self.is_enclosing_macro_renamed_builtin_self_call(&func);
  ...
  if !is_renamed_builtin_self_call
      && let Some(descriptor) = self.macro_registry.resolve_macro(&func) {
      let descriptor = descriptor.clone();
      return self.expand_module_function(&descriptor, &func, args, loc);
  }
  // falls through to alias/MODULO/PREVIOUS/INIT intrinsic routing
  ```
  When the branch is skipped, control reaches the `init`/`previous` intrinsic routing (~`builtins_visitor.rs:655-697`), which emits `LoadInitial`/`LoadPrev` (with `make_temp_arg` hoisting at `:413-440` for expression args).
- **`MacroRegistry`** (`src/simlin-engine/src/module_functions.rs:209-214`): `macros: HashMap<String, ModuleFunctionDescriptor>`; `resolve_macro` at `:283-286`. **`ModuleFunctionDescriptor`** at `module_functions.rs:45-60`. **`MacroRegistry::build`** (`:230-280`) calls `check_for_recursion` (`:296-363`) which parses each body via `Expr0::new(formula, LexerType::Equation)` and walks it (`collect_called_macros`, `:419-470`) — so the body AST IS available at build time and the classification can be computed there.
- **The collision predicate** `is_renamed_builtin_macro_collision` (`module_functions.rs:180-182`) is `true` for `init`/`previous` and stdlib-module names — exactly the renamed-builtins. `is_renamed_builtin_macro_collision("init")` is `true`.
- **`LoadInitial`** (`src/simlin-engine/src/vm.rs:1352-1363`): in `StepPart::Initials` reads `curr[abs_off]` (so the referenced var must be ordered earlier in the Initials runlist); in flows reads the t=0 snapshot `initial_values[abs_off]` (snapshot taken at `vm.rs:1151-1155`).
- **Runlist/deps**: `model.rs` (`init_referenced_vars:122-129`, `module_output_deps:203-248`, `all_deps:301-486`) is the **legacy** path; the **production** path is salsa: `VariableDeps.init_referenced_vars` at `db.rs:854-857`/`944`, module deps at `db.rs:881-896`. Any contingent secondary fix must target the salsa path (and likely `model.rs` for parity).
- **Proven reference test:** `helper_recurrence_mdl_synthetic_helper_in_scc_simulates` at `tests/simulate.rs:2377` (fixture `test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl`), exercising `ecc[tNext] = INITIAL(ecc[tPrev]*2)` over a subrange — the **bare-INITIAL, no-macro** path. Expected `ecc[t1]=1, ecc[t2]=2, ecc[t3]=4` constant across saved steps. This is what the macro-routed version must equal.
- **Test-helper caveat:** `ensure_results` skips implicit module vars (`tests/test_helpers.rs:81-87`; `is_implicit_module_var` matches the `$\u{205A}` prefix at `:29-31`). Fixtures must assert the USER-FACING variable (e.g. `ecc[t1]`), not the implicit `$⁚...` helper.
- **Inline harness:** `tests/simulate.rs::run_inline_mdl` (`:3032-3040`) → `Results`; `element_series(&r, "ecc[t1]")` (`:1831-1842`); `macro_test_value_at` (`:3014-3028`). Inline `:MACRO:` example: `simulates_macro_independent_invocation_state` (`:3305-3339`). `simulate_mdl_path(path)` (`:763`) for file fixtures with a sibling reference.
- **Feasibility (confirmed):** the call-site collapse is sound and the safer design option. The strict structural match (single param; single primary-output body equation that is exactly `App(BUILTIN, [Var(the_sole_param)])`; the call name canonicalizes to a renamed-builtin collision name) cannot misfire on a non-passthrough macro that merely shares a name. NO NA-arithmetic change is made (`float.rs::NA` is left untouched).

---

<!-- START_TASK_1 -->
### Task 1: Add failing INIT-macro recurrence + scalar fixtures (RED)

**Verifies:** clearn-residual.AC3.1, clearn-residual.AC3.2

**Files:**
- Test (scalar, inline): `src/simlin-engine/tests/simulate.rs` (new `#[test]` using `run_inline_mdl` near the other inline `:MACRO:` tests, e.g. by `simulates_macro_independent_invocation_state`).
- Fixture (element-wise): copy `test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl` to a new fixture directory (e.g. `test/test-models/tests/macro_init_recurrence/`) and PREPEND a `:MACRO: INIT(x) = INITIAL(x)` block; copy the sibling reference output (the `helper_recurrence` expected `.dat`/`output.tab`) into the new directory. The recurrence text is unchanged — its existing `INITIAL(...)` is renamed to `INIT(...)` at import and now collides with the new macro.
- Test (element-wise): `src/simlin-engine/tests/simulate.rs` — a `#[test]` calling `simulate_mdl_path("../../test/test-models/tests/macro_init_recurrence/<file>.mdl")` (file_io). First confirm how `simulate_mdl_path` locates the reference for the existing fixtures and follow that convention.

**Implementation:**
- **Scalar (AC3.1):** inline MDL defining `:MACRO: INIT(x) = INITIAL(x)` and a scalar capture, e.g. `captured = INIT(growing)` with `growing = Time`. `INITIAL(Time)` captures `INITIAL TIME` at t0, so `captured` must be constant `== INITIAL TIME` across all saved steps. Assert the series is flat at `INITIAL TIME` via `macro_test_value_at`/`element_series`.
- **Element-wise (AC3.2):** the `macro_init_recurrence` fixture is `helper_recurrence.mdl` + the `:MACRO: INIT` block. Expected output is identical to `helper_recurrence` (`ecc[t1]=1, ecc[t2]=2, ecc[t3]=4` constant across saved steps). Assert via the same reference comparison the existing fixture uses (which skips implicit `$⁚` vars per `ensure_results`).

**Testing:**
- AC3.1: scalar `captured` series is constant at `INITIAL TIME` for every saved step.
- AC3.2: per-element `ecc[t1]/[t2]/[t3]` equal `1/2/4` at every saved step (no drop to `0` at t≥1, no `:NA:`).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate <scalar_test> <element_test>`
Expected: **FAILS** (RED) — the INIT-macro collision routes through the buggy synthetic module: the recurrence drops to `0`/`:NA:` at t≥1 and/or the scalar capture is not held constant.

**Commit:** `engine: add failing INIT passthrough-macro recurrence fixtures`
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-4) -->

<!-- START_TASK_2 -->
### Task 2: Pure passthrough-classification function + unit tests

**Verifies:** clearn-residual.AC3.4

**Files:**
- Modify: `src/simlin-engine/src/module_functions.rs` (add a pure classifier, e.g. `fn classify_passthrough(macro_name, parameter_ports, primary_output_body_ast) -> Option<PassthroughBuiltin>` and a `PassthroughBuiltin` type, or a `bool` if a single target suffices — `init` is the only live target, but keep the structural test general over `is_renamed_builtin_macro_collision`).
- Test: `src/simlin-engine/src/module_functions.rs` `#[cfg(test)]` (unit tests).

**Implementation:**
A pure function that returns `Some(passthrough target)` iff ALL of:
- the macro has exactly one parameter (`parameter_ports.len() == 1`);
- the primary-output body equation AST is exactly `App(UntypedBuiltinFn(call, [arg]), _)` (a single call with a single argument);
- `arg` is exactly `Var(the sole parameter)` (the bare parameter, not an expression like `param*2`);
- `canonicalize(call) == canonicalize(macro_name)` (a self-call — the form the importer's rename produces, e.g. `INIT = INIT(x)`); and
- `is_renamed_builtin_macro_collision(canonicalize(call))` is `true` (so the fall-through to the existing intrinsic routing is valid and the target is a real opcode-backed builtin).
Otherwise `None`. Keep it purely structural over the parsed AST (functional core), with no registry/IO access, so it is unit-testable in isolation.

**Testing:** unit tests:
- `INIT = INIT(x)` (single param `x`) → `Some(init)` (positive).
- `INIT = INIT(x) + 1` (Op2 body) → `None` (AC3.4 negative).
- `INIT = INIT(x * 2)` (arg not the bare param) → `None` (AC3.4 negative).
- two-parameter macro whose body is `F(a)` → `None` (arity).
- a macro with the body `ABS(x)` where `abs` is NOT a renamed-builtin collision → `None` (not opcode-backed via this path).
- a multi-output macro (additional outputs present) → `None`.

**Verification:**
Run: `cargo test -p simlin-engine --lib classify_passthrough`
Expected: all pass.

**Commit:** `engine: add pure passthrough-macro classifier`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Thread the passthrough classification onto the descriptor

**Verifies:** clearn-residual.AC3.4 (registry-level wiring of the classifier)

**Files:**
- Modify: `src/simlin-engine/src/module_functions.rs` — add a field to `ModuleFunctionDescriptor` (`:45-60`), e.g. `passthrough: Option<PassthroughBuiltin>`; populate it in `MacroRegistry::build` (`:230-280`) by parsing the primary-output body equation (reuse the `Expr0::new` parse already performed by `check_for_recursion`, or parse the primary-output equation once) and calling the Task 2 classifier. Non-macro descriptors and non-passthrough macros get `None`.
- Test: `src/simlin-engine/src/module_functions.rs` `#[cfg(test)]` — build a `MacroRegistry` from a small project containing `:MACRO: INIT(x) = INITIAL(x)` and assert its descriptor's `passthrough == Some(init)`; build one with `:MACRO: INIT(x) = INITIAL(x) + 1` and assert `passthrough == None`.

**Implementation:**
Compute the classification once at registry-build time (the only place the body AST is available), store it on the descriptor, and keep `salsa::Update`/`Clone`/`Eq` derivations intact (`ModuleFunctionDescriptor` and `MacroRegistry` derive these). Do not change `expand_module_function`'s signature. Ensure the new field participates in equality so salsa invalidation is correct.

**Testing:**
- The INIT passthrough macro is classified `Some`; the `INITIAL(x)+1` near-miss is `None` (registry-level AC3.4).

**Verification:**
Run: `cargo test -p simlin-engine --lib macro_registry passthrough`
Expected: pass.
Run: `cargo build -p simlin-engine` (salsa derives compile).

**Commit:** `engine: classify passthrough macros at registry build time`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Call-site passthrough collapse (GREEN)

**Verifies:** clearn-residual.AC3.1, clearn-residual.AC3.2, clearn-residual.AC3.3, clearn-residual.AC3.5

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs:623-638` (the macro-resolution branch in `walk`).

**Implementation:**
At the call site, when `resolve_macro(&func)` returns a descriptor whose `passthrough.is_some()` and the arity matches, SKIP `expand_module_function` and fall through to the existing intrinsic routing (which, because the passthrough is a self-call, sees `func` canonicalizing to the renamed builtin — e.g. `init` — and emits `LoadInitial` with the existing `make_temp_arg` hoisting for the expression argument). Concretely, restructure so the descriptor is obtained first and the expansion is guarded:
```rust
let is_renamed_builtin_self_call = self.is_enclosing_macro_renamed_builtin_self_call(&func);
let descriptor = if is_renamed_builtin_self_call { None } else { self.macro_registry.resolve_macro(&func) };
if let Some(descriptor) = descriptor {
    if descriptor.passthrough.is_none() {
        let descriptor = descriptor.clone();
        return self.expand_module_function(&descriptor, &func, args, loc);
    }
    // genuine passthrough macro: fall through to the renamed-builtin intrinsic
    // routing (func canonicalizes to the opcode-backed builtin), exactly as the
    // #554 self-call exception does inside a macro body.
}
```
Add a comment explaining the self-call invariant that makes the fall-through valid (the classifier guarantees `canonicalize(call) == canonicalize(macro_name)`, so `func` routes to the right intrinsic). Make NO NA-arithmetic change.

**Testing:**
- Task 1 fixtures pass (GREEN): scalar capture constant; element-wise recurrence = `1/2/4` across all steps.
- AC3.3: `helper_recurrence_mdl_synthetic_helper_in_scc_simulates` (the bare-INITIAL, no-macro path) still passes unchanged.
- All macro-expansion tests pass (the #554 in-body collapse and the Phase 2 RAMP FROM TO behavior are unaffected — RAMP FROM TO is not a passthrough, so `passthrough == None` and it still expands as a module).
- AC3.5: observe (informational) that, after this collapse, the `--ignored` C-LEARN `SAMPLE UNTIL`-fed bases (`last_set_target_year`, `last_active_target_year`, `time_from_target_to_ultimate_target`, `target_emissions_for_rate`, `ultimate_target_value_from_rate`, `depth_at_bottom`, `emissions_with_stopped_growth`) move toward reconciliation — final reconciliation/attribution is Phase 4. No dedicated `SAMPLE UNTIL` change is made.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate <task1 tests> helper_recurrence`
Expected: Task 1 tests GREEN; `helper_recurrence` still green.
Run: `cargo test -p simlin-engine --lib macro` and `cargo test -p simlin-engine` (default suite)
Expected: green.
Observe: `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored clearn_residual_exactness` reports the #591-c1 bases as `shrank` (records which reconciled — feeds Task 5's decision and Phase 4).

**Commit:** `engine: collapse trivial passthrough macros to the builtin opcode at the call site`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_5 -->
### Task 5: (Contingent) Fix residual synthetic-module ordering/propagation — only if needed

**Verifies:** clearn-residual.AC3.2 (only if a non-passthrough INITIAL-macro module remains divergent)

**Entry criteria (do this task ONLY if true):** After Task 4, the `--ignored` `clearn_residual_exactness` still shows one or more of the #591-c1 bases (`last_set_target_year` … `emissions_with_stopped_growth`) failing, AND root-causing shows the cause is a *non-passthrough* macro-module output (an INITIAL-derived module not collapsible by Task 4, or the Phase 2 `RAMP FROM TO` module) whose value is mis-ordered in the Initials runlist or mis-propagated in the flows phase. If, instead, Task 4 reconciled these bases, SKIP this task and record "not needed — passthrough collapse sufficed" with the observed evidence.

**Files (if performed):**
- `src/simlin-engine/src/db.rs` (salsa, production): `VariableDeps.init_referenced_vars` (`:854-857`/`:944`), module-output deps (`:881-896`).
- `src/simlin-engine/src/vm.rs`: Initials vs flows `LoadInitial` (`:1352-1363`).
- For parity: `src/simlin-engine/src/model.rs` (`module_output_deps:203-248`, `all_deps:301-486`).

**Implementation (if performed):** Use the systematic-debugging skill — reproduce on a minimal non-passthrough INITIAL-macro-module fixture (build it; do not rely on C-LEARN), confirm the root cause is Initials topological ordering or flows-phase module-output propagation, fix it generally in the salsa path (with `model.rs` parity), and add the minimal fixture as a regression test asserting the user-facing series. Do not key on any C-LEARN name.

**Verification (if performed):**
Run: the new minimal fixture test + `cargo test -p simlin-engine`.
Expected: green; the previously-divergent base reconciles in the `--ignored` gate.

**If skipped:** record the skip rationale (with the Task 4 `--ignored` evidence) in the Phase 3 completion notes / commit body. No code change.

**Commit (if performed):** `engine: fix synthetic-module INITIAL ordering/propagation`
<!-- END_TASK_5 -->

---

## Phase completion criteria

- Tasks 1-4 committed; the scalar and element-wise INIT-macro fixtures pass (RED before Task 4, GREEN after); `helper_recurrence` stays green (AC3.3); the passthrough classifier rejects near-misses (AC3.4).
- Task 5 either performed (with its minimal regression fixture green) or explicitly skipped with recorded evidence that the Task 4 collapse sufficed.
- `cargo test -p simlin-engine` (default, non-ignored) is green.
- NO NA-arithmetic change was made (`float.rs::NA` untouched).
- **Ignored C-LEARN gate note:** Do NOT prune `EXPECTED_VDF_RESIDUAL` here; Phase 4 re-measures and reconciles. The `--ignored` `clearn_residual_exactness` will report the reconciled #591-c1 bases as `shrank` — expected, closed in Phase 4. AC3.5 (`SAMPLE UNTIL` downstream) is verified there: those bases reconcile downstream of the INIT fix with no dedicated `SAMPLE UNTIL` change.

## No special-casing (hard constraint)

No change keys on a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list. The passthrough classifier is a general structural rule; the fixtures are small models (a `helper_recurrence` variant and an inline scalar capture) independent of C-LEARN.
