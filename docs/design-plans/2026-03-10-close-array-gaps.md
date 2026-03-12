# Close Array Gaps Design

## Summary

This design plan closes 12 open GitHub issues and enables 7 ignored unit tests related to array (subscript/dimension) handling in the Simlin simulation engine. Arrays in system dynamics allow a single variable to represent a vector or matrix of values indexed by named dimensions -- for example, `population[age_group, region]`. Simlin's engine can already handle basic arrayed variables, but several advanced features are incomplete: EXCEPT equations (where most dimension elements share a default formula but specific elements override it), cross-dimension mappings (where dimensions with different names are related through element-level correspondences), VECTOR ELM MAP and VECTOR SORT ORDER with cross-dimension sources, the RANK builtin, external data loading via GET DIRECT for arrayed constants/lookups/subscripts, and round-trip serialization of these constructs through both protobuf and XMILE formats.

The work is organized into 6 dependency-ordered phases. Phase 1 fixes two standalone architectural issues in the compiler's flag propagation and the VM's array reducer operations. Phase 2 builds EXCEPT infrastructure by adding an explicit boolean flag to replace a fragile text-matching heuristic, then fixes cross-dimension mapping resolution. Phase 3 extends VECTOR ELM MAP to handle source arrays from unrelated dimensions. Phase 4 adds protobuf and XMILE serialization for the constructs built in earlier phases. Phase 5 wires arrayed GET DIRECT resolution into the MDL import pipeline and implements the RANK builtin end-to-end (parser, compiler, VM, and interpreter). Phase 6 enables the 7 previously-ignored unit tests, verifies two already-resolved issues, and cleans up dead code. Each phase produces passing tests before the next begins, and the ordering ensures that foundational fixes (flag split, EXCEPT infrastructure) land before the features that depend on them.

## Definition of Done

