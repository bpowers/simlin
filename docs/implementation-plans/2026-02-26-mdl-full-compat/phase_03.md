# MDL Full Compatibility -- Phase 3: EXCEPT Semantics

**Goal:** Full `:EXCEPT:` support from MDL parsing through simulation, enabling the except and except2 test models.

**Architecture:** Three-layer implementation: (1) MDL converter reads `lhs.except` to produce `Equation::Arrayed` with `default_equation` and only override entries in the element list, (2) `variable.rs` expands the default equation to fill in all missing elements when building `Ast::Arrayed`, producing a dense HashMap, (3) the compiler processes the fully-expanded `Ast::Arrayed` with no changes needed. The parser already fully parses `:EXCEPT:` syntax into `ExceptList` on the `Lhs` struct.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 3 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC4: EXCEPT support
- **mdl-full-compat.AC4.3 Success:** EXCEPT equations compile and simulate correctly (except/except2 test models pass)

---

## Reference Files

- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/ast.rs` -- `Lhs` struct (line 283), `ExceptList` (line 273)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/parser.rs` -- `parse_except_list()` (line 867), parser test (line 2107)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/convert/variables.rs` -- `build_variable_with_elements()` (line 250), `build_equation()` (line 683)
- `/home/bpowers/src/simlin/src/simlin-engine/src/variable.rs` -- `Equation::Arrayed` to `Ast::Arrayed` (line 391)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/mod.rs` -- `Ast::Arrayed` iteration (lines 555, 657)
- `/home/bpowers/src/simlin/test/sdeverywhere/models/except/except.mdl` -- test model
- `/home/bpowers/src/simlin/test/sdeverywhere/models/except2/except2.mdl` -- test model
- `/home/bpowers/src/simlin/src/simlin-engine/tests/simulate.rs` -- `simulates_except()` (line 458, `#[ignore]`)

Key existing infrastructure:
- `Lhs.except: Option<ExceptList>` is already parsed by the MDL parser
- `ExceptList.subscripts: Vec<Vec<Subscript>>` holds the except bracket groups
- The converter currently ignores `lhs.except` entirely (line 275-350)
- `build_variable_with_elements()` iterates multiple equations, later overriding earlier (line 364-366)

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: MDL Converter EXCEPT Handling

<!-- START_TASK_1 -->
### Task 1: Filter excepted elements in MDL converter

**Verifies:** mdl-full-compat.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (inside `build_variable_with_elements()`, around line 374-408)

**Implementation:**

In `build_variable_with_elements()`, the loop at line 374 iterates over `expanded_eqs` and inserts each equation's element keys into `element_map`. When the equation's LHS has an `except` field, the excepted element keys must be EXCLUDED from the set of keys that get this equation.

The `ExpandedEquation` struct (or its source) carries the `lhs` which has the `except` field. The fix:

1. After computing `element_keys` for each expanded equation (from the Cartesian product of LHS subscripts), check if `lhs.except` is `Some`.

2. If EXCEPT is present, compute the set of excepted keys:
   - For each bracket group in `except.subscripts`, expand the subscript elements (using the same dimension expansion as LHS subscripts)
   - Compute the Cartesian product of except subscripts to get except keys
   - Remove these keys from the equation's `element_keys`

3. When EXCEPT is present, also track the equation string as the "default equation" for the resulting `Equation::Arrayed`. Store it alongside the override elements.

The element key computation pattern already exists in `expand_lhs_subscripts()` -- reuse that logic for the except subscripts.

After filtering, the `element_map` will contain:
- Override entries: elements with specific equations (from non-EXCEPT equations)
- Default entries: elements from EXCEPT equations (all elements MINUS excepted ones)

When building the final `Equation::Arrayed`, if any equation had EXCEPT syntax, set the `default_equation` field (third field from Phase 2) to the EXCEPT equation text. The element list should contain ONLY the override entries (elements that are excepted from the default and have their own specific equations).

**Testing:**

Add a unit test in the `#[cfg(test)]` module of `variables.rs` that converts an MDL snippet with `:EXCEPT:` syntax:
```
g[DimA] :EXCEPT: [A1] = 7 ~~|
g[A1] = 10 ~
```
Verify the resulting `Equation::Arrayed` has:
- `default_equation = Some("7")`
- Elements list contains only `[("A1", "10", None, None)]`
- Dimension names include `["DimA"]`

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass including new EXCEPT test

**Commit:** `engine: read lhs.except in MDL converter to filter excepted elements`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Track default equation through converter pipeline

**Verifies:** mdl-full-compat.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (around line 420-430, `Equation::Arrayed` construction)

**Implementation:**

The `build_variable_with_elements()` function currently constructs the equation at line 430:

```rust
let equation = Equation::Arrayed(formatted_dims.clone(), elements);
```

After Phase 2, this becomes:
```rust
let equation = Equation::Arrayed(formatted_dims.clone(), elements, None);
```

For EXCEPT support, when any of the equations for this variable had `:EXCEPT:` syntax, pass the default equation text:

```rust
let equation = Equation::Arrayed(formatted_dims.clone(), elements, default_equation);
```

The `default_equation` variable needs to be tracked through the equation expansion loop. Add a `let mut default_equation: Option<String> = None;` before the loop at line 374, and set it when processing an equation with `lhs.except`.

When `default_equation` is Some, the element list should contain ONLY the override entries. If there's no override for an element (it just uses the default), it should NOT appear in the elements list. The `default_equation` text represents what those elements compute.

**Testing:**

