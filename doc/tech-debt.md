# Tech Debt Tracker

Known debt items consolidated from CLAUDE.md files and codebase analysis. Each entry has a description, component, severity, and measurement command.

## Items

### 1. MDL Parser C-LEARN Equivalence

- **Component**: simlin-engine (src/simlin-engine/src/mdl/)
- **Severity**: medium
- **Description**: 26 differences remain between the native Rust MDL parser and the C++ xmutil reference path. Root causes: missing initial-value comments, trailing tabs in dimension names, net flow synthesis differences, middle-dot canonicalization, GF y-scale computation.
- **Measure**: `cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture 2>&1 | grep 'DIFF'`
- **Count**: 26 diffs (as of January 2026)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 2. `unwrap_or_default()` Usage in simlin-engine

- **Component**: simlin-engine
- **Severity**: medium
- **Description**: `unwrap_or_default()` can mask unexpected conditions by silently substituting default values. Many uses are idiomatic (e.g. `map.get().unwrap_or_default()` for missing keys), but others should be replaced with explicit error handling or `Option`/`Result` propagation. Tracked as a measurement, not enforced by ratchet -- automated grep-based enforcement cannot distinguish idiomatic from problematic uses and incentivizes worse code.
- **Measure**: `rg 'unwrap_or_default\(\)' --type rust -c src/simlin-engine/`
- **Count**: 99 occurrences across 17 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 3. `println!` in Library Code

