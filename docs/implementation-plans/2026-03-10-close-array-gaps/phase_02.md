# Close Array Gaps Implementation Plan -- Phase 2

**Goal:** Replace the fragile text-matching heuristic for EXCEPT detection with an explicit flag, then fix cross-dimension mapping resolution so EXCEPT equations with mapped dimensions compile and simulate correctly.

**Architecture:** Two related changes: (1) add `has_except_default: bool` to `Equation::Arrayed` and thread it through serialization/deserialization, replacing the text-comparison heuristic in `should_apply_default_to_missing`; (2) extend dimension matching in the AST lowering pipeline and MDL converter to resolve cross-dimension references via `DimensionsContext` mappings.

**Tech Stack:** Rust (simlin-engine crate), protobuf

**Scope:** 6 phases from original design (this is phase 2 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` for test index; `/home/bpowers/src/simlin/docs/dev/rust.md` for Rust standards.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC3: EXCEPT default flag (#356)
- **close-array-gaps.AC3.1 Success:** `has_except_default: true` is set on EXCEPT-derived equations during MDL conversion
- **close-array-gaps.AC3.2 Success:** `has_except_default: false` is default for non-EXCEPT equations and XMILE-parsed equations
- **close-array-gaps.AC3.3 Success:** Proto round-trip preserves `has_except_default` flag
- **close-array-gaps.AC3.4 Success:** Text-comparison heuristic in `should_apply_default_to_missing` is replaced by boolean check

### close-array-gaps.AC4: Cross-dimension EXCEPT resolution (#345)
- **close-array-gaps.AC4.1 Success:** `simulates_except` integration test passes (except.mdl with DimD->DimA mapping)
- **close-array-gaps.AC4.2 Success:** `simulates_except2` integration test passes
- **close-array-gaps.AC4.3 Success:** Per-element equations correctly resolve cross-dimension references (e.g., `j[DimD]` becomes `j[D1]` when iterating at DimA=A2)
- **close-array-gaps.AC4.4 Success:** Expr2 dimension unification accepts mapped dimensions without MismatchedDimensions error
- **close-array-gaps.AC4.5 Edge:** xmutil-blocked XMILE tests remain ignored (out of scope)

---

## Codebase Verification Findings

- Confirmed: `Equation::Arrayed` at `datamodel.rs:192-207` is a 3-field tuple variant: `(Vec<DimensionName>, Vec<(ElementName, String, Option<String>, Option<GraphicalFunction>)>, Option<String>)`. The third field is `default_equation`.
- Confirmed: `has_except_default: bool` does NOT exist anywhere -- needs to be added as 4th field
- Confirmed: `should_apply_default_to_missing` at `variable.rs:387-416` uses text comparison: `eqn.trim() == default_eq.trim()`
- Confirmed: MDL convert captures `default_equation` at `convert/variables.rs:421-433` when `eq_has_except` is true
- Confirmed: `find_matching_dimension` at `expr2.rs:491-515` does NOT check dimension mappings -- only name equality and indexed-dimension size match
- Confirmed: `DimensionsContext` at `dimensions.rs:211` has `has_mapping_to()` (line 299) and `translate_via_mapping()` (line 464) already implemented
- Confirmed: Proto `ArrayedEquation` has only fields 1 and 2 -- next available field is 3
- Confirmed: `serde.rs:295` silently drops `default_equation` from proto; `serde.rs:337-352` hardcodes `None` on deserialize; `serde.rs:2238-2246` has `validate_equation_for_protobuf` guard that rejects `Some(default_equation)` (removal deferred to Phase 4)
- Confirmed: `json.rs` already handles `default_equation` round-trip correctly (test at `json.rs:3562`)
- Confirmed: XMILE deserialization at `xmile/variables.rs:138` hardcodes `None` for default_equation
- Confirmed: `simulates_except` (line 620) and `simulates_except2` (line 627) in `tests/simulate.rs` are `#[ignore]`d due to cross-dimension mapping issues, NOT basic EXCEPT parsing
- Confirmed: Basic EXCEPT works -- `simulates_except_basic_mdl` (line 653) is an active passing test
- Confirmed: `simulates_except_xmile` and `simulates_except_xmile_interpreter_only` remain ignored (xmutil drops EXCEPT semantics, out of scope)
- Finding: The AST layer (`variable.rs`) already has an `apply_default_to_missing: bool` concept computed from `should_apply_default_to_missing` -- the new `has_except_default` field replaces this computation with a pre-computed flag from MDL conversion

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add has_except_default to Equation::Arrayed and update all pattern matches

**Verifies:** close-array-gaps.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs:192-207` (Equation enum)
- Modify: all files matching `Equation::Arrayed` -- pattern matches throughout the codebase

**Implementation:**

Add `bool` as the 4th field of the `Equation::Arrayed` tuple variant, representing `has_except_default`:

```rust
Arrayed(
    Vec<DimensionName>,
    Vec<(ElementName, String, Option<String>, Option<GraphicalFunction>)>,
    Option<String>,  // default_equation
    bool,            // has_except_default
),
```

Note: `Equation` derives salsa's `Update` trait. Adding a `bool` field is compatible since `bool` implements `Update`. No salsa-related changes are needed.

Search for all `Equation::Arrayed` pattern matches across the codebase and update them. The new field defaults to `false` in all existing construction sites except MDL EXCEPT conversion (Task 2). Key locations:

- `datamodel.rs` -- any helper methods or Default impls
- `variable.rs` -- parse_equation and should_apply_default_to_missing
- `mdl/convert/variables.rs` -- Arrayed construction (both EXCEPT and non-EXCEPT paths)
- `serde.rs` -- proto serialize/deserialize
- `json.rs` -- JSON serialize/deserialize
- `xmile/variables.rs` -- XMILE deserialization (hardcode false)
- `compiler/` -- any pattern matches on Equation
- `db.rs` -- any pattern matches

Use `cargo build 2>&1 | head -100` iteratively to find all locations that need updating.

**Testing:**

- close-array-gaps.AC3.2: Verify `false` default by checking that all existing tests still pass (no behavioral change from defaulting to false)

**Verification:**

```bash
cargo build -p simlin-engine
cargo test -p simlin-engine
```

Expected: Compiles and all existing tests pass.

**Commit:** `engine: add has_except_default field to Equation::Arrayed`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Set has_except_default in MDL convert and replace text heuristic

**Verifies:** close-array-gaps.AC3.1, close-array-gaps.AC3.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs:403-491` (EXCEPT equation construction)
- Modify: `src/simlin-engine/src/variable.rs:387-416` (should_apply_default_to_missing)

**Implementation:**

1. In `mdl/convert/variables.rs`, when building `Equation::Arrayed` at line 491, set `has_except_default` to `true` when `eq_has_except` is true (the EXCEPT path, around lines 421-433):

```rust
Equation::Arrayed(formatted_dims.clone(), elements, default_equation, eq_has_except)
```

For non-EXCEPT Arrayed equations (the apply-to-all override case), set to `false`.

2. In `variable.rs`, replace the `should_apply_default_to_missing` function body. Instead of the text-comparison heuristic, simply check the `has_except_default` field:

```rust
fn should_apply_default_to_missing(
    _dimension_names: &[DimensionName],
    _dimensions: &[datamodel::Dimension],
    _elements: &[(String, String, Option<String>, Option<datamodel::GraphicalFunction>)],
    _default_eq: &Option<String>,
    has_except_default: bool,
) -> bool {
    has_except_default
}
```

Or even simpler: remove the function entirely and use the bool directly at its call site. The `parse_equation` function at the call site (search for `should_apply_default_to_missing` in variable.rs) can just use the `has_except_default` value.

**Testing:**

- close-array-gaps.AC3.1: Existing `simulates_except_basic_mdl` test passes (it tests basic EXCEPT via MDL inline model)
- close-array-gaps.AC3.4: The text-comparison heuristic is no longer called

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All existing tests pass including `simulates_except_basic_mdl`.

**Commit:** `engine: replace EXCEPT text-comparison heuristic with has_except_default flag`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add proto field for has_except_default and update serialization

**Verifies:** close-array-gaps.AC3.3

**Files:**
- Modify: `src/simlin-engine/src/project_io.proto:44-54` (ArrayedEquation message)
- Modify: `src/simlin-engine/src/serde.rs:295,337-352` (proto serialize/deserialize)
- Modify: `src/simlin-engine/src/json.rs` (JSON serialize/deserialize)

**Implementation:**

1. Add to `project_io.proto` ArrayedEquation message:
```protobuf
optional bool has_except_default = 3;
```

2. Run `pnpm build:gen-protobufs` to regenerate Rust protobuf bindings.

3. In `serde.rs`, update the `Equation::Arrayed` serialization path (around line 295) to include `has_except_default` in the proto output. Update deserialization (around lines 337-352) to read the field, defaulting to `false` when absent (backward compatibility).

4. In `json.rs`, update the `ArrayedEquation` struct to include `has_except_default: bool` with `#[serde(default)]` for backward compatibility. Update both the serialization and deserialization paths.

Note: The `validate_equation_for_protobuf` guard at serde.rs:2238-2246 remains for now (it guards `default_equation`, not `has_except_default`). That guard is removed in Phase 4.

Note: This phase adds `has_except_default` as proto field 3 on `ArrayedEquation`. Phase 4 will add `default_equation` as field 4. The field numbering is sequential and depends on Phase 2 being completed first.

**Testing:**

- close-array-gaps.AC3.3: Add a unit test that constructs an `Equation::Arrayed` with `has_except_default: true`, serializes to proto, deserializes, and verifies the flag is preserved
- Backward compat: Deserialize a proto without the field, verify it defaults to `false`

**Verification:**

```bash
pnpm build:gen-protobufs
cargo test -p simlin-engine
```

Expected: Proto regeneration succeeds, all tests pass, new round-trip test passes.

**Commit:** `engine: add has_except_default to proto schema and serialization`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
<!-- START_TASK_4 -->
### Task 4: Extend find_matching_dimension for cross-dimension mapping

**Verifies:** close-array-gaps.AC4.4

**Files:**
- Modify: `src/simlin-engine/src/ast/expr2.rs:491-515` (find_matching_dimension)

**Implementation:**

After the name-match and indexed-size-match attempts, add a third fallback that checks dimension mappings. The `Expr2Context` trait (or a new parameter) needs to provide access to `DimensionsContext::has_mapping_to`.

Check if the current `Expr2Context` trait already exposes dimension mapping queries. If not, add a method to the trait:

```rust
fn has_mapping_to(&self, dim_name: &str, target: &str) -> bool;
```

Then in `find_matching_dimension`, add a third attempt after the indexed-dimension fallback:

```rust
// Third: try dimension mapping match
for (sec_name, &sec_size) in secondary_names.iter().zip(secondary_dims.iter()) {
    if ctx.has_mapping_to(sec_name, name) || ctx.has_mapping_to(name, sec_name) {
        return Some((sec_name.as_str(), sec_size));
    }
}
```

This allows dimension unification to accept mapped dimensions (e.g., DimD maps to DimA) without a `MismatchedDimensions` error.

Implement the trait method on all `Expr2Context` implementors (look for `impl Expr2Context for ...` in the codebase) by delegating to `DimensionsContext::has_mapping_to`.

**Testing:**

Tests in Task 6 via integration tests.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles without errors.

**Commit:** `engine: extend find_matching_dimension to check dimension mappings`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Fix cross-dimension mapping resolution in MDL convert

**Verifies:** close-array-gaps.AC4.1, close-array-gaps.AC4.2, close-array-gaps.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs:601+` (build_element_context or related)

**Implementation:**

The problem: when iterating `k[DimA]` at element A2 and encountering `j[DimD]` in the equation body, the converter needs to substitute `DimD` with the element of DimD that maps to A2 (e.g., D1 if DimD maps D1->A2).

In `build_element_context` (line 601), extend the substitution mapping to handle cross-dimension references:

1. For each LHS dimension subscript, identify mapped dimensions from `DimensionsContext`
2. When a dimension in the RHS equation maps to the iterating LHS dimension, substitute the corresponding mapped element using `DimensionsContext::translate_via_mapping`

The specific fix needs to handle the `except.mdl` model's pattern:
- Variable `k` is dimensioned by `DimA`
- EXCEPT equation body references `j[DimD]`
- DimD maps to DimA via element-level correspondences (D1->A2, D2->A3)
- When iterating at DimA=A2, `j[DimD]` should resolve to `j[D1]`

The `translate_via_mapping` method in `DimensionsContext` (line 464) already handles this translation bidirectionally. Thread the `DimensionsContext` (or equivalent) into `build_element_context` so it can perform cross-dimension substitution.

**Testing:**

Tests in Task 6 via integration tests.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles without errors.

**Commit:** `engine: fix cross-dimension mapping resolution in EXCEPT equations`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Un-ignore EXCEPT integration tests

**Verifies:** close-array-gaps.AC4.1, close-array-gaps.AC4.2, close-array-gaps.AC4.5

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:609-650` (EXCEPT test ignore annotations)

**Implementation:**

1. Remove `#[ignore]` from `simulates_except` (line 620) and `simulates_except2` (line 627).

2. Keep `#[ignore]` on `simulates_except_xmile` (line 609) and `simulates_except_xmile_interpreter_only` (line 645) -- these are blocked by xmutil C++ issues (out of scope per AC4.5).

3. The `simulates_except` test calls `simulate_mdl_path` which runs both interpreter and VM paths against the reference `.dat` output. The `simulates_except2` test follows the same pattern.

**Testing:**

- close-array-gaps.AC4.1: `simulates_except` passes (both interpreter and VM)
- close-array-gaps.AC4.2: `simulates_except2` passes (both interpreter and VM)
- close-array-gaps.AC4.5: xmutil-blocked tests remain ignored

**Verification:**

```bash
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_except
```

Expected: `simulates_except` and `simulates_except2` pass. The two XMILE tests remain ignored.

**Commit:** `engine: un-ignore simulates_except and simulates_except2 integration tests`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->
