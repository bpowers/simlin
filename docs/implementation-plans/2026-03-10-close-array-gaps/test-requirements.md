# Close Array Gaps -- Test Requirements

This document maps every acceptance criterion from the close-array-gaps design to either an automated test or a documented human verification step. Each sub-criterion appears in exactly one table.

## Automated Tests

| AC | Criterion | Test Type | Test File | Phase |
|---|---|---|---|---|
| close-array-gaps.AC1.1 | Reducer builtins (SUM, MEAN, MIN, MAX, SIZE, STDDEV) preserve Wildcard/SparseRange/Range ops but do NOT promote ActiveDimRef to Wildcard | unit | `src/simlin-engine/src/array_tests.rs` | 1 |
| close-array-gaps.AC1.2 | Vector builtins (VectorElmMap, VectorSortOrder, VectorSelect, AllocateAvailable) promote ActiveDimRef to Wildcard but do NOT preserve reducer-style ops | unit | `src/simlin-engine/src/array_tests.rs` | 1 |
| close-array-gaps.AC1.3 | Nested `SUM(VECTOR SORT ORDER(...))` produces correct results (not corrupted by conflated flag) | integration | `src/simlin-engine/tests/compiler_vector.rs` | 1 |
| close-array-gaps.AC2.1 | MEAN, MIN, MAX, STDDEV return NaN for zero-size views in both VM and interpreter | unit | `src/simlin-engine/src/array_tests.rs` | 1 |
| close-array-gaps.AC2.2 | SUM returns 0.0 for zero-size views in both VM and interpreter | unit | `src/simlin-engine/src/array_tests.rs` | 1 |
| close-array-gaps.AC2.3 | SIZE returns 0 for zero-size views in both VM and interpreter | unit | `src/simlin-engine/src/array_tests.rs` | 1 |
| close-array-gaps.AC3.1 | `has_except_default: true` is set on EXCEPT-derived equations during MDL conversion | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC3.2 | `has_except_default: false` is default for non-EXCEPT equations and XMILE-parsed equations | unit | `src/simlin-engine/src/serde.rs` | 2 |
| close-array-gaps.AC3.3 | Proto round-trip preserves `has_except_default` flag | unit | `src/simlin-engine/src/serde.rs` | 2 |
| close-array-gaps.AC3.4 | Text-comparison heuristic in `should_apply_default_to_missing` is replaced by boolean check | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC4.1 | `simulates_except` integration test passes (except.mdl with DimD->DimA mapping) | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC4.2 | `simulates_except2` integration test passes | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC4.3 | Per-element equations correctly resolve cross-dimension references (e.g., `j[DimD]` becomes `j[D1]` when iterating at DimA=A2) | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC4.4 | Expr2 dimension unification accepts mapped dimensions without MismatchedDimensions error | integration | `src/simlin-engine/tests/simulate.rs` | 2 |
| close-array-gaps.AC5.1 | `b[B1]` as source in DimA context treats as full b array (Vensim semantics) | integration | `src/simlin-engine/tests/simulate.rs` | 3 |
| close-array-gaps.AC5.2 | `d[DimA,B1]` partial collapse with DimB broadcast works correctly | integration | `src/simlin-engine/tests/simulate.rs` | 3 |
| close-array-gaps.AC5.3 | `x[three]` scalar source with cross-dimension offset works | integration | `src/simlin-engine/tests/simulate.rs` | 3 |
| close-array-gaps.AC5.4 | `simulates_vector_xmile` integration test passes | integration | `src/simlin-engine/tests/simulate.rs` | 3 |
| close-array-gaps.AC5.5 | `simulates_vector_simple_mdl` runs full (VM + interpreter), not interpreter-only | integration | `src/simlin-engine/tests/simulate.rs` | 3 |
| close-array-gaps.AC6.1 | Proto round-trips EXCEPT equations with default_equation | unit | `src/simlin-engine/src/serde.rs` | 4 |
| close-array-gaps.AC6.2 | Proto round-trips element-level dimension mappings | unit | `src/simlin-engine/src/serde.rs` | 4 |
| close-array-gaps.AC6.3 | Proto round-trips DataSource metadata | unit | `src/simlin-engine/src/serde.rs` | 4 |
| close-array-gaps.AC6.4 | XMILE round-trips EXCEPT via top-level `<eqn>` + `<element>` overrides | unit | `src/simlin-engine/src/xmile/` (new test in variables.rs or mod.rs) | 4 |
| close-array-gaps.AC6.5 | XMILE round-trips element-level mappings via `<simlin:mapping>` vendor extension | unit | `src/simlin-engine/src/xmile/` (new test in dimensions.rs or mod.rs) | 4 |
| close-array-gaps.AC6.6 | XMILE round-trips DataSource via `<simlin:data_source>` vendor extension | unit | `src/simlin-engine/src/xmile/` (new test in variables.rs or mod.rs) | 4 |
| close-array-gaps.AC6.7 | Old protos without new fields deserialize correctly | unit | `src/simlin-engine/src/serde.rs` | 4 |
| close-array-gaps.AC7.1 | `simulates_directsubs_mdl` passes (GET DIRECT SUBSCRIPT in dimension definitions) | integration | `src/simlin-engine/tests/simulate.rs` | 5 |
| close-array-gaps.AC7.2 | `simulates_directconst_mdl` passes (arrayed GET DIRECT CONSTANTS with star pattern + 2D grid) | integration | `src/simlin-engine/tests/simulate.rs` | 5 |
| close-array-gaps.AC7.3 | `simulates_directlookups_mdl` passes (arrayed GET DIRECT LOOKUPS) | integration | `src/simlin-engine/tests/simulate.rs` | 5 |
| close-array-gaps.AC8.1 | MDL parser recognizes `VECTOR RANK` and maps to `Rank` builtin | unit | `src/simlin-engine/src/array_tests.rs` (or mdl parser test module) | 5 |
| close-array-gaps.AC8.2 | `RANK(A, N)` returns value at 1-based position N of sorted array in both VM and interpreter | unit | `src/simlin-engine/src/array_tests.rs` | 5 |
| close-array-gaps.AC8.3 | `RANK(A, N, B)` with tie-break array works correctly | unit | `src/simlin-engine/src/array_tests.rs` | 5 |
| close-array-gaps.AC8.4 | Unit tests cover 1-arg, 2-arg, and 3-arg forms | unit | `src/simlin-engine/src/array_tests.rs` | 5 |
| close-array-gaps.AC9.1 | `range_basic` passes with NaN fill for out-of-bounds elements | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.2 | `range_with_expressions` passes with dynamic range bounds in A2A context | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.3 | `out_of_bounds_iteration_returns_nan` passes | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.4 | `bounds_check_in_fast_path` passes | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.5 | `transpose_and_slice` passes | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.6 | `complex_expression` compiles (parser accepts `Dimension.*` subscript syntax) | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC9.7 | `star_to_indexed_subdimension` passes (datamodel has parent pointer for indexed subdimensions) | unit | `src/simlin-engine/src/array_tests.rs` | 6 |
| close-array-gaps.AC10.2 | #344 existing rejection tests pass, JSON multi-target works | unit | `src/simlin-engine/src/json.rs` | 6 |

