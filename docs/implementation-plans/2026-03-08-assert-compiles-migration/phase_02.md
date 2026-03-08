# Assert-Compiles Migration -- Phase 2: Array-Reduce Abstraction and MEAN Dynamic Ranges

**Goal:** Unify the array-reduce codegen pattern across all builtins and fix MEAN with dynamic range arguments.

**Architecture:** Extract a shared `emit_array_reduce(&mut self, arg: &Expr, opcode: Opcode) -> Result<Option<()>>` helper in `codegen.rs` that encapsulates the `walk_expr_as_view(arg) + push(opcode) + push(PopView) + return Ok(Some(()))` pattern. Refactor SUM, SIZE, STDDEV, and the 1-arg paths of MIN/MAX to use it. For MEAN, restructure the handler: when `args.len() == 1`, call `emit_array_reduce` directly (like SUM/SIZE/STDDEV do), which routes through `walk_expr_as_view` and correctly handles `Expr::Subscript` with `Range` indices. The multi-argument scalar MEAN path remains for `args.len() > 1`.

**Tech Stack:** Rust (simlin-engine)

**Scope:** Phase 2 of 3 from design plan

**Codebase verified:** 2026-03-08

**Reference files for executor:**
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- engine architecture and module map
- `/home/bpowers/src/simlin/CLAUDE.md` -- project-wide development standards

---

## Acceptance Criteria Coverage

This phase implements and tests:

### assert-compiles-migration.AC2: MEAN with dynamic ranges works on incremental path
- **assert-compiles-migration.AC2.1 Success:** `MEAN(data[start_idx:end_idx])` with variable bounds compiles and simulates correctly
- **assert-compiles-migration.AC2.2 Success:** Existing SUM, SIZE, STDDEV, VMIN, VMAX behavior unchanged after refactoring to shared helper

---

## Codebase Verification Findings

- **Confirmed:** `BuiltinFn::Mean(args)` handler at `codegen.rs:865-897`. The `is_array` check at line 871 matches only `Expr::StaticSubscript` and `Expr::TempArray`, missing `Expr::Subscript` with `Range` indices. This is the bug.
- **Confirmed:** SUM (line 915), SIZE (line 901), STDDEV (line 908) all call `walk_expr_as_view(arg)` unconditionally with no `is_array` guard. MIN/MAX (lines 794-823) use `Option<Box<Expr>>` arity dispatch, calling `walk_expr_as_view` for 1-arg form.
- **Confirmed:** The `walk_expr_as_view` -> ArrayOp -> PopView pattern is copy-pasted 6 times (SUM, SIZE, STDDEV, MIN 1-arg, MAX 1-arg, MEAN array path).
- **Confirmed:** `walk_expr_as_view` at line 281 handles `Expr::Subscript` with `SubscriptIndex::Range` via `ViewRangeDynamic` opcode. The plumbing works -- SUM/SIZE/STDDEV already use it for dynamic ranges.
- **Confirmed:** `mean_with_dynamic_range` test at `array_tests.rs:2010-2026` uses `assert_compiles()`. Adjacent `size_with_dynamic_range` (line 2028) and `stddev_with_dynamic_range` (line 2045) use `assert_compiles_incremental()`, confirming those builtins already work.
- **Confirmed:** `BuiltinFn::Mean` takes `Vec<Expr>` (variadic). `Sum`/`Size`/`Stddev` take `Box<Expr>`. `Min`/`Max` take `(Box<Expr>, Option<Box<Expr>>)`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Extract `emit_array_reduce` helper

**Verifies:** assert-compiles-migration.AC2.2 (no behavior change for existing builtins)

**Files:**
- Modify: `src/simlin-engine/src/compiler/codegen.rs`

**Implementation:**

Add a private method to the codegen struct:

```rust
/// Emit the array-reduce pattern: push view, emit reduction opcode, pop view.
/// Used by SUM, SIZE, STDDEV, MIN (1-arg), MAX (1-arg), and MEAN (1-arg).
fn emit_array_reduce(&mut self, arg: &Expr, opcode: Opcode) -> Result<Option<()>> {
    self.walk_expr_as_view(arg)?;
    self.push(opcode);
    self.push(Opcode::PopView {});
    Ok(Some(()))
}
```

