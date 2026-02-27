# MDL Full Compatibility -- Phase 1: Tactical Fixes

**Goal:** Fix high-leverage bugs that cause cascading failures in existing tests and the C-LEARN model, bringing the C-LEARN equivalence test to zero diffs.

**Architecture:** Six targeted fixes across the MDL conversion pipeline: expression formatting, dimension alias handling, equation type coverage, compiler panic guards, stdlib module subscript context, and C-LEARN-specific equivalence issues (trailing tabs, initial-value comments, net flow synthesis, Unicode normalization).

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 1 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC6: No panics
- **mdl-full-compat.AC6.1 Success:** Compiler handles missing/sparse array element keys without panic (returns error)
- **mdl-full-compat.AC6.2 Success:** MDL conversion of any valid MDL file completes without panic (returns Result with errors)

---

## Reference Files

Testing conventions and infrastructure:
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- module map and test file purposes
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/CLAUDE.md` -- MDL parser status and commands
- `/home/bpowers/src/simlin/docs/dev/rust.md` -- Rust coding standards
- `/home/bpowers/src/simlin/src/simlin-engine/src/test_common.rs` -- `TestProject` builder for unit tests
- `/home/bpowers/src/simlin/src/simlin-engine/tests/mdl_equivalence.rs` -- C-LEARN equivalence test infrastructure

Key test commands:
- `cargo test -p simlin-engine` -- all engine unit tests
- `cargo test --features file_io --test simulate` -- simulation integration tests
- `cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture` -- C-LEARN equivalence
- `cargo test -p simlin-engine --features xmutil test_mdl_equivalence -- --nocapture` -- all MDL equivalence tests

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
## Subcomponent A: Core Conversion Fixes

<!-- START_TASK_1 -->
### Task 1: Fix `:NA:` emission to NaN in xmile_compat.rs

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs:107`

**Implementation:**

