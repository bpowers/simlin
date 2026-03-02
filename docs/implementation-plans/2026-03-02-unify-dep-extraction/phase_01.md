# Unify PREVIOUS/INIT Dependency Extraction -- Phase 1: Unified Walker and DepClassification

**Goal:** Replace the 5 overlapping AST-walk functions in `variable.rs` with a single `classify_dependencies()` function returning a `DepClassification` struct, then convert the old functions to thin wrappers.

**Architecture:** A `ClassifyVisitor` struct combines `IdentifierSetVisitor`'s dimension filtering and `IsModuleInput` branch selection with `in_previous`/`in_init` state flags and multiple accumulators. After walking, derived sets (`previous_only`, `init_only`) are computed via set difference. Old public functions become one-line wrappers delegating to `classify_dependencies()`.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 5 phases from original design (phase 1 of 5)

**Codebase verified:** 2026-03-02

---

## Acceptance Criteria Coverage

This phase implements:

### unify-dep-extraction.AC1: Single unified dependency analysis pass
- **unify-dep-extraction.AC1.1 Success:** `classify_dependencies()` on a scalar equation with mixed references (`PREVIOUS(a) + INIT(b) + c`) returns correct `all`, `previous_only`, `init_only`, `init_referenced`, `previous_referenced` sets in one call
- **unify-dep-extraction.AC1.2 Success:** `classify_dependencies()` handles `ApplyToAll` and `Arrayed` AST variants, walking all element expressions and default expressions
- **unify-dep-extraction.AC1.3 Success:** `IsModuleInput` branch selection works correctly when `module_inputs` is provided -- only the active branch's deps are collected
- **unify-dep-extraction.AC1.4 Success:** `IndexExpr2::Range` endpoints are walked and dimension-element names are filtered out
- **unify-dep-extraction.AC1.5 Success:** The 5 old functions (`identifier_set`, `init_referenced_idents`, `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`) are removed or reduced to thin wrappers
- **unify-dep-extraction.AC1.6 Edge:** Nested `PREVIOUS(PREVIOUS(x))` correctly classifies `x` as previous_only at both nesting levels

### unify-dep-extraction.AC0: Regression Safety
- **unify-dep-extraction.AC0.1 Success:** All existing simulation tests (`tests/simulate.rs`) pass at each phase boundary
- **unify-dep-extraction.AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary

---

## Reference files

Read these CLAUDE.md files for project conventions before implementing:
- `/home/bpowers/src/simlin/CLAUDE.md` (project root)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (engine crate)
- `/home/bpowers/src/simlin/docs/dev/rust.md` (Rust coding standards)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Implement `DepClassification` struct and `classify_dependencies()` function

**Verifies:** unify-dep-extraction.AC1.1, unify-dep-extraction.AC1.2, unify-dep-extraction.AC1.3, unify-dep-extraction.AC1.4, unify-dep-extraction.AC1.6

**Files:**
- Modify: `src/simlin-engine/src/variable.rs` -- add `DepClassification`, `ClassifyVisitor`, and `classify_dependencies()` above the existing `IdentifierSetVisitor` (around line 673)

**Implementation:**

Add a public `DepClassification` struct and a private `ClassifyVisitor` struct, followed by a public `classify_dependencies()` function. Place these ABOVE the existing `IdentifierSetVisitor` block (which starts at line 674).

**`DepClassification` struct:**

```rust
/// Result of classifying all dependency categories from a single AST walk.
///
/// Replaces the five separate AST-walking functions that previously computed
/// these categories independently: `identifier_set`, `init_referenced_idents`,
/// `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`,
/// and `init_only_referenced_idents_with_module_inputs`.
pub struct DepClassification {
    /// All referenced identifiers (current + lagged + init-only).
    /// Dimension names are filtered out. Replaces `identifier_set`.
    pub all: HashSet<Ident<Canonical>>,
    /// Idents appearing as direct args to INIT() calls.
    /// Replaces `init_referenced_idents`.
    pub init_referenced: BTreeSet<String>,
    /// Idents appearing as direct args to PREVIOUS() calls.
    /// Replaces `previous_referenced_idents`.
    pub previous_referenced: BTreeSet<String>,
    /// Idents referenced ONLY inside PREVIOUS() -- not outside it.
    /// Replaces `lagged_only_previous_idents_with_module_inputs`.
    pub previous_only: BTreeSet<String>,
    /// Idents referenced ONLY inside INIT() or PREVIOUS() -- not outside either.
    /// Replaces `init_only_referenced_idents_with_module_inputs`.
    pub init_only: BTreeSet<String>,
}
```