## Human Verification

| AC | Criterion | Justification | Verification Approach |
|---|---|---|---|
| close-array-gaps.AC4.5 | xmutil-blocked XMILE tests remain ignored (out of scope) | This criterion asserts that tests remain *ignored*, which is a structural property of the test source code, not a behavioral property testable at runtime. An automated test that checks for `#[ignore]` annotations would be brittle and test the test infrastructure rather than the product. | Verify during code review that `simulates_except_xmile` and `simulates_except_xmile_interpreter_only` in `src/simlin-engine/tests/simulate.rs` retain their `#[ignore]` annotations. Confirm that no Phase 2 commit removes these annotations. |
| close-array-gaps.AC7.4 | `simulates_directdata_mdl` remains ignored (ext_data feature, out of scope) | Same rationale as AC4.5: asserts a test *stays ignored* due to an out-of-scope dependency (Excel file support via ext_data feature). Testing that a test is ignored is a code review concern, not a runtime behavior. | Verify during code review that `simulates_directdata_mdl` in `src/simlin-engine/tests/simulate.rs` retains its `#[ignore]` and `#[cfg(feature = "ext_data")]` annotations. Confirm that no Phase 5 commit removes these annotations. |
| close-array-gaps.AC10.1 | #351 has "why" comments documenting 0-based/1-based asymmetry | This criterion requires the presence and accuracy of source code comments. Comment quality and correctness cannot be meaningfully tested at runtime; it is inherently a review-time concern. | Verify during code review that `vm.rs` contains "why" comments at the VectorElmMap opcode handler (explaining 0-based offset indexing and Vensim VECTOR ELM MAP semantics) and at the VectorSortOrder opcode handler (explaining 1-based rank indices and the intentional asymmetry with VectorElmMap). Confirm comments are factually accurate per Vensim documentation. |
| close-array-gaps.AC10.3 | `#[allow(dead_code)]` count reduced for array-related scaffolding | The criterion requires a *reduction* relative to the prior count (58, measured 2026-02-15), which is a relative assertion about codebase hygiene. An automated test would need to hardcode a threshold that becomes stale as soon as unrelated code changes land. | Run `rg '#\[allow\(dead_code\)\]' --type rust src/simlin-engine/src/ -c` after Phase 6 Task 7 and verify the count is strictly less than 58. Confirm that `bytecode.rs` no longer has the stale "Array opcodes not yet emitted" dead_code annotation. Document the new count in the PR description. |
| close-array-gaps.AC10.4 | `docs/tech-debt.md` items 12 and 13 updated with new counts | This criterion requires that a documentation file contains updated numeric counts. Testing documentation content at runtime is fragile and tests the wrong thing -- the content is the output of a measurement, not a product behavior. | Verify during code review that `docs/tech-debt.md` item 12 (dead_code suppressions) reflects the post-cleanup count with the current date, and item 13 (ignored tests) reflects the reduced count after un-ignoring 7 array_tests plus integration tests from Phases 2, 3, and 5. Confirm counts match actual `rg` output. |

