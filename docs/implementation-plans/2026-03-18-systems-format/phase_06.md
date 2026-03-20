# Systems Format Support - Phase 6: ALLOCATE BY PRIORITY Builtin

**Goal:** Implement ALLOCATE BY PRIORITY as a first-class engine builtin that desugars to ALLOCATE AVAILABLE with rectangular priority profiles during Expr0->Expr1 lowering.

**Architecture:** `ALLOCATE BY PRIORITY(request, priority, size, width, supply)` is syntactic sugar for `ALLOCATE AVAILABLE(request, pp, supply)` where `pp[i] = (1, priority[i], width, 0)` (rectangular profile). The desugaring happens in `ast/expr1.rs` during `Expr1::from`, reusing the entire existing `AllocateAvailable` pipeline. The MDL `xmile_compat.rs` translation is simplified to pass through `allocate_by_priority` as a native XMILE function call, and the MDL writer's `recognize_allocate` is updated to recognize the native form.

**Tech Stack:** Rust

**Scope:** 7 phases from original design (phase 6 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- `BuiltinFn::AllocateAvailable(Box<Expr>, Box<Expr>, Box<Expr>)` is an existing 3-arg variant in `builtins.rs` (line 104)
- `"allocate_available"` is registered in `is_builtin_fn` at line 402
- `check_arity!` macro in `ast/expr1.rs` validates arg counts; `"allocate_available" => check_arity!(AllocateAvailable, 3)` at line 254
- There is NO `check_arity!` variant for 5 args -- will need one or manual validation
- `xmile_compat.rs` currently handles MDL `ALLOCATE BY PRIORITY` via `format_allocate_by_priority_ctx` (lines 536-583), which reorders 5 args into a 5-arg `ALLOCATE(...)` XMILE function
- `writer.rs` `recognize_allocate` (lines 373-425) matches XMILE `allocate()` 5-arg form and reconstructs MDL `ALLOCATE BY PRIORITY` syntax
- `alloc.rs` `allocate_available` takes `(requests, profiles, avail)` where profiles are `(ptype, ppriority, pwidth, pextra)` tuples; rectangular profile: ptype=1
- The `size`/`ignore` parameter (arg 3) is discarded in translation
- No `allocate_by_priority` entry exists anywhere in the engine currently
- `builtins.rs` traversal helpers (`try_map`, `for_each_expr_ref`, `walk_builtin_expr`) need arms for any new variant
- `constify_dimensions` in `expr1.rs` would need coverage for a new variant if it persists past lowering (it shouldn't, since it's desugared)

---

## Acceptance Criteria Coverage

This phase implements and tests:

### systems-format.AC5: ALLOCATE BY PRIORITY works as native builtin
- **systems-format.AC5.1 Success:** ALLOCATE BY PRIORITY in XMILE equations compiles and executes correctly
- **systems-format.AC5.2 Success:** Results match ALLOCATE AVAILABLE with equivalent rectangular priority profiles
- **systems-format.AC5.3 Success:** Existing MDL allocation tests continue to pass
- **systems-format.AC5.4 Success:** MDL writer emits ALLOCATE BY PRIORITY syntax for the Vensim form

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Register AllocateByPriority builtin variant

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` -- add `BuiltinFn::AllocateByPriority` variant, register in `is_builtin_fn`, add traversal arms

**Implementation:**

Add the new variant to the `BuiltinFn` enum:
```rust
// ALLOCATE BY PRIORITY(request, priority, size, width, supply)
AllocateByPriority(Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>),
```

Register in `is_builtin_fn`:
```rust
| "allocate_by_priority"
```
(Add to the existing list of array-only builtins near `"allocate_available"`)

Add the `name()` arm:
```rust
AllocateByPriority(_, _, _, _, _) => "allocate_by_priority",
```

Add arms to ALL traversal methods that already handle `AllocateAvailable`:
- `try_map` -- visit all 5 sub-expressions
- `map` -- visit all 5 sub-expressions
- `for_each_expr_ref` -- visit all 5 sub-expressions
- `walk_builtin_expr` -- visit all 5 sub-expressions

These are at the same locations as the existing `AllocateAvailable` arms (lines ~242, ~326, ~497 area). Follow the exact same pattern, just with 5 args instead of 3.

**Testing:**

Compilation verification only (the variant is exercised in Task 2).

**Verification:**

Run:
```bash
cargo build -p simlin-engine
```

Expected: Builds without errors. No unused-variant warnings because traversal arms are present.

**Commit:** `engine: register AllocateByPriority builtin variant`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement Expr0->Expr1 desugaring

**Verifies:** systems-format.AC5.1, AC5.2

**Files:**
- Modify: `src/simlin-engine/src/ast/expr1.rs` -- add `"allocate_by_priority"` lowering arm

**Implementation:**

In the `Expr1::from` function's `match id.as_str()` block (lines 162-277), add a new arm:

```rust
"allocate_by_priority" => {
    // ALLOCATE BY PRIORITY(request, priority, size, width, supply)
    //   -> ALLOCATE AVAILABLE(request, pp, supply)
    //   where pp[i] = (1, priority[i], width, 0)
    if args.len() != 5 {
        return eqn_err!(BadBuiltinArgs, loc, id, 5);
    }
    let request = args.remove(0);
    let priority = args.remove(0);
    let _size = args.remove(0);  // ignored (Vensim compat)
    let width = args.remove(0);
    let supply = args.remove(0);

    // Synthesize the priority profile: array where each element is
    // (ptype=1, ppriority=priority[i], pwidth=width, pextra=0)
    // The synthesized expression needs to produce a flat array of
    // (1, priority, width, 0) tuples compatible with AllocateAvailable.
    // Since priority is already an array and width is a scalar, we
    // construct a 2D array expression matching the expected profile layout.
    // ... implementation details depend on how AllocateAvailable expects
    // the profile array at the Expr1 level

    Expr1::App(
        BuiltinFn::AllocateByPriority(
            Box::new(request),
            Box::new(priority),
            Box::new(_size),
            Box::new(width),
            Box::new(supply),
        ),
        loc,
    )
}
```

**Important design consideration:** The desugaring must produce an `Expr1` that the existing `AllocateAvailable` compilation path can consume. Looking at how `AllocateAvailable` is compiled (in `compiler/codegen.rs` and `vm.rs`), the priority profile is a 2D array where each row is `[ptype, ppriority, pwidth, pextra]`. The synthesized profile needs to construct this from the scalar `width` and array `priority`.

**Recommended approach:** Keep `AllocateByPriority` as a distinct variant through compilation to the bytecode level. Add a new `Opcode::AllocateByPriority` in `bytecode.rs` and a VM dispatch case in `vm.rs` that constructs rectangular priority profiles `(1.0, priority[i], width, 0.0)` at runtime and delegates to `alloc::allocate_available`. This avoids synthesizing complex array expressions at the AST level and follows the existing `AllocateAvailable` opcode pattern.

Concretely:
1. Add `Opcode::AllocateByPriority` variant in `bytecode.rs`
2. In `compiler/codegen.rs`, emit `AllocateByPriority` when encountering the `BuiltinFn::AllocateByPriority` variant
3. In `vm.rs`, dispatch `AllocateByPriority` by reading request array, priority array, and width/supply scalars from the stack, constructing `Vec<(f64, f64, f64, f64)>` profiles inline, and calling `allocate_available`
4. Add `AllocateByPriority` to `constify_dimensions` in `expr1.rs` for dimension propagation

**Testing:**

Tests must verify:
- AC5.1: Create a `TestProject` with an array aux that uses `allocate_by_priority(request[D], priority[D], 0, 2, supply)`. Verify it compiles and produces correct output via both VM and interpreter paths.
- AC5.2: Create an equivalent `TestProject` using `allocate_available` with explicit rectangular priority profiles. Verify both produce identical results.

Test with a concrete scenario:
- Dimension `D` with 3 elements
- `request[D]` = [10, 20, 30]
- `priority[D]` = [3, 1, 2]
- `width` = 1
- `supply` = 35
- Expected: higher priority (3) gets full allocation first, then priority 2, then 1 gets remainder

**Verification:**

Run:
```bash
cargo test -p simlin-engine allocate_by_priority
```

**Commit:** `engine: implement ALLOCATE BY PRIORITY desugaring to ALLOCATE AVAILABLE`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Simplify MDL translation and update writer

**Verifies:** systems-format.AC5.3, AC5.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs` -- simplify `format_allocate_by_priority_ctx` to emit `allocate_by_priority(...)` directly
- Modify: `src/simlin-engine/src/mdl/writer.rs` -- update `recognize_allocate` to handle the native form

**Implementation:**

**xmile_compat.rs changes:**

Currently `format_allocate_by_priority_ctx` (lines 536-583) reorders MDL args into a 5-arg `ALLOCATE(...)` XMILE function. Simplify to emit `allocate_by_priority(request, priority, size, width, supply)` as a native XMILE function call, since the engine now understands this natively:

```rust
fn format_allocate_by_priority_ctx(&self, args: &[Expr<'_>], ctx: &Ctx) -> Result<String> {
    // MDL: ALLOCATE BY PRIORITY(demand, priority, size, width, supply)
    // XMILE: allocate_by_priority(demand, priority, size, width, supply)
    // Args pass through in same order
    let formatted_args: Vec<String> = args.iter()
        .map(|arg| self.format_expr_ctx(arg, ctx))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("allocate_by_priority({})", formatted_args.join(", ")))
}
```

**writer.rs changes:**

Update `recognize_allocate` to handle the native form. When the XMILE expression is `allocate_by_priority(request, priority, size, width, supply)`, emit MDL `ALLOCATE BY PRIORITY(request, priority, size, width, supply)`:

Add a new case in `try_recognize_pattern` or in `recognize_allocate` itself:
```rust
if f == "allocate_by_priority" && args.len() == 5 {
    // Native form: args are already in MDL order
    // Emit: ALLOCATE BY PRIORITY(request, priority, size, width, supply)
    ...
}
```

**Testing:**

- AC5.3: Run existing MDL allocation tests to verify they still pass:
  ```bash
  cargo test --features "file_io,testing" --test simulate simulates_allocate
  ```
- AC5.4: Write a unit test that creates a project with `allocate_by_priority` in XMILE, round-trips through MDL writer, and verifies the MDL output contains `ALLOCATE BY PRIORITY`.

**Verification:**

Run:
```bash
cargo test -p simlin-engine allocate
cargo test --features "file_io,testing" --test simulate simulates_allocate
```

Expected: All tests pass, including existing MDL allocation tests.

**Commit:** `engine: simplify MDL ALLOCATE BY PRIORITY to use native engine support`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
