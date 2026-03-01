# Activate PREVIOUS/INIT Opcodes -- Phase 1

**Goal:** Make 1-arg PREVIOUS(x) and INIT(x) compile to direct LoadPrev/LoadInitial opcodes instead of module expansion, and ensure existing tests pass.

**Architecture:** Replace the blanket PREVIOUS/INIT gate in builtins_visitor.rs with an arity-based check. Remove prev_values seeding in vm.rs. Extend the Initials runlist in db.rs to include INIT-referenced variables (required for existing tests to pass -- pulled forward from design Phase 2).

**Tech Stack:** Rust (simlin-engine)

**Scope:** Covers design Phases 1 and 2 (combined because the Initials runlist fix is required for existing tests to pass after the gate change)

**Codebase verified:** 2026-03-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### prev-init-opcodes.AC1: 1-arg PREVIOUS compiles to LoadPrev
- **prev-init-opcodes.AC1.1 Success:** `PREVIOUS(x)` in a scalar user model equation emits `LoadPrev` opcode (not module expansion)
- **prev-init-opcodes.AC1.2 Success:** `PREVIOUS(x[DimA])` in an arrayed equation emits per-element `LoadPrev` with correct offsets

### prev-init-opcodes.AC2: INIT compiles to LoadInitial
- **prev-init-opcodes.AC2.1 Success:** `INIT(x)` in a user model equation emits `LoadInitial` opcode
- **prev-init-opcodes.AC2.2 Success:** `INIT(x)` in an aux-only model (no stocks) returns x's initial value correctly (Initials runlist includes x)
- **prev-init-opcodes.AC2.3 Failure:** `initial_values` snapshot is not all zeros when INIT references a variable that has no stock dependency

### prev-init-opcodes.AC3: 2-arg PREVIOUS preserved
- **prev-init-opcodes.AC3.1 Success:** `PREVIOUS(x, init_val)` still uses module expansion and produces identical results to pre-change behavior

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Replace blanket gate with arity-based check

**Verifies:** prev-init-opcodes.AC1.1, prev-init-opcodes.AC2.1, prev-init-opcodes.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs:328-345`

**Implementation:**

Replace the blanket PREVIOUS/INIT gate at lines 328-345 with an arity-based check:

```rust
// 2-arg PREVIOUS(x, init_val) falls through to module expansion.
// 1-arg PREVIOUS(x) and INIT(x) pass through as builtins --
// they compile to LoadPrev and LoadInitial opcodes respectively.
if func == "previous" && args.len() > 1 {
    // Fall through to module expansion for 2-arg form.
} else if is_builtin_fn(&func) {
    return Ok(App(UntypedBuiltinFn(func, args), loc));
}
```

This replaces the old comment block (lines 328-340) and the gate condition (lines 341-345). The key change: `func == "previous" || func == "init"` becomes `func == "previous" && args.len() > 1`.

After this change:
- 1-arg `PREVIOUS(x)`: `is_builtin_fn("previous")` returns true, passes through as `UntypedBuiltinFn` -> lowers to `BuiltinFn::Previous` in expr1.rs (line 255) -> compiles to `LoadPrev` in codegen.rs (lines 692-723)
- 2-arg `PREVIOUS(x, init_val)`: `args.len() > 1` is true, falls through to module expansion using stdlib `previous` model
- `INIT(x)`: `is_builtin_fn("init")` returns true, passes through -> lowers to `BuiltinFn::Init` (expr1.rs line 256) -> compiles to `LoadInitial` (codegen.rs lines 725-737)

No changes needed in A2A handling: `contains_stdlib_call()` (lines 31-54) still matches "previous" and "init" via `MODEL_NAMES`, so arrayed equations still trigger per-element expansion via `instantiate_implicit_modules`. The per-element visitor walks with `active_subscript` set, applying `substitute_dimension_refs` to args before the builtin passes through. This correctly resolves dimension references to concrete elements.

**Verification:**

Run: `cargo test -p simlin-engine test_previous_module_expansion`
Expected: This test may change behavior (1-arg PREVIOUS now uses opcode path). Continue to Task 2.

**Commit:** Do not commit yet (combine with Tasks 2-3)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove prev_values seeding after Initials phase

**Files:**
- Modify: `src/simlin-engine/src/vm.rs:887-890`

**Implementation:**

Remove the `prev_values` seeding lines (887-890) that copy curr[] into prev_values after the Initials phase:

```rust
// DELETE these 3 lines:
// Seed prev_values with the post-initials state so that
// PREVIOUS(x) at t=0 returns the initial value of x.
self.prev_values
    .copy_from_slice(&data[curr_start..curr_start + self.n_slots]);
