# Unify PREVIOUS/INIT Dependency Extraction -- Phase 3: Module-Backed Classifier Unification

**Goal:** Extract the shared module-classification predicate so `collect_module_idents()` and `builtins_visitor` routing use the same function for stdlib-call detection, eliminating duplicated name-matching logic.

**Architecture:** Two functions currently independently check whether a function name is a stdlib module function: `equation_is_stdlib_call()` in model.rs and `contains_stdlib_call()` in builtins_visitor.rs. Both hardcode the same set of names (`MODEL_NAMES` + aliases `delay`/`delayn`/`smthn`) but differ in structural behavior (top-level-only vs recursive, different PREVIOUS/INIT handling). The shared core -- "is this function name a stdlib module function?" -- is extracted to a single `pub(crate)` predicate. Both callers use it, each adding their own structural logic on top.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 5 phases from original design (phase 3 of 5)

**Codebase verified:** 2026-03-02

---

## Acceptance Criteria Coverage

This phase implements and tests:

### unify-dep-extraction.AC3: Authoritative module-backed classifier
- **unify-dep-extraction.AC3.1 Success:** `collect_module_idents()` and `builtins_visitor` PREVIOUS/INIT routing use the same predicate function for stdlib-call detection
- **unify-dep-extraction.AC3.2 Success:** No duplicated logic for determining whether an equation expands to a module

### unify-dep-extraction.AC0: Regression Safety
- **unify-dep-extraction.AC0.1 Success:** All existing simulation tests (`tests/simulate.rs`) pass at each phase boundary
- **unify-dep-extraction.AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary

---

## Reference files

Read these CLAUDE.md files for project conventions before implementing:
- `/home/bpowers/src/simlin/CLAUDE.md` (project root)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (engine crate)

---

## Prerequisites

Phase 1 must be complete.

---

## Codebase context

The investigation revealed that `equation_is_stdlib_call()` and `contains_stdlib_call()` are NOT simple duplicates -- they serve structurally different purposes:

| Function | Location | Level | Purpose |
|---|---|---|---|
| `equation_is_stdlib_call()` | model.rs:831-850 | Top-level only | "Does this equation's outermost call expand to a module?" Pre-scan for classifying variable NAMES. |
| `contains_stdlib_call()` | builtins_visitor.rs:31-54 | Recursive | "Does this expression contain any stdlib call needing per-element A2A expansion?" Walk-time decision. |

Key differences:
- `equation_is_stdlib_call` handles PREVIOUS specially: 1-arg = false (LoadPrev), 2+ args = true (module)
- `contains_stdlib_call` includes `"init"` as a trigger (INIT needs per-element temp vars in A2A context) but `equation_is_stdlib_call` does not
- `contains_stdlib_call` recurses into nested expressions; `equation_is_stdlib_call` only checks the top-level

The shared core is the NAME SET check: "is this function name (lowercased) one that expands to a stdlib module?" Both functions independently match against `crate::stdlib::MODEL_NAMES` plus the aliases `delay`, `delayn`, `smthn`.

---

<!-- START_TASK_1 -->
### Task 1: Extract `is_stdlib_module_function()` predicate