A phased implementation plan that closes all 12 open array-related GitHub issues (#341, #342, #344, #345, #348, #351, #356, #357, #358, #359, #365, #388), enables the 7 ignored tests in `array_tests.rs`, and reduces tech debt items 12 and 13 as features land.

Each phase produces passing tests -- simulation integration tests for the issue-tracked gaps, unit tests for the `array_tests.rs` features. The plan orders work to maximize unblocked tests per phase and respects the dependency graph (#365 first, #356 before #348's EXCEPT case, etc.).

Every commit or PR that resolves a GitHub issue must include a `Fixes #N` line in the commit message body so the issue is auto-closed on merge.

**Key exclusions:** xmutil C++ fixes (2 of 4 #345 tests stay ignored), Excel/ext_data support for the directdata test model, any UI/diagram changes.

## Acceptance Criteria

### close-array-gaps.AC1: preserve_wildcards flag split (#365)
- **close-array-gaps.AC1.1 Success:** Reducer builtins (SUM, MEAN, MIN, MAX, SIZE, STDDEV) preserve Wildcard/SparseRange/Range ops but do NOT promote ActiveDimRef to Wildcard
- **close-array-gaps.AC1.2 Success:** Vector builtins (VectorElmMap, VectorSortOrder, VectorSelect, AllocateAvailable) promote ActiveDimRef to Wildcard but do NOT preserve reducer-style ops
- **close-array-gaps.AC1.3 Regression:** Nested `SUM(VECTOR SORT ORDER(...))` produces correct results (not corrupted by conflated flag)

### close-array-gaps.AC2: Empty-view array reducer guards (#388)
- **close-array-gaps.AC2.1 Success:** MEAN, MIN, MAX, STDDEV return NaN for zero-size views in both VM and interpreter
- **close-array-gaps.AC2.2 Success:** SUM returns 0.0 for zero-size views in both VM and interpreter
- **close-array-gaps.AC2.3 Success:** SIZE returns 0 for zero-size views in both VM and interpreter

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

### close-array-gaps.AC5: VECTOR ELM MAP cross-dimension (#357, #358)
- **close-array-gaps.AC5.1 Success:** `b[B1]` as source in DimA context treats as full b array (Vensim semantics)
- **close-array-gaps.AC5.2 Success:** `d[DimA,B1]` partial collapse with DimB broadcast works correctly
- **close-array-gaps.AC5.3 Success:** `x[three]` scalar source with cross-dimension offset works
- **close-array-gaps.AC5.4 Success:** `simulates_vector_xmile` integration test passes
- **close-array-gaps.AC5.5 Success:** `simulates_vector_simple_mdl` runs full (VM + interpreter), not interpreter-only

### close-array-gaps.AC6: Proto + XMILE serialization (#341, #342)
- **close-array-gaps.AC6.1 Success:** Proto round-trips EXCEPT equations with default_equation
- **close-array-gaps.AC6.2 Success:** Proto round-trips element-level dimension mappings
- **close-array-gaps.AC6.3 Success:** Proto round-trips DataSource metadata
- **close-array-gaps.AC6.4 Success:** XMILE round-trips EXCEPT via top-level `<eqn>` + `<element>` overrides
- **close-array-gaps.AC6.5 Success:** XMILE round-trips element-level mappings via `<simlin:mapping>` vendor extension
- **close-array-gaps.AC6.6 Success:** XMILE round-trips DataSource via `<simlin:data_source>` vendor extension
- **close-array-gaps.AC6.7 Backward compat:** Old protos without new fields deserialize correctly

### close-array-gaps.AC7: GET DIRECT wiring (#348)
- **close-array-gaps.AC7.1 Success:** `simulates_directsubs_mdl` passes (GET DIRECT SUBSCRIPT in dimension definitions)
- **close-array-gaps.AC7.2 Success:** `simulates_directconst_mdl` passes (arrayed GET DIRECT CONSTANTS with star pattern + 2D grid)
- **close-array-gaps.AC7.3 Success:** `simulates_directlookups_mdl` passes (arrayed GET DIRECT LOOKUPS)
- **close-array-gaps.AC7.4 Edge:** `simulates_directdata_mdl` remains ignored (ext_data feature, out of scope)

### close-array-gaps.AC8: RANK builtin (#359)
- **close-array-gaps.AC8.1 Success:** MDL parser recognizes `VECTOR RANK` and maps to `Rank` builtin
- **close-array-gaps.AC8.2 Success:** `RANK(A, N)` returns value at 1-based position N of sorted array in both VM and interpreter
- **close-array-gaps.AC8.3 Success:** `RANK(A, N, B)` with tie-break array works correctly
- **close-array-gaps.AC8.4 Success:** Unit tests cover 1-arg, 2-arg, and 3-arg forms

### close-array-gaps.AC9: Ignored array_tests.rs tests enabled
- **close-array-gaps.AC9.1 Success:** `range_basic` passes with NaN fill for out-of-bounds elements
- **close-array-gaps.AC9.2 Success:** `range_with_expressions` passes with dynamic range bounds in A2A context
- **close-array-gaps.AC9.3 Success:** `out_of_bounds_iteration_returns_nan` passes
- **close-array-gaps.AC9.4 Success:** `bounds_check_in_fast_path` passes
- **close-array-gaps.AC9.5 Success:** `transpose_and_slice` passes
- **close-array-gaps.AC9.6 Success:** `complex_expression` compiles (parser accepts `Dimension.*` subscript syntax)
- **close-array-gaps.AC9.7 Success:** `star_to_indexed_subdimension` passes (datamodel has parent pointer for indexed subdimensions)

### close-array-gaps.AC10: Verify and cleanup (#351, #344, tech debt)
- **close-array-gaps.AC10.1 Success:** #351 has "why" comments documenting 0-based/1-based asymmetry
- **close-array-gaps.AC10.2 Success:** #344 existing rejection tests pass, JSON multi-target works
- **close-array-gaps.AC10.3 Success:** `#[allow(dead_code)]` count reduced for array-related scaffolding
- **close-array-gaps.AC10.4 Success:** `docs/tech-debt.md` items 12 and 13 updated with new counts

## Glossary

- **A2A (Array-to-Array) assignment**: The compiler's mechanism for expanding an arrayed equation into per-element bytecode. When an equation's output is arrayed, the compiler iterates over the target dimension's elements and emits code for each one, resolving dimension references to concrete element indices at each iteration step.
- **ActiveDimRef**: An `IndexOp` variant representing a reference to the currently-iterating dimension during A2A expansion. When the compiler encounters a dimension name used as a subscript (e.g., `x[DimA]` while iterating over DimA), it produces an `ActiveDimRef` that resolves to the current element's index at code generation time.
- **Array reducer / reducer builtin**: Builtins (SUM, MEAN, MIN, MAX, SIZE, STDDEV) that collapse an array dimension into a scalar value. Distinct from "vector builtins" (VectorElmMap, VectorSortOrder, etc.) that produce array-valued results.
- **ArrayView**: A runtime representation of a slice into an arrayed variable's storage, defined by dimension offsets, strides, and element counts. The VM builds views on a stack to support array operations.
- **DataProvider**: A trait abstraction for resolving external data references at MDL import time. Implementations load data from CSV or Excel files when the model uses Vensim's GET DIRECT family of functions.
- **Dimension mapping**: A relationship between two dimensions with different names, established through element-level correspondences (e.g., DimD maps to DimA via D1->A2, D2->A3). Used in Vensim models to allow equations to reference variables subscripted by a mapped dimension.
- **EXCEPT equation**: A Vensim construct where an arrayed variable has a default equation applied to most elements of a dimension, with specific elements overriding that default. Serialized in the datamodel as `Equation::Arrayed` with a `default_equation` field.
- **Expr0 / Expr1 / Expr2 / Expr3**: The four stages of the engine's AST progressive lowering pipeline. Expr0 is the raw parse output. Expr1 has modules expanded. Expr2 has dimensions resolved. Expr3 has subscripts fully expanded into concrete element references.
- **GET DIRECT (CONSTANTS / DATA / LOOKUPS / SUBSCRIPT)**: A family of Vensim built-in functions that load data from external files (CSV or Excel) at model import time. GET DIRECT SUBSCRIPT loads dimension element names; GET DIRECT CONSTANTS loads numeric arrays; GET DIRECT LOOKUPS loads per-element graphical function tables; GET DIRECT DATA loads time-series data.
- **IndexOp**: An enum representing the possible subscript operations during array compilation: `Single` (one element), `Wildcard` (all elements), `Range` (contiguous slice), `SparseRange` (non-contiguous elements from a subdimension), `ActiveDimRef` (reference to the iterating dimension).
- **MDL**: Vensim's native model file format. The engine has a native Rust parser (`src/simlin-engine/src/mdl/`) that converts MDL models into the internal datamodel.
- **preserve_wildcards_for_iteration**: A boolean flag on the compiler's `Context` struct that controls whether `Wildcard` and `Range` index operations are preserved through expression lowering (needed for reducer builtins like SUM) or collapsed. Issue #365 splits this single flag into two, separating reducer behavior from vector-builtin behavior.
- **promote_active_dim_ref**: The new flag introduced by the #365 split. When true (inside vector builtins like VectorElmMap), `ActiveDimRef` subscripts are promoted to `Wildcard` so the full source array is available, matching Vensim's semantics for cross-dimension array sources.
- **Vendor extension (XMILE)**: Elements in the `simlin:` XML namespace used to serialize Simlin-specific constructs that have no representation in the standard XMILE specification. Examples include `<simlin:mapping>` for element-level dimension mappings and `<simlin:data_source>` for external data metadata.
- **VECTOR ELM MAP / VECTOR SORT ORDER**: Vensim array-producing builtins. VECTOR ELM MAP applies an element-wise mapping using an offset array. VECTOR SORT ORDER returns the sort-order indices of an array. Both produce array-valued results (unlike reducers, which produce scalars).
- **VM (Virtual Machine)**: The engine's stack-based bytecode VM (`vm.rs`) that executes compiled simulation models. The primary execution path for production; the AST-walking interpreter is retained as a reference implementation for correctness verification.
- **XMILE**: An XML-based interchange format for system dynamics models (IEEE standard). Simlin uses XMILE as one of its serialization formats, with vendor extensions for constructs beyond the spec.
- **xmutil**: Bob Eberlein's C++ tool for converting Vensim MDL files to XMILE. Used only in tests; the native Rust MDL parser has replaced it for production use. Some test cases are blocked by xmutil limitations in EXCEPT handling.

## Architecture

This design closes 12 open array-related issues plus 7 ignored unit tests across 6 dependency-ordered phases. The work spans the compiler (`src/simlin-engine/src/compiler/`), AST lowering (`src/simlin-engine/src/ast/`), MDL converter (`src/simlin-engine/src/mdl/convert/`), VM (`src/simlin-engine/src/vm.rs`), interpreter (`src/simlin-engine/src/interpreter.rs`), serialization (`src/simlin-engine/src/serde.rs`, `src/simlin-engine/src/xmile/`), and data provider (`src/simlin-engine/src/data_provider/`).

The dependency graph drives the phase ordering:

```
Phase 1: #365 (split flags) + #388 (NaN guard)     [standalone fixes]
    |
    v
Phase 2: #356 (has_except_default) + #345 (cross-dim EXCEPT)
    |
    v
Phase 3: #357 + #358 (VECTOR ELM MAP cross-dim)
    |
    v
Phase 4: #341 + #342 (proto + XMILE serialization)
    |
    v
Phase 5: #348 (GET DIRECT) + #359 (RANK)
    |
    v
Phase 6: Ignored tests + #351/#344 verify + cleanup
```

Phases 1-3 are strictly ordered: the flag split (#365) prevents latent bugs in subsequent compiler work; EXCEPT infrastructure (#356) must exist before cross-dimension EXCEPT resolution (#345); and VECTOR ELM MAP cross-dimension (#357) builds on the same compiler infrastructure. Phase 4 depends on Phase 2 (proto schema must include the `has_except_default` field). Phase 5's GET DIRECT (#348) depends on Phase 4 (serialization must support EXCEPT constructs that GET DIRECT produces). Phase 6 is cleanup that depends on all prior work.

## Existing Patterns

**Compiler flag propagation:** The `Context` struct in `compiler/context.rs` carries boolean flags through recursive expression lowering via `with_*` constructor methods. The `preserve_wildcards_for_iteration` flag follows this pattern; the new `promote_active_dim_ref` flag will use the same mechanism.

**Protobuf backward compatibility:** New proto fields use optional or repeated types with field numbers that don't conflict with existing ones. The JSON path (`json.rs`) already handles all three missing constructs (EXCEPT default, element-level mappings, DataSource) and serves as the reference implementation for round-trip fidelity.

**MDL conversion pipeline:** `convert_mdl_with_data()` threads a `DataProvider` through `ConversionContext` for resolving external data references at import time. The scalar GET DIRECT path already works end-to-end; the arrayed extensions follow the same pattern.

**Vendor extensions in XMILE:** The `simlin:` namespace prefix is used for constructs that don't exist in the XMILE spec (element-level dimension mappings, DataSource metadata).

**Array builtin structure:** New builtins follow the pattern of existing ones: parsed in `parser/mod.rs`, AST node in `builtins.rs`, type-checked in `units_infer.rs`, lowered through `lower_builtin_expr3` in `compiler/context.rs`, codegen in `compiler/codegen.rs`, executed in both `vm.rs` and `interpreter.rs`.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Standalone Architectural Fixes (#365, #388)

**Goal:** Split the conflated `preserve_wildcards_for_iteration` flag and add empty-view guards to all array reducers, removing latent bugs before subsequent compiler work.

**Components:**

- `Context` struct in `src/simlin-engine/src/compiler/context.rs` -- add `promote_active_dim_ref: bool` field, split `with_preserved_wildcards()` into two constructors, split `has_iteration_preserving_ops` check into separate guards for reducers vs vector builtins, update all 12 call sites in `lower_builtin_expr3`
- `src/simlin-engine/src/vm.rs` -- add zero-size view guards before division/comparison in `Opcode::ArrayMean`, `Opcode::ArrayStddev`, `Opcode::ArrayMin`, `Opcode::ArrayMax`. Return NaN for empty views. SUM returns 0.0 (additive identity). SIZE returns 0.
- `src/simlin-engine/src/interpreter.rs` -- align empty-view behavior: MEAN/MIN/MAX/STDDEV return NaN for empty views (interpreter currently returns 0.0 for MEAN/STDDEV; update to NaN)

**Dependencies:** None (first phase)

**Done when:** Regression test with nested `SUM(VECTOR SORT ORDER(...))` passes (verifies flag split). Unit tests for all 6 array reducers (SUM, SIZE, MEAN, MIN, MAX, STDDEV) with empty views pass in both VM and interpreter, returning the correct values (NaN for MEAN/MIN/MAX/STDDEV, 0.0 for SUM, 0 for SIZE).
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: EXCEPT Infrastructure + Cross-Dimension Resolution (#356, #345)

**Goal:** Replace the fragile text-matching heuristic for EXCEPT detection with an explicit flag, then fix cross-dimension mapping resolution so EXCEPT equations with mapped dimensions compile and simulate correctly.

**Components:**

- `src/simlin-engine/src/datamodel.rs` -- add `has_except_default: bool` field to `Equation::Arrayed` variant; update all pattern matches across the codebase
- `src/simlin-engine/src/variable.rs` -- replace `should_apply_default_to_missing` text-comparison logic with check of the boolean field
- `src/simlin-engine/src/mdl/convert/variables.rs` -- set `has_except_default: true` when building EXCEPT-derived equations; extend `build_element_context` to resolve cross-dimension mappings (when iterating `k[DimA]` at element A2 and encountering `j[DimD]` where DimD maps to DimA, substitute `j[D1]`)
- `src/simlin-engine/src/ast/expr2.rs` -- extend `find_matching_dimension` to accept `DimensionsContext` and check `has_mapping_to` when name-matching fails
- `src/simlin-engine/src/project_io.proto` -- add `optional bool has_except_default = 4` to `ArrayedEquation`
- `src/simlin-engine/src/xmile/variables.rs` -- default `has_except_default` to `false` on XMILE deserialization
- `src/simlin-engine/src/serde.rs`, `src/simlin-engine/src/json.rs` -- update serialization/deserialization for the new field

**Dependencies:** Phase 1 (flag split prevents latent bugs in compiler work)

**Done when:** `simulates_except` and `simulates_except2` integration tests pass. The 2 xmutil-blocked XMILE tests remain ignored. Proto round-trip test for `has_except_default` passes.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: VECTOR ELM MAP Cross-Dimension Sources (#357, #358)

**Goal:** Allow VECTOR ELM MAP (and other array-producing builtins) to accept source arrays from dimensions unrelated to the output, matching Vensim semantics.

**Components:**

- `src/simlin-engine/src/compiler/context.rs` -- in `lower_from_expr3`'s `Expr3::Subscript` arm, when `promote_active_dim_ref` is true (from Phase 1's flag split), suppress the `MismatchedDimensions` guard at line ~1375 for cross-dimension arguments. For named-element subscripts that collapse all dimensions inside an array-producing builtin context, reinterpret `IndexOp::Single` as `IndexOp::Wildcard` to preserve the full source array view (matching Vensim's semantics where `b[B1]` inside VectorElmMap means "full b array").
- `src/simlin-engine/src/compiler/mod.rs` -- fix the VM incremental path bug for `vector_simple` where `m[a3]` returns 0 instead of 2 for VECTOR SORT ORDER cross-dimension scenarios
- `src/simlin-engine/tests/simulate.rs` -- uncomment `simulates_vector_xmile` test (line ~888), upgrade `simulates_vector_simple_mdl` from interpreter-only to full (VM + interpreter)

**Dependencies:** Phase 1 (#365 flag split provides the `promote_active_dim_ref` mechanism)

**Done when:** `simulates_vector_xmile` and `simulates_vector_simple_mdl` (full, not interpreter-only) integration tests pass. All cross-dimension VECTOR ELM MAP sub-cases work: `b[B1]` as full-array source, `d[DimA,B1]` partial collapse with broadcast, `x[three]` scalar source.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Protobuf + XMILE Serialization (#341, #342)

**Goal:** Extend protobuf and XMILE serialization to support EXCEPT equations, element-level dimension mappings, and DataSource metadata, enabling full round-tripping of arrayed models.

**Components:**

- `src/simlin-engine/src/project_io.proto` -- add `optional string default_equation = 5` to `ArrayedEquation`; add `DimensionMapping` message with `string target` and `repeated ElementMapEntry entries`; add `repeated DimensionMapping mappings = 6` to `Dimension`; add `DataSource` message with `DataSourceKind` enum and string fields; add `optional DataSource data_source = 6` to `Compat`
- `src/simlin-engine/src/serde.rs` -- remove the three `validate_for_protobuf` guard functions; update serialize/deserialize for all new fields; maintain backward compat (absent fields = None/empty)
- `src/simlin-engine/src/xmile/variables.rs` -- serialize EXCEPT as unsubscripted `<eqn>` alongside `<element>` overrides; deserialize back to `Equation::Arrayed` with default_equation; add `<simlin:data_source>` vendor extension
- `src/simlin-engine/src/xmile/dimensions.rs` -- add `<simlin:mapping>` with `<simlin:elem>` children for element-level mappings
- `src/simlin-engine/src/xmile/mod.rs` -- remove the three `validate_for_xmile` guard functions; register `simlin` namespace

**Dependencies:** Phase 2 (#356 adds `has_except_default` to proto; this phase extends the same schema)

**Done when:** Proto and XMILE round-trip tests pass for: EXCEPT equations with default_equation, element-level dimension mappings, DataSource metadata. Existing rejection tests become round-trip success tests. Backward compatibility test: old protos without new fields deserialize correctly.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: External Data Wiring + RANK Builtin (#348, #359)

**Goal:** Wire arrayed GET DIRECT resolution into the MDL converter pipeline (unblocking 3 of 4 blocked test models) and implement the RANK builtin end-to-end.

**Components:**

- `src/simlin-engine/src/mdl/convert/dimensions.rs` -- detect `GET DIRECT SUBSCRIPT(...)` in subscript range bodies; thread `data_provider` into dimension-building functions; call `load_subscript()` to resolve element names
- `src/simlin-engine/src/data_provider/mod.rs` -- add `load_constants_range` (returns `Vec<f64>` for `*` patterns), `load_constants_grid` (returns `Vec<Vec<f64>>` for 2D), `load_lookups_array` (returns `Vec<Vec<(f64, f64)>>` for per-element lookups) to `DataProvider` trait
- `src/simlin-engine/src/data_provider/csv_provider.rs` -- implement the new methods on `FilesystemDataProvider`
- `src/simlin-engine/src/mdl/convert/variables.rs` -- extend `try_resolve_data_equation` to handle arrayed patterns (`*` suffix, 2D grids); produce `Equation::Arrayed` with per-element constants or graphical functions
- `src/simlin-engine/src/mdl/builtins.rs` -- add `"vector rank"` to builtin recognition
- `src/simlin-engine/src/mdl/convert/xmile_compat.rs` -- add rename from `vector_rank` to `rank`
- `src/simlin-engine/src/interpreter.rs` -- implement `RANK(A, N)` and `RANK(A, N, B)`: sort array ascending, return value at 1-based position N, optional tie-breaking with array B
- `src/simlin-engine/src/compiler/codegen.rs` -- replace `TodoArrayBuiltin` for Rank with opcode emission; add `Opcode::Rank`
- `src/simlin-engine/src/vm.rs` -- implement `Opcode::Rank` execution

**Dependencies:** Phase 4 (serialization must support the constructs GET DIRECT produces, particularly EXCEPT + DataSource)

**Done when:** `simulates_directsubs_mdl`, `simulates_directconst_mdl`, and `simulates_directlookups_mdl` integration tests pass. `simulates_directdata_mdl` remains ignored (requires ext_data feature). RANK builtin works in both interpreter and VM with unit tests covering 1-arg, 2-arg, and 3-arg forms.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Ignored Tests + Verify/Cleanup (#351, #344, array_tests.rs, tech debt)

**Goal:** Enable the 7 ignored `array_tests.rs` unit tests, verify and close the 2 resolved issues, and clean up dead code suppressions.

**Components:**

- `src/simlin-engine/src/compiler/mod.rs` -- support dimension-mismatched A2A assignment: when the source range produces fewer elements than the target dimension, emit code that iterates the target dimension with bounds checking (VM returns NaN for out-of-bounds reads). Unblocks `range_basic`, `range_with_expressions`, `out_of_bounds_iteration_returns_nan`, `bounds_check_in_fast_path`. Update `range_basic`/`range_with_expressions` expected values from last-element-repeat to NaN.
- `src/simlin-engine/src/compiler/context.rs` -- support `range_with_expressions`: extend dynamic range bounds (scalar variables as start/end) to work in A2A assignment context (already works inside reducers)
- `src/simlin-engine/src/compiler/` -- support transpose + subscript chaining (`matrix'[1:3, *]`): ensure subscripting a transposed expression operates on the transposed view. Unblocks `transpose_and_slice`.
- `src/simlin-engine/src/parser/mod.rs` or `src/simlin-engine/src/ast/` -- recognize `DimName.*` as a qualified wildcard subscript. Unblocks `complex_expression`.
- `src/simlin-engine/src/datamodel.rs` -- add `parent: Option<DimensionName>` to `Dimension` for indexed subdimension parent relationships. Update proto and XMILE accordingly. Unblocks `star_to_indexed_subdimension`.
- `src/simlin-engine/src/vm.rs`, `src/simlin-engine/src/interpreter.rs` -- add "why" comments documenting the intentional 0-based VectorElmMap / 1-based VectorSortOrder asymmetry (#351 verify)
- Verify #344: confirm existing rejection tests pass and JSON handles multi-target correctly
- `docs/tech-debt.md` -- update items 12 and 13 counts; remove `#[allow(dead_code)]` suppressions for code now reachable after Phases 1-5

**Dependencies:** All prior phases (Phases 1-5)

**Done when:** All 7 previously-ignored `array_tests.rs` tests pass. #351 and #344 issues verified and closeable. `#[allow(dead_code)]` count in array-related files reduced. `docs/tech-debt.md` updated.
<!-- END_PHASE_6 -->

## Additional Considerations

**Fill policy for dimension-mismatched A2A assignment:** Out-of-bounds elements are NaN. This is implementation-defined per XMILE spec Section 3.7.1.2 (range extension is "non-standard"). NaN signals a likely model error; users who want a specific default can wrap in an IF checking array size. The `range_basic` and `range_with_expressions` tests need their expected values updated from last-element-repeat to NaN.

**xmutil EXCEPT handling:** Two of the four #345 tests (`simulates_except_xmile`, `simulates_except_xmile_interpreter_only`) are blocked by the C++ xmutil converter dropping EXCEPT semantics during MDL-to-XMILE conversion. This is out of scope; those tests remain ignored.

**GET DIRECT DATA with Excel:** The `simulates_directdata_mdl` test requires the `ext_data` feature (Excel file support). This is out of scope; the test remains ignored.

**VECTOR RANK naming:** Vensim names this function `VECTOR RANK`; the engine's AST uses `Rank`. The MDL parser needs to recognize `VECTOR RANK` and the xmile_compat layer needs a rename mapping.