```

Keep the `initial_values` seeding (lines 882-886) -- that's still needed for LoadInitial.

After this change, `prev_values` stays at its initialized-to-zeros state (line 507: `vec![0.0; n_slots]`). During the first DT step (Flows phase), `LoadPrev` reads from `prev_values` which is all zeros, so `PREVIOUS(x)` returns 0 at the first timestep. This matches the module expansion behavior for 1-arg PREVIOUS where `initial_value` defaults to 0.

The per-timestep snapshot at line 578 (`state.prev_values.copy_from_slice(curr)`) correctly maintains prev_values from timestep 1 onward.

**Verification:**

Not yet -- combine with Task 3.

**Commit:** Do not commit yet (combine with Task 3)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Extend Initials runlist for INIT-referenced variables

**Verifies:** prev-init-opcodes.AC2.2, prev-init-opcodes.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs:752-756` (VariableDeps struct)
- Modify: `src/simlin-engine/src/db.rs:770-837` (variable_direct_dependencies_impl)
- Modify: `src/simlin-engine/src/db.rs:1396-1406` (Initials runlist "needed" filter)

**Implementation:**

Without this fix, `INIT(x)` in an aux-only model (no stocks/modules) would have an empty Initials runlist -- `x` would never be evaluated during Initials, so `initial_values[x_offset]` stays 0.

**Step 1: Add `init_referenced_vars` field to `VariableDeps`**

In `src/simlin-engine/src/db.rs`, find the `VariableDeps` struct (around line 752):

```rust
pub struct VariableDeps {
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
    pub implicit_vars: Vec<ImplicitVarDeps>,
}
```

Add a new field:

```rust
pub struct VariableDeps {
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
    pub implicit_vars: Vec<ImplicitVarDeps>,
    /// Variables referenced by BuiltinFn::Init in this variable's equation.
    /// These must be included in the Initials runlist so their values are
    /// captured in the initial_values snapshot.
    pub init_referenced_vars: BTreeSet<String>,
}
```

**Step 2: Populate `init_referenced_vars` in `variable_direct_dependencies_impl`**

In `variable_direct_dependencies_impl` (line 770), after computing `dt_deps` and `initial_deps` from the lowered AST (lines 812-826), walk the lowered AST for `BuiltinFn::Init` references.

Create a helper function in `src/simlin-engine/src/db.rs` (near the `VariableDeps` struct). Follow the `IdentifierSetVisitor::walk` pattern from `variable.rs:698` -- match all `Expr2` variants and recurse, using `walk_builtin_expr` for `App`:

```rust
/// Collect variable identifiers referenced by BuiltinFn::Init calls in an AST.
fn init_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    fn walk(expr: &Expr2, result: &mut BTreeSet<String>) {
        match expr {
            Expr2::Const(_, _, _) => {}
            Expr2::Var(_, _, _) => {}
            Expr2::App(builtin, _, _) => {
                // Check if this is Init specifically -- extract the referenced var
                if let BuiltinFn::Init(arg) = builtin {
                    if let Expr2::Var(ident, _, _) = arg.as_ref() {
                        result.insert(ident.to_string());
                    }
                }
                // Recurse into all builtin subexpressions (handles nested Init too)
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(_, _) => {}
                    BuiltinContents::Expr(expr) => walk(expr, result),
                });
            }
            Expr2::Subscript(_, args, _, _) => {
                for arg in args {
                    match arg {
                        IndexExpr2::Expr(expr) | IndexExpr2::Range(expr, _, _) => {
                            walk(expr, result);
                        }
                        _ => {}
                    }
                    if let IndexExpr2::Range(_, end, _) = arg {
                        walk(end, result);
                    }
                }
            }
            Expr2::Op2(_, l, r, _, _) => {
                walk(l, result);
                walk(r, result);
            }
            Expr2::Op1(_, l, _, _) => {
                walk(l, result);
            }
            Expr2::If(cond, t, f, _, _) => {
                walk(cond, result);
                walk(t, result);
                walk(f, result);
            }
        }
    }
    match ast {
        Ast::Scalar(expr) => walk(expr, &mut result),
        Ast::ApplyToAll(_, expr) => walk(expr, &mut result),
        Ast::Arrayed(_, map, default_expr, _) => {
            for expr in map.values() {
                walk(expr, &mut result);
            }
            if let Some(expr) = default_expr {
                walk(expr, &mut result);
            }
        }
    }
    result
}
```

