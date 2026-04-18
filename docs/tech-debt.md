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
- **Description**: 49 `#[allow(dead_code)]` attributes across 24 files. Heaviest in bytecode.rs (8), expr3.rs (5), dimensions.rs (4), compiler/context.rs (3), test_common.rs (3). Remaining suppressions fall into three categories: (1) ByteCodeContext builder methods unused in production because ByteCodeCompiler builds tables directly, (2) expr3 variants and methods reserved for pass 2, (3) scaffolding types (DimensionRange, DimensionVec, StridedDimension) for future strided array views. The stale Opcode-level suppression and reachable dimensions.rs code were cleaned up in the close-array-gaps work.
- **Measure**: `rg '#\[allow\(dead_code\)\]' --type rust src/simlin-engine/src/ -c`
- **Count**: 49 occurrences across 24 files (as of 2026-03-12)
- **Owner**: unassigned
- **Last reviewed**: 2026-03-12

### 13. Ignored Rust Tests

- **Component**: simlin-engine
- **Severity**: low
- **Description**: 23 tests are marked `#[ignore]`. 11 are in tests/simulate.rs (tracked individually by GitHub issues: #346 DELAY FIXED ring-buffer [4 tests], #347 GET DATA BETWEEN TIMES+implicit .dat loading [2 tests], #348 directdata/directconst/directlookups/directsubs [4 tests], #349 C-LEARN macro expansion [1 test]). 8 are in vdf.rs (VDF binary format tests). 2 are in json_sdai_proptest.rs (file system writes). 1 is in tests/mdl_equivalence.rs (tracked by item 1). 1 is in tests/mdl_roundtrip.rs. The close-array-gaps work enabled all 8 array_tests.rs tests plus 5 simulate.rs tests (#345 EXCEPT tests and the basic EXCEPT test).
- **Measure**: `rg '#\[ignore\]' --type rust src/simlin-engine/ -c`
- **Count**: 23 ignored tests across 5 files (as of 2026-03-12)
- **Owner**: unassigned
- **Last reviewed**: 2026-03-12

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
- **Description**: The `errors` and `unit_errors` fields on `Variable`, and the `errors` field on `ModelStage0`/`ModelStage1`, are now redundant with the salsa incremental compilation pipeline. Diagnostics are collected via `collect_all_diagnostics` / `collect_model_diagnostics` from tracked functions, making the embedded error fields dead weight carried through the monolithic compilation path. Removing them would simplify the data model and reduce confusion about the source of truth for errors. This cleanup was identified as Step 13 in the incremental compilation design (`docs/design/incremental-compilation.md`) but is not required by any acceptance criterion.
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

### 20. LTM FixedIndex References Expand to N-squared Edges

- **Component**: simlin-engine (src/simlin-engine/src/db_analysis.rs)
- **Severity**: high
- **Description**: `classify_element_dependency` lumps both wildcard reducers (`population[*]`) and fixed-index references (`population[NYC]`) under `CrossElement`, which `expand_edge_to_elements` then expands to the full N-by-N element cross-product. For a pattern like `relative_pop[R] = population / population[NYC]` the true element structure is two N-edge patterns (same-element and broadcast from NYC), not N-squared. On arrays with tens of elements the spurious edges trigger combinatorial loop-enumeration blow-ups even though their runtime link scores are effectively zero. Fix: add a `FixedIndex(element)` classification and emit `source[element] -> target[d]` edges instead of the all-to-all expansion.
- **Measure**: Build a test model with explicit subscript references and count element-level edges vs. `N + N` expected.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 21. LTM Polarity Analysis Has Reducer Blind Spots

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `analyze_expr_polarity_with_context`)
- **Severity**: medium
- **Description**: Only the scalar two-arg forms of `Max(a, Some(b))` / `Min(a, Some(b))` are handled; the array reducer forms (`Sum`, `Mean`, `Max(_, None)`, `Min(_, None)`, `Stddev`, `Rank`) fall through to `App(_, _, _) => Unknown`. Any variable computed via `SUM(x[*])` or `MEAN(x[*])` therefore contributes `Unknown` polarity, and every loop through it is classified `Undetermined`. For `Sum` and `Mean` polarity is trivially the argument's polarity (monotone in every element). Graphical-function monotonicity also uses a strict EPSILON=1e-10 check that flags numeric import noise as `Unknown`. Fix: add reducer cases (pass through for SUM/MEAN, Unknown for STDDEV/RANK) and consider a plateau-tolerant GF monotonicity check.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 22. LTM Dedup Keys Fold Distinct Directed Cycles with Matching Node Sets

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `IndexedGraph::dedup_circuits`, `CausalGraph::deduplicate_loops`)
- **Severity**: medium
- **Description**: Dedup hashes the sorted node-index vector. In a multidigraph a cycle A->B->C->A and the distinct cycle A->C->B->A share a node set and are silently merged into a single `Loop`. `test_layout_arms_race` exercises this today. For scalar SD models with asymmetric dependency structure the merge happens to be benign, but the semantics are wrong: the two cycles have distinct edge sequences and potentially distinct polarity products. Fix: key dedup by a canonical edge-sequence rotation (rotate so the lex-smallest node starts the cycle, then compare the ordered edge list) instead of the sorted node set.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 23. LTM Circuit Enumeration Is Tiernan-Style, Not Johnson's

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `IndexedGraph::enumerate_circuits_in_scc`)
- **Severity**: low
- **Description**: The DFS uses Tiernan (1970) lexicographic-restart with a per-start visited set; the comment labelling it "Johnson-style" is inaccurate. Real Johnson (1975) maintains a blocked set and B[w] unblock lists to achieve O((V+E)(C+1)) time; Tiernan can repeat exploration of the same subtree from multiple starts when those subtrees yield no cycle back to the start. On wrld3 with a dense 166-node SCC the current MAX_LTM_CIRCUITS=100_000 path is already fast (~26 ms) because cycles close early, but pathological graphs with long non-cyclic branches could benefit meaningfully from the switch (~50 lines of added code). Fix: either rename the comment to "Tiernan-style" or implement the blocked-set mechanism.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 24. LTM SearchGraph Uses String-Backed Idents in Hot Path