**Verifies:** unify-dep-extraction.AC3.1, unify-dep-extraction.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` -- add `pub(crate) fn is_stdlib_module_function()`
- Modify: `src/simlin-engine/src/model.rs:831-850` -- update `equation_is_stdlib_call()` to use the shared predicate
- Modify: `src/simlin-engine/src/builtins_visitor.rs:31-54` -- update `contains_stdlib_call()` to use the shared predicate

**Implementation:**

**Step 1: Add `is_stdlib_module_function` to `builtins.rs`.**

Place this near the existing `is_builtin_fn()` function in `src/simlin-engine/src/builtins.rs`. It belongs in `builtins.rs` because it is a predicate about builtin function semantics, alongside `is_builtin_fn()`.

```rust
/// Returns true if `func_name` (already lowercased) names a function that
/// expands to a stdlib module: the canonical names in `MODEL_NAMES` plus
/// the alias forms `delay`, `delayn`, and `smthn`.
///
/// This is the authoritative check shared by `equation_is_stdlib_call()`
/// (pre-scan name classification) and `contains_stdlib_call()` (walk-time
/// A2A expansion decision). Each caller adds its own structural logic on
/// top (e.g., PREVIOUS arg-count check, INIT inclusion for A2A).
pub(crate) fn is_stdlib_module_function(func_name: &str) -> bool {
    matches!(func_name, "delay" | "delayn" | "smthn")
        || crate::stdlib::MODEL_NAMES.contains(&func_name)
}
```

**Step 2: Update `equation_is_stdlib_call()` in model.rs (line 831-850).**

Currently (lines 842-849):
```rust
match &ast {
    Expr0::App(crate::builtins::UntypedBuiltinFn(func, args), _) => {
        let func_lower = func.to_lowercase();
        match func_lower.as_str() {
            "previous" => args.len() > 1,
            "delay" | "delayn" | "smthn" => true,
            _ => crate::stdlib::MODEL_NAMES.contains(&func_lower.as_str()),
        }
    }
    _ => false,
}
```

Replace the match body with:
```rust
match &ast {
    Expr0::App(crate::builtins::UntypedBuiltinFn(func, args), _) => {
        let func_lower = func.to_lowercase();
        // PREVIOUS(x) with 1 arg uses LoadPrev; 2+ args expand to a module.
        if func_lower == "previous" {
            args.len() > 1
        } else {
            crate::builtins::is_stdlib_module_function(&func_lower)
        }
    }
    _ => false,
}
```

Also promote `equation_is_stdlib_call` from `fn` (private) to `pub(crate) fn` so it can be reused if needed. Update its doc comment to reference `is_stdlib_module_function` as the underlying predicate.

**Step 3: Update `contains_stdlib_call()` in builtins_visitor.rs (lines 31-54).**

Currently (lines 36-41):
```rust
App(UntypedBuiltinFn(func, args), _) => {
    if crate::stdlib::MODEL_NAMES.contains(&func.as_str())
        || matches!(func.as_str(), "delay" | "delayn" | "smthn" | "init")
    {
        return true;
    }
    args.iter().any(contains_stdlib_call)
}
```

Replace with:
```rust
App(UntypedBuiltinFn(func, args), _) => {
    // INIT is included because it needs per-element temp vars in A2A
    // context, though it doesn't create a standalone module.
    if crate::builtins::is_stdlib_module_function(func.as_str())
        || func.as_str() == "init"
    {
        return true;
    }
    args.iter().any(contains_stdlib_call)
}
```

**Note on case sensitivity:** The current code uses `func.as_str()` for direct comparison, relying on the fact that function names in the lowered `Expr0` AST are already lowercase (the parser normalizes them). The replacement preserves this behavior exactly -- `is_stdlib_module_function` accepts a `&str` that is already lowercased, and the `"init"` check remains a direct string comparison. No case sensitivity change is introduced.

**Step 4: Document the `self.vars` runtime extension in builtins_visitor.rs.**

Add a doc comment to the `vars` field (line 124 of builtins_visitor.rs) or to `is_known_module_ident()` (line ~188) explaining that `self.vars` contains modules synthesized during the current walk, using the same `is_stdlib_module_function` classification rule. These are incremental additions to the base set from `collect_module_idents()`.

**Testing:**

Existing tests exercise both code paths:
- `collect_module_idents` is tested through the full compilation pipeline (all simulation tests use it)
- `builtins_visitor` PREVIOUS/INIT routing is tested through every model that uses SMOOTH, DELAY, PREVIOUS, INIT, TREND
- The `test_identifier_sets` test in variable.rs exercises IsModuleInput handling which depends on correct module_idents classification

No new tests are needed -- this is a pure refactoring of duplicated logic into a shared function. Behavioral equivalence is confirmed by existing tests passing.

**Verification:**

```bash
cargo test -p simlin-engine
```

```bash
cargo test -p simlin-engine --features file_io
```
Expected: all tests pass.

**Commit:** `engine: extract shared is_stdlib_module_function predicate`

<!-- END_TASK_1 -->