This mirrors the `IdentifierSetVisitor::walk` pattern in `variable.rs:698` but only collects identifiers from `BuiltinFn::Init` args. Uses `walk_builtin_expr` (from `builtins.rs:411`) for recursive traversal of builtin subexpressions.

Then in `variable_direct_dependencies_impl`, after line 826:

```rust
let init_referenced_vars = match lowered.ast() {
    Some(ast) => init_referenced_idents(ast),
    None => BTreeSet::new(),
};
```

And add it to the return:

```rust
VariableDeps {
    dt_deps,
    initial_deps,
    implicit_vars,
    init_referenced_vars,
}
```

Also update the Module branch (line 783) to include `init_referenced_vars: BTreeSet::new()`.

**Step 3: Update Initials runlist "needed" filter**

In `model_dependency_graph_impl` (around line 1396), update the "needed" set to also include INIT-referenced variables.

Before the `let needed` declaration, collect all init-referenced vars:

```rust
let init_referenced: HashSet<&String> = var_info_sources
    .values()
    .flat_map(|deps| deps.init_referenced_vars.iter())
    .collect();
```

Wait -- `var_info` doesn't store `init_referenced_vars`. We need access to the `VariableDeps` which contains it. The current code only extracts `VarInfo` (is_stock, is_module, deps) from `VariableDeps`.

The cleanest approach: collect init-referenced var names during the var_info population loop (lines 1138-1212). When processing each variable's `VariableDeps`, also accumulate the `init_referenced_vars` into a separate set.

Add before the `var_info` loop:

```rust
let mut all_init_referenced: HashSet<String> = HashSet::new();
```

Inside the loop, after `var_info.insert(...)` at line 1195:

```rust
all_init_referenced.extend(deps.init_referenced_vars.iter().cloned());
```

Then update the "needed" filter at lines 1398-1405:

```rust
let needed: HashSet<&String> = var_names
    .iter()
    .filter(|n| {
        var_info
            .get(n.as_str())
            .map(|i| i.is_stock || i.is_module)
            .unwrap_or(false)
            || all_init_referenced.contains(*n)
    })
    .collect();
```

This adds any variable referenced by `INIT()` to the Initials runlist seed, ensuring it's evaluated during the Initials phase and its value is captured in `initial_values`.

**Testing:**

Tests must verify each AC listed above:
- prev-init-opcodes.AC1.1: Build a scalar model with `PREVIOUS(x)`, verify it compiles (no module expansion for PREVIOUS). The existing `test/previous/model.stmx` uses 2-arg PREVIOUS so it still module-expands -- verify it still passes.
- prev-init-opcodes.AC2.1: Build a model with `INIT(x)`, verify it compiles to LoadInitial.
- prev-init-opcodes.AC2.2: The existing `test/builtin_init/builtin_init.stmx` model is aux-only with `INIT(increasing)`. Verify it produces correct results (init_of_increasing = 1, the initial value of TIME).
- prev-init-opcodes.AC2.3: Verify that `initial_values` is correctly populated (the test above implicitly verifies this).
- prev-init-opcodes.AC3.1: The existing `test/previous/model.stmx` uses 2-arg PREVIOUS. Verify it still produces identical results to the reference output.

Follow project testing patterns from `src/simlin-engine/src/test_common.rs` (TestProject builder) for programmatic tests and `src/simlin-engine/tests/simulate.rs` for integration tests.

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All existing tests pass, including `simulates_previous`, `simulates_init_builtin`, and all LTM tests.

**Commit:** `engine: activate LoadPrev/LoadInitial opcodes for 1-arg PREVIOUS and INIT`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_4 -->
### Task 4: Run full test suite and commit

**Files:** None (verification only)

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All tests pass. Key tests to watch:
- `simulates_previous` -- 2-arg PREVIOUS still works via module expansion
- `simulates_init_builtin` -- 1-arg INIT now uses LoadInitial opcode
- `test_previous_module_expansion_interpreter_vm_parity` in db_tests.rs
- `test_init_module_expansion_interpreter_vm_parity` in db_tests.rs
- All LTM tests in simulate_ltm.rs

If `test_previous_and_init_still_in_stdlib_model_names` in db_tests.rs fails (it asserts "init" is still in MODEL_NAMES), that's expected -- update or mark it for Phase 3 (it will be fully addressed in Phase 3).

**Commit:**
```bash
git add src/simlin-engine/src/builtins_visitor.rs src/simlin-engine/src/vm.rs src/simlin-engine/src/db.rs
git commit -m "engine: activate LoadPrev/LoadInitial opcodes for 1-arg PREVIOUS and INIT"
```
<!-- END_TASK_4 -->