- **Component**: simlin-engine (src/simlin-engine/src/ltm_finding.rs `SearchGraph::check_outbound_uses`)
- **Severity**: medium
- **Description**: The per-timestep strongest-path DFS keys `best_score` and `visiting` on `Ident<Canonical>` (String-backed), cloning identifiers into hash maps on every recursive call. For a 1000-variable model with 500 saved timesteps this is ~5x10^7 map operations per run; element-level expansion makes it far worse. Apply the same NodeId indexing pattern that `IndexedGraph` uses in the exhaustive path: per-timestep `Vec<u32>`-indexed `SearchGraph`, dense `Vec<f64>` for `best_score`, `Vec<bool>` for `visiting`. Expected 5-10x speedup on large discovery runs.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 25. LTM Element-Level Loop Enumeration Runs at Wrong Granularity

- **Component**: simlin-engine (src/simlin-engine/src/db_analysis.rs `model_element_loop_circuits`)
- **Severity**: medium
- **Description**: `model_element_loop_circuits` enumerates circuits on the element graph. For a pure-A2A model with 20 variables over a 100-element dimension, every variable-level circuit produces 100 element-level circuits that `build_element_level_loops` then collapses into one A2A loop. The multiplication hits MAX_LTM_CIRCUITS=100_000 far sooner than variable-level enumeration would. Fix: enumerate at the variable level first, tag each loop's edges by same-element / cross-element / scalar, and only element-level-enumerate the cross-element subgraph. Cost becomes additive in N for pure-A2A loops instead of multiplicative.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 26. LTM A2A Partial Equation Is Wrong When Target Mixes Same-Element and Cross-Element References

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs `build_partial_equation`)
- **Severity**: medium
- **Description**: For a target like `share[R] = population / SUM(population[*])` the source `population` appears both as a bare (same-element) reference and inside a wildcard reducer. The A2A ceteris-paribus wrapper leaves all `population` references unchanged, so when `share[nyc]` is evaluated in the partial, `SUM(population[*])` uses CURRENT populations for every element instead of PREV for the non-target elements. The partial equals the full expression, link-score magnitude is always 1, and dominance is misattributed. Fix: detect dual-mode references during AST analysis and either split into separate same-element and cross-element link scores or fall back to explicit per-element scalar scores for the edge.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 27. LTM STDDEV/RANK Fallback Scores Are Silently Wrong

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs `generate_nonlinear_partial`)
- **Severity**: medium
- **Description**: For STDDEV and RANK the "nonlinear" reducer path returns the target variable itself, yielding a delta-ratio `(target - PREV(target)) / (source[d] - PREV(source[d]))` instead of a ceteris-paribus partial. Under uniform scaling of all elements, STDDEV does not change, but the delta-ratio still reports nonzero per-element attributions. Fix: unroll STDDEV element-by-element with the standard formula `sqrt(((s[d] - mean_p)^2 + sum_{i!=d}(PREV(s[i]) - mean_p)^2) / N)` where `mean_p = (s[d] + sum_{i!=d}PREV(s[i]))/N`. Similar unroll applies to RANK.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 28. LTM Discovery Truncates Before Partition-Scoped Filtering