In `XmileFormatter::format_expr_ctx()` at line 107, change the `Expr::Na` arm from emitting `:NA:` (which is MDL-specific syntax the core equation parser doesn't understand) to emitting `NAN` (the XMILE-compatible representation):

```rust
// Before:
Expr::Na(_) => ":NA:".to_string(),

// After:
Expr::Na(_) => "NAN".to_string(),
```

This matches the existing handling of `A FUNCTION OF` at lines 219-222 which already emits `"NAN"`.

**Testing:**

Add a unit test in the existing `#[cfg(test)]` module of `xmile_compat.rs` that verifies `Expr::Na` formats to `"NAN"`. The test should construct an `Expr::Na` node and assert `format_expr()` returns `"NAN"`.

**Verification:**
Run: `cargo test -p simlin-engine mdl::xmile_compat`
Expected: All tests pass including the new one

**Commit:** `engine: emit NAN instead of :NA: in xmile-compat formatter`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add missing equation type arms to build_equation_rhs_with_context

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs:480-512`

**Implementation:**

The function `build_equation_rhs_with_context()` at line 473 handles per-element equation expansion for arrayed variables. Its catch-all arm at line 511 (`_ => (String::new(), None, None)`) silently returns empty strings for `Data`, `TabbedArray`, `NumberList`, and `Implicit` equation types. The companion function `build_equation()` at line 683 handles all these types properly.

Add explicit arms matching the patterns in `build_equation()`:

```rust
MdlEquation::Implicit(lhs) => {
    let gf = self.make_default_lookup();
    ("TIME".to_string(), None, Some(gf))
}
MdlEquation::Data(_lhs, expr) => {
    let eq_str = expr
        .as_ref()
        .map(|e| self.formatter.format_expr_with_context(e, ctx))
        .unwrap_or_default();
    (eq_str, None, None)
}
MdlEquation::TabbedArray(_lhs, _values) | MdlEquation::NumberList(_lhs, _values) => {
    // TabbedArray/NumberList per-element expansion is handled by
    // make_array_equation at the build_equation level. If we reach
    // here, the element-level equation is already decomposed --
    // return empty and let the caller handle it.
    (String::new(), None, None)
}
MdlEquation::EmptyRhs(_, _) => ("0+0".to_string(), None, None),
```

Replace the current catch-all `_ => (String::new(), None, None)` with these explicit arms plus an `unreachable!()` for `SubscriptDef` and `Equivalence` (those should never reach this function).

**Testing:**

Add a unit test in the existing `#[cfg(test)]` module of `variables.rs` that converts an MDL snippet containing a `Data` equation type with subscripts, verifying the conversion produces a non-empty equation string for each element.

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: handle Data/Implicit/EmptyRhs in per-element equation expansion`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Fix dimension alias casing preservation

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/dimensions.rs:97-98`

**Implementation:**

In `build_dimensions()` Phase 5 (line 89-109), when materializing equivalence dimensions as aliases, the name is built from the canonical (lowercased) source name. This loses the original casing from the MDL file, causing diffs with xmutil output.

The `equivalences` map stores `(src_canonical, dst_canonical)` from line 38. We need to also track the original source name. Change the approach:

1. In Phase 1b (line 31-39), store the original name alongside the canonical name. Change the `equivalences` field to store `(src_canonical, (original_src_name, dst_canonical))` or add a parallel map `equivalence_original_names: HashMap<String, String>` mapping canonical -> original name.

2. In Phase 5 (line 97-98), use the original name instead of the canonical name:

```rust
let alias = Dimension {
    name: space_to_underbar(original_src_name),  // preserve original casing
    elements: target_dim.elements.clone(),
    maps_to: Some(dst.clone()),
};
```

The `ConversionContext` struct (in `mod.rs` or `types.rs`) needs the new field. Check where `equivalences` is defined and add the original name tracking.

**Testing:**

Add a unit test verifying that an equivalence like `DimA <-> DimB` produces a dimension with name `DimA` (preserving case), not `dima`.

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass including the new one

**Commit:** `engine: preserve original casing for dimension equivalence aliases`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add compiler panic guards for sparse element keys

**Verifies:** mdl-full-compat.AC6.1

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs:563`
- Modify: `src/simlin-engine/src/compiler/mod.rs:665`

**Implementation:**

Two locations in the compiler use direct HashMap indexing `elements[&canonical_key]` which will panic if the key is absent (e.g., for EXCEPT equations or sparse arrays where not all subscript combinations have entries).

At line 563 (inside `is_initial` branch, `Ast::Arrayed` arm):
```rust
// Before:
let ast = &elements[&canonical_key];

// After:
let Some(ast) = elements.get(&canonical_key) else {
    return sim_err!(
        UnknownVariable,
        format!(
            "missing array element '{}' for variable '{}'",
            canonical_key.as_str(),
            var.ident()
        )
    );
};
```

Apply the same pattern at line 665 (non-initial branch, `Ast::Arrayed` arm).

**Testing:**

Add a unit test using the `TestProject` builder (`src/simlin-engine/src/test_common.rs`) that constructs an arrayed variable with sparse element keys (not all combinations present) and verifies compilation returns an error rather than panicking.

**Verification:**
Run: `cargo test -p simlin-engine compiler`
Expected: All tests pass, new test verifies error return instead of panic

**Commit:** `engine: return error instead of panicking on missing array element keys`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 5-6) -->
## Subcomponent B: Builtins Visitor and Lexer Fixes

<!-- START_TASK_5 -->
### Task 5: Provide subscript context for stdlib modules in Arrayed equations

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs` (around `instantiate_implicit_modules()`, line ~443)

**Implementation:**

The `instantiate_implicit_modules()` function in `builtins_visitor.rs` already handles `Ast::ApplyToAll` by iterating subscript elements and creating a `BuiltinVisitor::new_with_subscript_context()` for each element. However, for `Ast::Arrayed` expressions where each element has its own equation, if those element equations contain stdlib function calls (like SMOOTH, DELAY1, etc.), the visitor needs to know the specific subscript element to generate correctly-named module instantiations.

Check how `instantiate_implicit_modules()` handles the `Ast::Arrayed` variant. If it processes element equations without providing subscript context, each element's stdlib calls would generate modules with overlapping names. The fix is to pass the element's subscript information via `new_with_subscript_context()` when visiting each element's expression in the Arrayed case.

The existing pattern from `ApplyToAll` handling (which uses `SubscriptIterator::new(&dimensions)` to iterate elements and creates per-element visitors) should be replicated for the Arrayed case, but iterating over the explicit element map instead.

**Testing:**

Add a unit test that creates an MDL snippet with an arrayed variable (different equations per element) where at least one element uses a stdlib function (e.g., `SMOOTH`). Verify that conversion produces correctly-named module instantiations for each element.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: provide subscript context for stdlib modules in arrayed equations`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Strip trailing tabs from dimension element names in lexer

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/lexer.rs` (around the `symbol()` function, line ~727)

**Implementation:**

The `symbol()` function in the lexer strips trailing spaces from symbol names (lines 727-729) but does not strip trailing tabs. MDL files (especially C-LEARN) sometimes have trailing tabs on dimension element names, causing mismatches with xmutil output.

Extend the trailing character stripping to include tabs:

```rust
// Before:
while name.ends_with(' ') {
    name.pop();
}

// After:
while name.ends_with(' ') || name.ends_with('\t') {
    name.pop();
}
```

Note: There is already a similar pattern in the continuation-line path at lines 386-390 that strips both spaces and tabs. This change makes the non-continuation path consistent.

**Testing:**

Add a unit test in the lexer's `#[cfg(test)]` module that tokenizes a dimension definition with trailing tabs on element names and verifies the element names come through without trailing tabs. There is an existing test `symbols_strip_trailing_tabs()` at line ~1180 -- verify it covers this case or extend it.

**Verification:**
Run: `cargo test -p simlin-engine mdl::lexer`
Expected: All tests pass

**Commit:** `engine: strip trailing tabs from symbol names in MDL lexer`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-9) -->
## Subcomponent C: C-LEARN Equivalence Diff Resolution