- **Component**: simlin-engine, libsimlin
- **Severity**: low
- **Description**: `println!` calls in library code (outside CLI and test code) should use proper logging or be removed. They can cause issues in WASM builds and pollute output for library consumers.
- **Measure**: `rg 'println!' --type rust src/simlin-engine/src/ src/libsimlin/src/ -c`
- **Count**: 55 in simlin-engine/src/, 6 in libsimlin/src/ (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 4. `@typescript-eslint/no-explicit-any` Disabled

- **Component**: TypeScript packages (diagram, app, server)
- **Severity**: medium
- **Description**: The `no-explicit-any` eslint rule is turned off. `any` types bypass TypeScript's type system and can mask bugs. Should be enabled with a gradual cleanup of existing violations.
- **Measure**: `rg 'no-explicit-any.*off' --type js --type ts`
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 5. Class Component Migration

- **Component**: diagram
- **Severity**: low
- **Description**: Project preference is class components, but hooks usage exists in the diagram package. When touching these files, prefer migrating to class components.
- **Measure**: `rg 'use(State|Effect|Ref|Memo|Callback|Context)\b' --glob '*.tsx' src/diagram/`
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 6. `@simlin/core` -> `@simlin/engine` Dependency Direction

- **Component**: core, engine
- **Severity**: low
- **Description**: `@simlin/core` depends on `@simlin/engine`. This means the "shared data models" package depends on the WASM engine wrapper. Evaluate whether to invert this relationship or restructure so core is truly a leaf package.
- **Measure**: Check `src/core/package.json` dependencies
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 7. `.unwrap()` in simlin-engine

- **Component**: simlin-engine
- **Severity**: medium
- **Description**: 1,276 `.unwrap()` calls across 59 files. Many are in test code (parser/tests.rs: 100, json_proptest.rs: 59, json_sdai_proptest.rs: 62) but a significant number are in production code paths. The highest concentrations are in vm.rs (316), mdl/parser.rs (55), json.rs (41), and mdl/convert/variables.rs (39). VM unwraps are largely on view_stack operations where emptiness would indicate a compiler bug, but other call sites could benefit from proper error propagation.
- **Measure**: `rg '\.unwrap\(\)' --type rust -c src/simlin-engine/src/`
- **Count**: 1,276 occurrences across 59 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 8. `.unwrap()` in libsimlin

- **Component**: libsimlin
- **Severity**: medium
- **Description**: 102 `.unwrap()` calls in production FFI code (excluding tests_remaining.rs). Panicking across an FFI boundary is undefined behavior. lib.rs has 55 unwraps, simulation.rs has 14, model.rs has 8. These should be converted to return error codes through the FFI error mechanism.
- **Measure**: `rg '\.unwrap\(\)' --type rust src/libsimlin/src/ --glob '!tests_remaining.rs' -c`
- **Count**: 102 occurrences across 8 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 9. Explicit `any` Types in TypeScript

- **Component**: TypeScript packages (diagram, server, app, core)
- **Severity**: medium
- **Description**: 42 explicit `: any` type annotations and 24 `as any` type assertions across the TypeScript codebase. Heaviest concentrations: Editor.tsx (14 `: any`), server/authn.ts (7 `: any`), server/models/table-firestore.ts (5 `: any`), Canvas.tsx (5 `as any`), VariableDetails.tsx (4 `as any`). These bypass type safety and should be replaced with proper types.
- **Measure**: `rg ': any\b' --glob '*.{ts,tsx}' src/ -c` and `rg 'as any\b' --glob '*.{ts,tsx}' src/ -c`
- **Count**: 42 `: any` + 24 `as any` = 66 total (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 10. `console.log/warn/error` in Production TypeScript

- **Component**: TypeScript packages (diagram, app, engine, server)
- **Severity**: low
- **Description**: 50 `console.*` calls in production TypeScript code (non-test). Breakdown: diagram (37 across 8 files, mostly VariableDetails.tsx with 19), app (5), engine (4), server (4). These should be replaced with structured logging or removed.
- **Measure**: `rg 'console\.(log|warn|error)\(' --glob '*.{ts,tsx}' src/diagram/ src/app/ src/engine/src/ src/server/ -c`
- **Count**: 50 occurrences across 18 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 11. TODO/FIXME Comments

- **Component**: all
- **Severity**: low
- **Description**: 81 TODO/FIXME/HACK/XXX comments across the codebase (Rust and TypeScript). Highest concentrations: simlin-engine/model.rs (10), xmile/mod.rs (6), array_tests.rs (5), xmile/variables.rs (4). These represent acknowledged but unresolved work items that should be triaged into tracked issues or resolved.
- **Measure**: `rg 'TODO|FIXME|HACK|XXX' --glob '*.{rs,ts,tsx}' src/ -c`
- **Count**: 81 occurrences across 36 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 12. `#[allow(dead_code)]` Suppressions

- **Component**: simlin-engine
- **Severity**: low
- **Description**: 58 `#[allow(dead_code)]` attributes across 23 files. Heaviest in bytecode.rs (11), dimensions.rs (10), expr3.rs (5), compiler/dimensions.rs (4). Most are scaffolding for incomplete array features (bytecode opcodes not yet emitted, dimension matching helpers). These should be cleaned up as array features land, and remaining ones should have justification comments.
- **Measure**: `rg '#\[allow\(dead_code\)\]' --type rust src/simlin-engine/src/ -c`
- **Count**: 58 occurrences across 23 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 13. Ignored Rust Tests

- **Component**: simlin-engine
- **Severity**: low
- **Description**: 13 tests are marked `#[ignore]`. 8 are in array_tests.rs (deferred array features: range operations, transpose, star-to-indexed subdimensions, bounds checking). 2 are in tests/simulate.rs (EXCEPT statement handling). 2 are in json_sdai_proptest.rs (file system writes). 1 is in tests/mdl_equivalence.rs (tracked by item 1). These represent planned but incomplete functionality.
- **Measure**: `rg '#\[ignore\]' --type rust src/simlin-engine/ -c`
- **Count**: 13 ignored tests across 4 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 14. TypeScript Test Coverage Gaps

- **Component**: app, core, server, engine, diagram
- **Severity**: medium
- **Description**: Large portions of TypeScript code lack corresponding test files. The app package has zero tests. The core package has 1 test file (datamodel.test.ts). The server package has 7 test files but no coverage for database models, auth helpers, route handlers, or rendering pipeline. The engine package has 8 test files covering the public API but no unit tests for internal modules (dispose, memory, import-export, error handling). The diagram package has 24 test files but none for the 23 component library files (Paper, Tabs, Card, etc.) or major UI modules (VariableDetails, ModelPropertiesDrawer, HostedWebEditor).
- **Measure**: `find src/{app,core,diagram,engine,server} -name '*.test.ts' -o -name '*.test.tsx' | grep -v node_modules | wc -l`
- **Count**: 40 test files total: diagram (24), engine (8), server (7), core (1), app (0) (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 15. `.clone()` Density in simlin-engine

- **Component**: simlin-engine
- **Severity**: low
- **Description**: 707 `.clone()` calls across 50 files. Most clones are in non-hot-path code (serde.rs: 49, ltm.rs: 49, model.rs: 47, units_infer.rs: 38, compiler/context.rs: 30). The VM hot path has only 33 clones across 5,513 lines, which is well-controlled. Many clones in compiler/ and model.rs are for building intermediate data structures during compilation, where ownership transfer is impractical. Worth monitoring but not actionable today.
- **Measure**: `rg '\.clone\(\)' --type rust src/simlin-engine/src/ -c`
- **Count**: 707 occurrences across 50 files (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 16. `eprintln!` in Library Code

- **Component**: simlin-engine, libsimlin
- **Severity**: low
- **Description**: 44 `eprintln!` calls in simlin-engine and 6 in libsimlin. In simlin-engine, 26 are in debug-gated functions (`debug_print_runlists` in interpreter.rs, `debug_print_bytecode` in vm.rs). The remaining 18 are runtime warnings in results.rs (unsupported sim methods), model.rs (compilation errors), and variable.rs. These should use proper error types or conditional logging rather than printing to stderr.
- **Measure**: `rg 'eprintln!' --type rust src/simlin-engine/src/ src/libsimlin/src/ -c`
- **Count**: 44 in simlin-engine, 6 in libsimlin (as of 2026-02-15)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-15

### 17. Remove Legacy Error Fields from Variable/ModelStage Types

- **Component**: simlin-engine
- **Severity**: low
- **Description**: The `errors` and `unit_errors` fields on `Variable`, and the `errors` field on `ModelStage0`/`ModelStage1`, are now redundant with the salsa incremental compilation pipeline. Diagnostics are collected via `collect_all_diagnostics` / `collect_model_diagnostics` from tracked functions, making the embedded error fields dead weight carried through the monolithic compilation path. Removing them would simplify the data model and reduce confusion about the source of truth for errors. This cleanup was identified as Step 13 in the incremental compilation design (`doc/design/incremental-compilation.md`) but is not required by any acceptance criterion.
- **Owner**: unassigned
- **Last reviewed**: 2026-02-22

### 18. Dimension-Granularity Incremental Invalidation

- **Component**: simlin-engine (src/simlin-engine/src/db.rs)
- **Severity**: low
- **Description**: When project dimensions change, all variables are currently re-parsed because `parse_source_variable` depends on the full dimension list via `SourceProject::dimensions`. A `variable_relevant_dimensions` tracked function could narrow invalidation to only variables whose equations reference changed dimensions, avoiding unnecessary re-parsing for unaffected variables. AC1.5 (dimension changes propagate correctly) is already satisfied by salsa's backdating -- this is a pure performance optimization. For current model sizes the overhead is negligible; this would matter for projects with many dimensions and thousands of variables.
- **Owner**: unassigned
- **Last reviewed**: 2026-02-22

### 19. Flaky Hypothesis Tests in pysimlin Due to Slow Input Generation

- **Component**: pysimlin (src/pysimlin/tests/test_json_types.py)
- **Severity**: medium
- **Description**: Several Hypothesis property-based tests intermittently fail with `FailedHealthCheck` because input generation is too slow. The affected tests are `TestJsonRoundtrip::test_stock_roundtrip`, `TestSchemaCompliance::test_flow_validates_against_schema`, and `TestPatchRoundtrip::test_upsert_stock_roundtrip`. The root cause is deeply nested composite strategies: `flow_strategy` and `auxiliary_strategy` conditionally invoke `graphical_function_strategy`, which itself draws from two `graphical_function_scale_strategy` instances plus variable-length point lists with constrained floats. The `stock_strategy` draws multiple `ident_strategy` lists. When Hypothesis explores complex branches (e.g., graphical functions with many points and both scales), generation time can exceed the default health check deadline, causing intermittent failures that are environment-dependent (slower in CI, under load, or in sandboxed environments). Possible fixes include: (1) adding `suppress_health_check=[HealthCheck.too_slow]` to the `@settings` decorator on affected tests, (2) simplifying strategies by reducing `max_size` parameters or using `st.builds` instead of `@st.composite` where possible, (3) caching or flattening nested composite strategies to reduce draw overhead, or (4) increasing the `deadline` setting. Option 2 is preferred as it addresses the root cause rather than suppressing the symptom.
- **Measure**: Run `cd src/pysimlin && uv run pytest tests/test_json_types.py -x --count=10` (with pytest-repeat) to observe intermittent failures
- **Count**: 3 affected tests (as of 2026-02-24)
- **Owner**: unassigned
- **Last reviewed**: 2026-02-24