Extend the unit test from Task 1 to also verify that the constructed `Equation::Arrayed` has the correct third field (default_equation).

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: propagate default_equation through EXCEPT equation building`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Handle per-element substitution in default equation

**Verifies:** mdl-full-compat.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (around `build_equation_rhs_with_context`)

**Implementation:**

When an EXCEPT equation references dimension names (e.g., `g[DimA] :EXCEPT: [A1] = a[DimA] + 1`), the default equation text needs per-element dimension substitution. For element A2, `a[DimA]` should become `a[A2]`; for A3, it becomes `a[A3]`.

The existing `build_element_context()` and `format_expr_with_context()` infrastructure already handles this for explicit per-element equations. For EXCEPT default equations, when building the element_map entries for non-excepted elements, each element should get the default equation formatted with its specific element context (via `build_equation_rhs_with_context()`).

This is already partially handled by the code at line 386-405 where `build_element_context` creates per-element substitutions and `build_equation_rhs_with_context` applies them. The key change is ensuring that when processing the EXCEPT equation, the non-excepted elements go through this same substitution path.

**Testing:**

Add a unit test with an EXCEPT equation that references dimension variables:
```
g[DimA] :EXCEPT: [A1] = a[DimA] + 1
g[A1] = 10
a[DimA] = 5, 6, 7
```
Verify that:
- g[A2] gets equation `a[A2]+1`
- g[A3] gets equation `a[A3]+1`
- g[A1] gets equation `10`

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: apply per-element substitution in EXCEPT default equations`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-5) -->
## Subcomponent B: Compiler-Level EXCEPT Expansion

<!-- START_TASK_4 -->
### Task 4: Expand default_equation in variable.rs when building Ast::Arrayed

**Verifies:** mdl-full-compat.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/variable.rs:391-414`

**Implementation:**

The `Equation::Arrayed` to `Ast::Arrayed` conversion at line 391 currently iterates only the explicit element list. With `default_equation` (Phase 2 third field), elements not in the explicit list need the default equation parsed and added to the HashMap.

After Phase 2, the pattern match becomes:
```rust
datamodel::Equation::Arrayed(dimension_names, elements, default_equation) => {
```

The expansion logic:
1. Parse all explicit elements as before (line 395-407)
2. If `default_equation` is Some:
   a. Get all dimension elements from `dimensions` (the `Vec<Dimension>` parameter)
   b. Compute the full Cartesian product of all element combinations
   c. For each combination not already in the explicit elements HashMap, parse the default equation and insert it
3. The resulting HashMap should contain entries for ALL element combinations

```rust
if let Some(ref default_eq) = default_equation {
    let (default_ast, default_errors) = parse_inner(default_eq);
    errors.extend(default_errors);
    if let Some(default_ast) = default_ast {
        // Fill in missing elements with the default
        for subscripts in SubscriptIterator::new(&dims) {
            let key = CanonicalElementName::from_raw(&subscripts.join(","));
            elements.entry(key).or_insert_with(|| default_ast.clone());
        }
    }
}
```

Note: The converter (Tasks 1-3) stores per-element-substituted equation strings in the elements list. The `default_equation` field holds the raw equation text (before substitution). Since `variable.rs` receives the `Equation::Arrayed` with the raw default, it needs to either: (a) parse the default once and clone it for each missing element (works when the default has no dimension references, e.g., a constant like `5`), or (b) for defaults that reference dimension names, store each non-excepted element's substituted equation in the elements list during conversion (Task 3 handles this). In approach (b), `default_equation` is only used as documentation/metadata and `variable.rs` does not need to expand it. The converter in Task 3 should produce dense element entries so `variable.rs` just parses them normally.

**Testing:**

Add a unit test using `TestProject` that creates an `Equation::Arrayed` with `default_equation = Some("5")` and only one override element. Verify that `Ast::Arrayed` contains entries for ALL dimension elements.

**Verification:**
Run: `cargo test -p simlin-engine variable`
Expected: All tests pass

**Commit:** `engine: expand default_equation to fill all elements in Ast::Arrayed`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Enable and run except test models

**Verifies:** mdl-full-compat.AC4.3

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (lines 458, 478, and lines 541-543)

**Implementation:**

Remove the `#[ignore]` annotations from the except test functions:
- `simulates_except()` at line 458
- `simulates_except_interpreter_only()` at line 478

Uncomment the except models from `TEST_SDEVERYWHERE_MODELS` at lines 541-543:
```rust
// Before (commented out):
// "except/except.xmile", // MismatchedDimensions
// "except2/except2.xmile", // MismatchedDimensions

// After:
"except/except.xmile",
"except2/except2.xmile",
```

Run the tests. If the models also require dimension mapping support (the comments mention "subscript mappings"), the MDL-path tests should still work since the native converter handles mappings via the `maps_to` field. The XMILE-path tests (from xmutil) may continue to fail because xmutil drops subscript mappings.

If the XMILE-path tests fail but MDL-path tests pass, keep the XMILE-path tests `#[ignore]` with an updated comment explaining they need xmutil EXCEPT support.

**Testing:**

The except models themselves are the tests. They contain `.dat` expected output files with per-element values validated against Vensim's simulation.

**Verification:**
Run: `cargo test --features file_io --test simulate -- except`
Expected: except and except2 tests pass

Run: `cargo test --features file_io --test simulate`
Expected: All simulation tests pass (no regressions)

**Commit:** `engine: enable except and except2 test models`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_6 -->
### Task 6: Final verification

**Verifies:** mdl-full-compat.AC4.3

**Files:** None (verification only)

**Implementation:**

Run the full test suite:

```bash
cargo test -p simlin-engine
cargo test --features file_io --test simulate
cargo test -p simlin-engine --test mdl_roundtrip
```

Expected: All tests pass. The except/except2 models simulate correctly through the MDL path.

No commit (verification only).
<!-- END_TASK_6 -->