**`ClassifyVisitor` struct:**

```rust
/// Unified AST walker that computes all dependency categories in a single pass.
///
/// Maintains two boolean flags (`in_previous`, `in_init`) to track whether the
/// current position is inside a PREVIOUS() or INIT() call. Accumulates identifiers
/// into multiple sets:
///
/// - `all`: every referenced identifier, with dimension names filtered (same as
///   `IdentifierSetVisitor`)
/// - `init_referenced` / `previous_referenced`: direct Var/Subscript args of
///   INIT() / PREVIOUS() calls
/// - `non_previous`: idents seen outside any PREVIOUS() context
/// - `non_init`: idents seen outside both INIT() and PREVIOUS() context
///
/// After walking, derived sets are computed:
/// - `previous_only = previous_referenced - non_previous`
/// - `init_only = init_referenced - non_init`
///
/// The walker preserves `IdentifierSetVisitor`'s behaviors: dimension-name
/// filtering from index expressions, `IsModuleInput` branch selection via
/// `module_inputs`, and `IndexExpr2::Range` endpoint walking.
struct ClassifyVisitor<'a> {
    all: HashSet<Ident<Canonical>>,
    init_referenced: BTreeSet<String>,
    previous_referenced: BTreeSet<String>,
    non_previous: BTreeSet<String>,
    non_init: BTreeSet<String>,
    dimensions: &'a [Dimension],
    module_inputs: Option<&'a BTreeSet<Ident<Canonical>>>,
    in_previous: bool,
    in_init: bool,
}
```

**`ClassifyVisitor` method implementations:**

The impl block needs these methods, matching the patterns of both `IdentifierSetVisitor` and the standalone functions:

`is_dimension_or_element(&self, ident: &str) -> bool`: Identical to `IdentifierSetVisitor::is_dimension_or_element` (lines 682-696). Checks dimension names via `canonicalize(dim.name())` and element names via `named_dim.get_element_index(ident)`.

`walk_index_expr(&mut self, expr: &Expr2)`: Identical to `IdentifierSetVisitor::walk_index_expr` (lines 699-707). Filters bare `Expr2::Var` nodes against `is_dimension_or_element` before calling `self.walk(expr)`.

`walk_index(&mut self, e: &IndexExpr2)`: Identical to `IdentifierSetVisitor::walk_index` (lines 709-724). Dispatches `Range(start, end, _)` to `walk_index_expr` for both endpoints; handles `Wildcard`, `StarRange`, `DimPosition` as no-ops; dispatches `Expr(expr)` to `walk_index_expr`.

`record_ident(&mut self, ident_str: &str)`: Helper that records an identifier string in the flag-dependent sets. Called for both `Var` and `Subscript` ident names:
```rust
fn record_ident(&mut self, ident_str: &str) {
    if !self.in_previous {
        self.non_previous.insert(ident_str.to_owned());
    }
    // PREVIOUS() context also excludes from non_init, matching the existing
    // behavior of init_only_referenced_idents_with_module_inputs (line 1079)
    // where BuiltinFn::Previous sets in_init=true.
    if !self.in_init && !self.in_previous {
        self.non_init.insert(ident_str.to_owned());
    }
}
```

`walk(&mut self, e: &Expr2)`: The main walk method combines `IdentifierSetVisitor::walk` with the flag-tracking from the standalone functions. Key dispatch:

