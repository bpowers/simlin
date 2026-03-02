# Activate PREVIOUS/INIT Opcodes -- Phase 3

**Goal:** Remove the unused INIT stdlib module and all references to it.

**Architecture:** Delete `stdlib/init.stmx`, remove the generated code in `stdlib.gen.rs`, remove "init" from `stdlib_args()`, and update tests that assert INIT's presence. Keep "init" in `contains_stdlib_call()` matching for correct A2A (array-to-array) expansion of arrayed INIT equations.

**Tech Stack:** Rust (simlin-engine)

**Scope:** Design Phase 4

**Codebase verified:** 2026-03-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### prev-init-opcodes.AC5: INIT stdlib model deleted
- **prev-init-opcodes.AC5.1 Success:** `stdlib/init.stmx` deleted, "init" removed from `MODEL_NAMES` and `stdlib_args`, no compilation errors

---

<!-- START_TASK_1 -->
### Task 1: Delete init.stmx and remove generated code

**Verifies:** prev-init-opcodes.AC5.1

**Files:**
- Delete: `stdlib/init.stmx`
- Modify: `src/simlin-engine/src/stdlib.gen.rs:24-40` (MODEL_NAMES, get function)
- Modify: `src/simlin-engine/src/stdlib.gen.rs:436-493` (init function)

**Implementation:**

**Step 1: Delete `stdlib/init.stmx`**

```bash
git rm stdlib/init.stmx
```

**Step 2: Remove "init" from MODEL_NAMES in `src/simlin-engine/src/stdlib.gen.rs`**

Change line 24 from:
```rust
pub const MODEL_NAMES: [&str; 8] = [
    "delay1", "delay3", "init", "npv", "previous", "smth1", "smth3", "trend",
];
```
to:
```rust
pub const MODEL_NAMES: [&str; 7] = [
    "delay1", "delay3", "npv", "previous", "smth1", "smth3", "trend",
];
```

**Step 3: Remove "init" from `get()` dispatch (line 32)**

Remove:
```rust
"init" => Some(init()),
```

**Step 4: Delete the `init()` function (lines 436-493)**

Remove the entire function.

**Step 5: Update `contains_stdlib_call()` in `builtins_visitor.rs`**

Since "init" is removed from `MODEL_NAMES`, `contains_stdlib_call()` will no longer match INIT calls. This breaks A2A expansion for arrayed equations containing `INIT(x[DimA])`.

In `src/simlin-engine/src/builtins_visitor.rs:36-39`, add "init" to the special-case list:

```rust
if crate::stdlib::MODEL_NAMES.contains(&func.as_str())
    || matches!(func.as_str(), "delay" | "delayn" | "smthn" | "init")
{
```

This preserves A2A expansion for INIT while removing the stdlib model. The pattern matches existing special cases for "delay"/"delayn"/"smthn" which are aliases not in MODEL_NAMES.

**Commit:** Do not commit yet (combine with Task 2)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove "init" from stdlib_args and update LTM classification

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs:15-27` (stdlib_args)
- Modify: `src/simlin-engine/src/ltm.rs:151` (Infrastructure classification)

**Implementation:**

**Step 1: Remove "init" from `stdlib_args()` in `builtins_visitor.rs`**

In `stdlib_args()` (lines 15-27), remove the "init" arm:

```rust
// DELETE this line:
"init" => &["input"],
```

After Phase 1, 1-arg INIT passes through as a builtin, so `stdlib_args("init")` is never called.

**Step 2: Update LTM classification in `ltm.rs`**

In `src/simlin-engine/src/ltm.rs:151`, the `classify_module_for_ltm` function checks for `"stdlib⁚init"`:

```rust
if name == "stdlib⁚previous" || name == "stdlib⁚init" {
    return ModuleLtmRole::Infrastructure;
}
```

Remove the `|| name == "stdlib⁚init"` clause:

```rust
if name == "stdlib⁚previous" {
    return ModuleLtmRole::Infrastructure;
}
```

After Phase 1, no `stdlib⁚init` module instances exist, so this check is dead code. Removing it is cleanup.

**Commit:** Do not commit yet (combine with Task 3)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Fix broken tests and commit

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs:4944-4959` (stdlib model names test)
- Modify: `src/simlin-engine/src/db_tests.rs:107-108` (model count assertion)
- Modify: `src/simlin-engine/src/ltm.rs:3283-3303` (test_classify_init_as_infrastructure)

**Implementation:**

**Step 1: Update `test_previous_and_init_still_in_stdlib_model_names` in `db_tests.rs` (line 4949)**

This test asserts "init" is in MODEL_NAMES. Two options:
- Remove the "init" assertion entirely (it's now correctly removed)
- Or rename the test and update the assertion

Recommended: remove the "init" assertion, keep the "previous" assertion, and update the test name/comment:

```rust
/// "previous" is still in MODEL_NAMES because 2-arg PREVIOUS(x, init_val)
/// still uses module expansion. "init" was removed -- 1-arg INIT now
/// compiles directly to LoadInitial.
#[test]
fn test_previous_still_in_stdlib_model_names() {
    let names = crate::stdlib::MODEL_NAMES;
    assert!(
        names.contains(&"previous"),
        "expected 'previous' in MODEL_NAMES (2-arg form still module-expanded)"
    );
    assert!(
        !names.contains(&"init"),
        "'init' should no longer be in MODEL_NAMES"
    );
}
```

**Step 2: Update model count assertion in `db_tests.rs` (line 108)**

Change from:
```rust
// 1 user model + 8 stdlib models
assert_eq!(result.project.model_names(&db).len(), 9);
```
to:
```rust
// 1 user model + 7 stdlib models (init stdlib removed)
assert_eq!(result.project.model_names(&db).len(), 8);
```

**Step 3: Update `test_classify_init_as_infrastructure` in `ltm.rs` (line 3283)**

This test looks up `stdlib⁚init` in a compiled LTM project and asserts it's classified as Infrastructure. After deletion, the model won't exist.

Delete or rewrite the test. Since the init model no longer exists, the test is no longer meaningful. Delete the test function (lines 3283-3303).

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All tests pass. The init stdlib model is fully removed.

**Commit:**
```bash
git add stdlib/init.stmx src/simlin-engine/src/stdlib.gen.rs src/simlin-engine/src/builtins_visitor.rs src/simlin-engine/src/ltm.rs src/simlin-engine/src/db_tests.rs
git commit -m "engine: delete init stdlib model"
```
<!-- END_TASK_3 -->