<!-- START_TASK_7 -->
### Task 7: Run C-LEARN equivalence test and triage remaining diffs

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Read: `src/simlin-engine/tests/mdl_equivalence.rs` (lines 1004-1029, the `test_clearn_equivalence` test)

**Implementation:**

After applying Tasks 1-6, run the C-LEARN equivalence test to capture the current state of diffs:

```bash
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture 2>&1 | tee /tmp/clearn-diffs.txt
```

Capture the full output. The test at line 1004 uses `collect_project_diffs()` which produces structured diff output showing the path and detail of each difference.

Categorize remaining diffs into:
1. **Initial-value comments** -- variables where xmutil preserves an initial-value comment that the native parser doesn't extract (~4 expected)
2. **Middle-dot Unicode** -- dimension element names or variable names containing middle-dot character (U+00B7) that need normalization (~some)
3. **Net flow synthesis** -- differences in synthetic net flow variable generation for stocks (~some)
4. **GF y-scale** -- graphical function y-axis scaling differences (~some)
5. **Other** -- any remaining diffs

Document findings in `docs/implementation-plans/2026-02-26-mdl-full-compat/clearn-triage.md` with the categorized diffs and counts. This artifact persists across sessions and informs Phase 7's C-LEARN work.

**Verification:**
Run: `cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture`
Expected: Test shows reduced diff count (should be less than 26 after Tasks 1-6 fixes)

**Commit:** `doc: add C-LEARN equivalence triage results`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Fix initial-value comment extraction

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (equation building functions)
- Read: `src/simlin-engine/src/mdl/ast.rs` (FullEquation struct, line ~454)

**Implementation:**