- **Component**: simlin-engine (src/simlin-engine/src/ltm_finding.rs `rank_and_filter`)
- **Severity**: low
- **Description**: `rank_and_filter` sorts loops by average absolute score, truncates to MAX_LOOPS=200, and then applies MIN_CONTRIBUTION filtering per-partition. A loop that is dominant in a small partition but globally ranked below 200 is lost before the partition scope sees it. In practice MAX_LOOPS is generous enough that the case is rare; the comment already acknowledges the concern. Fix: filter first, truncate second.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 29. LTM LOOPSCORE / PATHSCORE Builtins Not Implemented

- **Component**: simlin-engine (LTM augmentation layer)
- **Severity**: low
- **Description**: The reference treats `LOOPSCORE(path...)` and `PATHSCORE(path...)` as primitives users invoke to track loops the heuristic discovery may have missed. Simlin does not implement them. Given discovery is heuristic, users currently have no way to pin a specific loop and compare it across runs or parameter sweeps. Fix: generate one synthetic variable per user-named loop that computes the product of its constituent link scores; coexists cleanly with discovery.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 30. LTM Polarity Confidence Metric Missing

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `LoopPolarity::from_runtime_scores`)
- **Severity**: low
- **Description**: The paper's polarity-confidence metric `|r - |b|| / (r + |b|)` classifies loops as Rux/Bux when mostly one polarity. Simlin collapses any sign change to Undetermined, losing information on mostly-reinforcing loops that briefly dip balancing (or vice versa). Fix: retain the ratio alongside the categorical polarity and surface it in `DetectedLoopsResult`.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 31. RK4 + LTM Combination Has No Hard-Error Guard

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs flow-to-stock path)
- **Severity**: medium
- **Description**: The 2023 flow-to-stock link-score formula assumes Euler integration: `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` aligns the numerator to the causal interval that drove the stock change from t-1 to t. Under RK2/RK4 this alignment breaks and link scores become mathematically nonsensical. Nothing currently prevents a user from setting `integration_method = RK4` and `ltm_enabled = true`; they'd get numbers that look plausible but are wrong. Fix: emit a compile-time diagnostic (preferably an Error) when LTM is enabled on a model whose sim specs select a non-Euler integrator.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 32. LTM Unused `_is_affecting_stock` Flag

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs:374)
- **Severity**: low
- **Description**: `generate_stock_to_flow_equation` computes `_is_affecting_stock` and discards it. Either use it (e.g., zero out scores for non-connected stock->flow pairs) or delete. Trivial cleanup.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17