- `Expr2::Const(_, _, _)` -- no-op
- `Expr2::Var(id, _, _)` -- dimension check for `all` (same as IdentifierSetVisitor lines 729-739: check `self.dimensions.iter().any(|dim| id.as_str() == &*canonicalize(dim.name()))`; if not a dimension, insert `id.clone()` into `self.all`). Then call `self.record_ident(id.as_str())` unconditionally (the flag-dependent sets use the raw string and don't filter dimensions, matching the standalone functions).
- `Expr2::Subscript(id, args, _, _)` -- insert `id.clone()` into `self.all` (no dimension filter, matching IdentifierSetVisitor line 750). Call `self.record_ident(id.as_str())`. Walk indices via `self.walk_index(arg)`.
- `Expr2::App(builtin, _, _)` -- dispatch on builtin:
  - `BuiltinFn::Previous(arg)` -- extract direct arg name: if `arg` is `Var(ident, _, _)` or `Subscript(ident, _, _, _)`, insert `ident.to_string()` into `self.previous_referenced`. Save `self.in_previous`, set `self.in_previous = true`, call `self.walk(arg)`, restore old value.
  - `BuiltinFn::Init(arg)` -- same pattern for `init_referenced`. Save `self.in_init`, set `self.in_init = true`, call `self.walk(arg)`, restore old value.
  - All other builtins -- delegate to `walk_builtin_expr(builtin, |contents| ...)` exactly as `IdentifierSetVisitor` does (lines 742-747): for `BuiltinContents::Ident(id, _loc)`, insert `Ident::new(id)` into `self.all`; for `BuiltinContents::Expr(expr)`, call `self.walk(expr)`.
- `Expr2::Op2(_, l, r, _, _)` -- walk both operands
- `Expr2::Op1(_, l, _, _)` -- walk operand
- `Expr2::If(cond, t, f, _, _)` -- `IsModuleInput` branch selection identical to `IdentifierSetVisitor` (lines 761-775): if `self.module_inputs` is `Some` and `cond` is `App(BuiltinFn::IsModuleInput(ident, _))`, check `module_inputs.contains(&*canonicalize(ident))` to select branch; otherwise walk all three.

**`classify_dependencies()` function:**

```rust
/// Classify all dependency categories of an AST in a single walk.
///
/// Returns a `DepClassification` with five sets:
/// - `all`: every referenced identifier (dimension names filtered)
/// - `init_referenced` / `previous_referenced`: direct args of INIT/PREVIOUS calls
/// - `previous_only`: idents referenced ONLY inside PREVIOUS (not outside)
/// - `init_only`: idents referenced ONLY inside INIT or PREVIOUS (not outside either)
///
/// This replaces five separate functions that previously required up to 10 calls
/// per variable. The walker applies `IsModuleInput` branch selection when
/// `module_inputs` is provided, and filters dimension/element names from index
/// expressions.
pub fn classify_dependencies(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> DepClassification {
    let mut visitor = ClassifyVisitor {
        all: HashSet::new(),
        init_referenced: BTreeSet::new(),
        previous_referenced: BTreeSet::new(),
        non_previous: BTreeSet::new(),
        non_init: BTreeSet::new(),
        dimensions,
        module_inputs,
        in_previous: false,
        in_init: false,
    };
    match ast {
        Ast::Scalar(expr) => visitor.walk(expr),
        Ast::ApplyToAll(_, expr) => visitor.walk(expr),
        Ast::Arrayed(_, elements, default_expr, _) => {
            for expr in elements.values() {
                visitor.walk(expr);
            }
            if let Some(default_expr) = default_expr {
                visitor.walk(default_expr);
            }
        }
    }
    let previous_only = visitor
        .previous_referenced
        .difference(&visitor.non_previous)
        .cloned()
        .collect();
    let init_only = visitor
        .init_referenced
        .difference(&visitor.non_init)
        .cloned()
        .collect();
    DepClassification {
        all: visitor.all,
        init_referenced: visitor.init_referenced,
        previous_referenced: visitor.previous_referenced,
        previous_only,
        init_only,
    }
}
```

**Important behavioral note:** The `init_referenced` and `previous_referenced` sets in the unified walker apply `IsModuleInput` branch pruning when `module_inputs` is `Some(...)`. The old standalone `init_referenced_idents` and `previous_referenced_idents` did NOT prune branches. This only affects callers that pass `module_inputs`; callers passing `None` see identical behavior. The thin wrappers in Task 2 pass `&[]` for dimensions and `None` for module_inputs, preserving exact compatibility for their current callers.

**Testing:**

Existing tests verify the old functions produce correct results. After Task 2 converts those functions to wrappers, the same tests verify the unified walker:
- `test_identifier_sets` (line 1137): exercises `identifier_set` with dimension filtering and IsModuleInput
- `test_init_only_referenced_idents` (line 1202): exercises `init_only_referenced_idents_with_module_inputs` with INIT, PREVIOUS+INIT, and dotted module refs
- `test_range_end_expressions_are_walked_in_init_previous_helpers` (line 1233): exercises range-endpoint walking for PREVIOUS and INIT

**Verification:**

```bash
cargo test -p simlin-engine
```
Expected: all existing tests pass (compilation of the new code is verified; behavioral correctness is verified after Task 2 wires up the wrappers).

**Commit:** `engine: add DepClassification struct and classify_dependencies()`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Convert old functions to thin wrappers over `classify_dependencies()`

**Verifies:** unify-dep-extraction.AC1.5

**Files:**
- Modify: `src/simlin-engine/src/variable.rs` -- replace function bodies of all 5 old functions with one-line delegations

**Implementation:**

Replace the bodies (NOT the signatures) of all 5 public functions. Each becomes a thin wrapper. Preserve existing doc comments and function signatures exactly so that all external callers continue to compile without changes.

**`identifier_set` (lines 780-803) becomes:**
```rust
pub fn identifier_set(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> HashSet<Ident<Canonical>> {
    classify_dependencies(ast, dimensions, module_inputs).all
}
```

Delete the `IdentifierSetVisitor` struct and its impl block (lines 674-778) -- it is fully replaced by `ClassifyVisitor`.

**`init_referenced_idents` (lines 809-872) becomes:**
```rust
pub fn init_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    classify_dependencies(ast, &[], None).init_referenced
}
```
Passing `&[]` for dimensions and `None` for module_inputs matches the original behavior: no dimension filtering (empty dimensions means `is_dimension_or_element` always returns false), no branch pruning (None module_inputs means all If branches are walked).

**`previous_referenced_idents` (lines 877-940) becomes:**
```rust
pub fn previous_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    classify_dependencies(ast, &[], None).previous_referenced
}
```

**`lagged_only_previous_idents_with_module_inputs` (lines 945-1037) becomes:**
```rust
pub fn lagged_only_previous_idents_with_module_inputs(
    ast: &Ast<Expr2>,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> BTreeSet<String> {
    classify_dependencies(ast, &[], module_inputs).previous_only
}
```
Passes `&[]` for dimensions (the original function never did dimension filtering) and forwards `module_inputs` for IsModuleInput branch selection.

**`init_only_referenced_idents_with_module_inputs` (lines 1042-1135) becomes:**
```rust
pub fn init_only_referenced_idents_with_module_inputs(
    ast: &Ast<Expr2>,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> BTreeSet<String> {
    classify_dependencies(ast, &[], module_inputs).init_only
}
```

Preserve the existing doc comments on each function (lines 805-808, 874-876, 942-944, 1039-1041).

**Testing:**

The existing tests now exercise `classify_dependencies()` through the wrapper functions:
- `test_identifier_sets`: calls `identifier_set` which delegates to `classify_dependencies().all`
- `test_init_only_referenced_idents`: calls `init_only_referenced_idents_with_module_inputs` which delegates to `classify_dependencies().init_only`
- `test_range_end_expressions_are_walked_in_init_previous_helpers`: calls `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_referenced_idents`, `init_only_referenced_idents_with_module_inputs` -- all now wrappers

All external callers in db.rs (lines 872, 880, 889, 893, 900, 907), db_implicit_deps.rs (lines 87, 94, 101, 108, 115), model.rs (lines 121, 267, 269, 275, 1582, 1614, 1615), ltm.rs (line 191), ltm_augment.rs (lines 522, 609), db_ltm.rs (line 417), and the re-export in lib.rs (line 81) continue to compile without changes because signatures are preserved.

**Verification:**

```bash
cargo test -p simlin-engine
```
Expected: all tests pass. The wrappers produce identical results to the old implementations.

```bash
cargo test -p simlin-engine --features file_io
```
Expected: integration tests in `tests/simulate.rs` pass, confirming no behavioral regressions in the full compilation pipeline.

**Commit:** `engine: convert dep-extraction functions to classify_dependencies wrappers`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