The `FullEquation` struct in `ast.rs` has a `comment` field that captures the text between `~` delimiters in MDL equations. For some variables (particularly ApplyToAll-style), xmutil extracts this as an "initial comment" that ends up in the datamodel. The native converter doesn't preserve this information in the same way.

The exact fix depends on the diff analysis from Task 7. The pattern is:
- MDL format: `var = equation ~ units ~ comment |`
- The comment text between the second `~` and `|` may contain metadata that xmutil stores differently than the native parser

Review the specific diffs from Task 7 related to initial-value comments. The fix likely involves:
1. Reading the `comment` field from `FullEquation` during variable construction
2. Placing it in the correct datamodel field to match xmutil's output

**Testing:**

Add a unit test with an MDL variable that has a comment section, verifying the native parser produces the same datamodel output as expected.

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: extract initial-value comments from MDL equations`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Fix remaining C-LEARN equivalence diffs (middle-dot, net flow, GF y-scale)

**Verifies:** mdl-full-compat.AC6.2

**Files:**
- Potentially modify: `src/simlin-engine/src/mdl/lexer.rs` (Unicode normalization)
- Potentially modify: `src/simlin-engine/src/mdl/convert/stocks.rs` (net flow synthesis)
- Potentially modify: `src/simlin-engine/src/mdl/convert/variables.rs` or `helpers.rs` (GF y-scale)

**Implementation:**

Fix the remaining diffs identified in Task 7. Based on the design plan and known root causes:

**Middle-dot normalization:** The middle-dot character (U+00B7 `·`) appears in some MDL variable names. The lexer at line 971 accepts characters with codepoint > 127 as valid symbol characters. If xmutil normalizes middle-dot differently (e.g., to regular dot or underscore), add the same normalization in the lexer or in `space_to_underbar()`.

**Net flow synthesis:** The `link_stocks_and_flows()` method in `stocks.rs` (line 151) implements net flow synthesis. Differences may arise from:
- The naming pattern for synthetic flows (`generate_net_flow_name()` at line 363)
- The threshold for when net flow synthesis triggers vs decomposing rate expressions
- The handling of multi-stock flow conflicts (lines 287-295)

Compare xmutil output for specific variables identified in Task 7 and adjust the native logic to match.

**GF y-scale:** Graphical function y-axis scaling may differ in how min/max are computed from lookup table data. Check `build_graphical_function()` in `variables.rs` against xmutil's output for the specific lookup tables that differ.

Each individual diff fix should be committed separately if the fix is non-trivial.

**Testing:**

For each category of fix, add targeted unit tests verifying the native parser matches expected output for the specific patterns that caused diffs.

**Verification:**
Run: `cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture`
Expected: Zero diffs

**Commit:** `engine: fix remaining C-LEARN equivalence diffs`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_TASK_10 -->
### Task 10: Final verification -- all tests pass

**Verifies:** mdl-full-compat.AC6.1, mdl-full-compat.AC6.2

**Files:** None (verification only)

**Implementation:**

Run the full test suite to confirm no regressions:

**Step 1:** Run all engine unit tests
```bash
cargo test -p simlin-engine
```
Expected: All tests pass

**Step 2:** Run simulation integration tests
```bash
cargo test --features file_io --test simulate
```
Expected: All tests pass

**Step 3:** Run MDL equivalence tests
```bash
cargo test -p simlin-engine --features xmutil test_mdl_equivalence -- --nocapture
```
Expected: All tests pass

**Step 4:** Run C-LEARN equivalence test
```bash
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture
```
Expected: Zero diffs, test passes

**Step 5:** Run MDL roundtrip tests
```bash
cargo test -p simlin-engine --test mdl_roundtrip
```
Expected: All tests pass

**Commit:** No commit (verification only). If the C-LEARN equivalence test now passes, consider removing the `#[ignore]` annotation -- but only if the team wants it running in CI (it requires the `xmutil` feature which depends on the C++ library).
<!-- END_TASK_10 -->