## Rationale

### Test type decisions

**Unit tests** (`array_tests.rs`, `serde.rs`, `json.rs`, `xmile/`) are used when the criterion targets a specific function's behavior in isolation -- flag propagation, serialization round-trips, empty-view edge cases, builtin evaluation. These tests use `TestProject` builders or direct serialization calls without requiring full model files or `.dat` reference outputs.

**Integration tests** (`tests/simulate.rs`, `tests/compiler_vector.rs`) are used when the criterion requires end-to-end simulation correctness against reference `.dat` output files from Vensim. These tests exercise the full pipeline: MDL/XMILE parse, AST lowering, compilation, and execution in both interpreter and VM. The `simulate_mdl_path` helper validates both execution backends against the reference data.

**NaN-returning tests** (AC2.1, AC9.1, AC9.2, AC9.3) require special handling: the standard `assert_interpreter_result` / `assert_vm_result_incremental` helpers use epsilon comparison (`(a - b).abs() < epsilon`) which always fails for NaN because `NaN != NaN`. These tests must use the raw result methods (`interpreter_result` / `vm_result_incremental`) and check NaN positions with `f64::is_nan()`.

### Human verification decisions

Five criteria (AC4.5, AC7.4, AC10.1, AC10.3, AC10.4) require human verification. These share a common characteristic: they assert properties of the source code itself (annotation presence, comment quality, documentation content, suppression counts) rather than runtime behavior. Automated tests for these properties would be brittle meta-tests that add maintenance burden without testing product correctness.

### Cross-referencing implementation decisions

- **AC1.1/AC1.2** rely on Phase 1's flag split introducing `promote_active_dim_ref`. The unit tests in `array_tests.rs` (Phase 1 Task 3) exercise both paths with `TestProject` models that create ActiveDimRef scenarios.
- **AC2.1/AC2.2/AC2.3** require constructing zero-size array views. Phase 1 Task 6 notes the implementation should prefer subdimension-based zero-element ranges at the model level, falling back to direct VM/interpreter-level view construction if zero-element subranges are unsupported.
- **AC3.1 through AC3.4** form a chain: MDL conversion sets the flag (AC3.1), all other paths default false (AC3.2), proto round-trips it (AC3.3), and the old heuristic is removed (AC3.4). AC3.4 is tested indirectly through AC3.1's integration test -- if the heuristic were still in use but broken, `simulates_except_basic_mdl` would fail.
- **AC4.3/AC4.4** are verified indirectly by the `simulates_except` integration test (AC4.1): the test model's `except.mdl` contains `j[DimD]` cross-dimension references that exercise both dimension unification (AC4.4) and per-element resolution (AC4.3). If either failed, the simulation output would not match the reference `.dat`.
- **AC5.1/AC5.2/AC5.3** are verified through the `simulates_vector_simple_mdl` (AC5.5) and `simulates_vector_xmile` (AC5.4) integration tests. Each sub-case corresponds to a specific equation in the test models: `c[DimA] = VECTOR ELM MAP(b[B1], a[DimA])` (AC5.1), `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` (AC5.2), `y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1))` (AC5.3).
- **AC6.1 through AC6.7** convert four existing rejection tests in `serde.rs` into round-trip success tests (Phase 4 Task 4). The XMILE tests (AC6.4/AC6.5/AC6.6) are new tests added alongside the vendor extension implementation.
- **AC8.4** is a coverage criterion satisfied by the combination of tests from AC8.2 (2-arg), AC8.3 (3-arg), and an additional arrayed test where position N comes from the iterating dimension (1-arg form, i.e., `RANK(A, Dim)`).
- **AC10.2** is verified by existing passing tests in `json.rs` (`json_preserves_multi_target_positional_mappings`, `mappings_takes_precedence_over_maps_to`). No new tests are needed; Phase 6 Task 6 confirms these tests pass.