Then refactor each builtin handler to use it:

**SUM** (currently lines 915-921):
```rust
BuiltinFn::Sum(arg) => {
    return self.emit_array_reduce(arg, Opcode::ArraySum {});
}
```

**SIZE** (currently lines 901-907):
```rust
BuiltinFn::Size(arg) => {
    return self.emit_array_reduce(arg, Opcode::ArraySize {});
}
```

**STDDEV** (currently lines 908-914):
```rust
BuiltinFn::Stddev(arg) => {
    return self.emit_array_reduce(arg, Opcode::ArrayStddev {});
}
```

**MAX** 1-arg path (currently lines 794-806):
```rust
BuiltinFn::Max(a, b) => {
    if let Some(b) = b {
        self.walk_expr(a)?.unwrap();
        self.walk_expr(b)?.unwrap();
        let id = self.curr_code.intern_literal(0.0);
        self.push(Opcode::LoadConstant { id });
    } else {
        return self.emit_array_reduce(a, Opcode::ArrayMax {});
    }
}
```

**MIN** 1-arg path (currently lines 809-823):
```rust
BuiltinFn::Min(a, b) => {
    if let Some(b) = b {
        self.walk_expr(a)?.unwrap();
        self.walk_expr(b)?.unwrap();
        let id = self.curr_code.intern_literal(0.0);
        self.push(Opcode::LoadConstant { id });
    } else {
        return self.emit_array_reduce(a, Opcode::ArrayMin {});
    }
}
```

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All existing tests pass (no behavior change).

**Commit:** `engine: extract emit_array_reduce helper for array-reduce builtins`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Fix MEAN to use `emit_array_reduce` for single-arg form

**Verifies:** assert-compiles-migration.AC2.1, assert-compiles-migration.AC2.2

**Files:**
- Modify: `src/simlin-engine/src/compiler/codegen.rs:865-897` (the `BuiltinFn::Mean` handler)

**Implementation:**

Replace the MEAN handler. When `args.len() == 1`, call `emit_array_reduce` directly (like SUM/SIZE/STDDEV), eliminating the `is_array` check entirely. The multi-arg scalar path remains for `args.len() > 1`:

```rust
BuiltinFn::Mean(args) => {
    if args.len() == 1 {
        return self.emit_array_reduce(&args[0], Opcode::ArrayMean {});
    }

    // Multi-argument scalar mean: (arg1 + arg2 + ... + argN) / N
    let id = self.curr_code.intern_literal(0.0);
    self.push(Opcode::LoadConstant { id });

    for arg in args.iter() {
        self.walk_expr(arg)?.unwrap();
        self.push(Opcode::Op2 { op: Op2::Add });
    }

    let id = self.curr_code.intern_literal(args.len() as f64);
    self.push(Opcode::LoadConstant { id });
    self.push(Opcode::Op2 { op: Op2::Div });
    return Ok(Some(()));
}
```

This removes the `is_array` check entirely for single-arg MEAN, routing all single-arg cases through `walk_expr_as_view`. Since `walk_expr_as_view` handles `StaticSubscript`, `TempArray`, `Var`, AND `Subscript` (with `Range`), all array forms now work including dynamic ranges.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All existing tests pass, including all MEAN-related tests.

**Commit:** `engine: fix MEAN to handle dynamic range subscripts via emit_array_reduce`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Switch `mean_with_dynamic_range` test to incremental path

**Verifies:** assert-compiles-migration.AC2.1

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs:2010-2026` (`mean_with_dynamic_range` test)

**Implementation:**

Change `project.assert_compiles()` to `project.assert_compiles_incremental()`. Remove the comment "MEAN with dynamic ranges is not yet supported on the incremental path". Keep `assert_sim_builds()` and `assert_scalar_result()` calls unchanged.

**Verification:**
Run: `cargo test -p simlin-engine mean_with_dynamic_range`
Expected: Test passes.

**Commit:** `engine: switch mean_with_dynamic_range test to incremental path`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->
