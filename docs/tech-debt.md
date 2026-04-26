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

### 19. Rust VDF Parser Boundary Parity

- **Component**: simlin-engine (src/simlin-engine/src/vdf.rs)
- **Severity**: high
- **Description**: `tools/vdf_xray.py` now handles several VDF boundary cases that the Rust parser still needs to adopt: section-6 empty ref streams must not advance past the section end, `SCEN01.VDF` slot-table detection should prefer the referenced prefix at `0x6b28` over the later candidate at `0x6b34`, non-Time sparse blocks can require `ceil(header[0x7c] / 8)` bitmap bytes while the Time block uses `ceil(header[0x78] / 8)` (`risk.vdf`, `risk2.vdf`), and raw-zero OT entries with class `0x11` plus missing final sentinel in `Ref.vdf` should decode as missing data rather than numeric zero constants.
- **Measure**: Port the Python assertions in `tools/test_vdf_xray.py` for `risk.vdf`, `risk2.vdf`, `run_1.vdf`, `run_2.vdf`, and `Ref.vdf` into Rust VDF tests.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-24

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
- **Severity**: RESOLVED (2026-04-25)
- **Description**: (**Resolved** during the per-reference element graph refactor; see commit ff3f1afe and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.) `classify_element_dependency` lumped both wildcard reducers (`population[*]`) and fixed-index references (`population[NYC]`) under `CrossElement`, which `expand_edge_to_elements` then expanded to the full N-by-N element cross-product. For a pattern like `relative_pop[R] = population / population[NYC]` the true element structure is two N-edge patterns (same-element and broadcast from NYC), not N-squared. On arrays with tens of elements the spurious edges triggered combinatorial loop-enumeration blow-ups even though their runtime link scores were effectively zero. The fix replaces the per-`(from, to)` classifier with an AST-walking per-reference emitter (`collect_reference_sites` + `emit_edges_for_reference`) that classifies each reference by `RefShape` and emits `source[element] -> target[d]` for `FixedIndex(element)` references rather than the all-to-all expansion.
- **Measure**: Build a test model with explicit subscript references and count element-level edges vs. `N + N` expected.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

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
- **Severity**: RESOLVED
- **Description**: (**Resolved** in commit aa56c5a1 on the reduce-ltm-mem branch.) Production code now implements Johnson 1975 with the blocked-set + B[w] unblock-list mechanism; the misnamed "Johnson-style" Tiernan variant is retained only under `#[cfg(test)]` as a test oracle for a Johnson-vs-Tiernan equivalence proptest.  Keeping the entry as a historical pointer to the commit that fixed it.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-18

### 24. LTM SearchGraph Uses String-Backed Idents in Hot Path

- **Component**: simlin-engine (src/simlin-engine/src/ltm_finding.rs `SearchGraph::check_outbound_uses`)
- **Severity**: medium
- **Description**: The per-timestep strongest-path DFS keys `best_score` and `visiting` on `Ident<Canonical>` (String-backed), cloning identifiers into hash maps on every recursive call. For a 1000-variable model with 500 saved timesteps this is ~5x10^7 map operations per run; element-level expansion makes it far worse. Apply the same NodeId indexing pattern that `IndexedGraph` uses in the exhaustive path: per-timestep `Vec<u32>`-indexed `SearchGraph`, dense `Vec<f64>` for `best_score`, `Vec<bool>` for `visiting`. Expected 5-10x speedup on large discovery runs.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-17

### 25. LTM Element-Level Loop Enumeration Runs at Wrong Granularity

- **Component**: simlin-engine (src/simlin-engine/src/db_analysis.rs `model_element_loop_circuits`)
- **Severity**: medium
- **Description**: `model_element_loop_circuits` enumerates circuits on the element graph. For a pure-A2A model with 20 variables over a 100-element dimension, every variable-level circuit produces 100 element-level circuits that `build_element_level_loops` then collapses into one A2A loop. The circuit count is no longer artificially capped (the old `MAX_LTM_CIRCUITS = 100_000` gate was retired on 2026-04-18 once auto-flip made it vestigial), but the `MAX_LTM_SCC_NODES = 50` gate in `model_ltm_variables` still flips dense feedback subgraphs to discovery mode far sooner than variable-level enumeration would. Fix: enumerate at the variable level first, tag each loop's edges by same-element / cross-element / scalar, and only element-level-enumerate the cross-element subgraph. Cost becomes additive in N for pure-A2A loops instead of multiplicative, and SCC width stays below the auto-flip threshold.
- **Note (2026-04-25):** The per-reference element graph refactor (`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`) eliminated the spurious NxN edge density that previously inflated element-graph SCCs on `FixedIndex`-using models. The Phase 5 measurement postscript records before/after numbers on cross_element_ltm, arrayed_population_ltm, hero_culture_ltm, and WRLD3-03: edge counts and circuit counts dropped on cross_element_ltm (20 -> 18 edges, 12 -> 8 circuits) without changing the largest element-SCC, and the other three fixtures were unchanged because they do not exercise the FixedIndex path. `MAX_LTM_SCC_NODES = 50` was retained because WRLD3 still trips the gate from variable-level cycle structure (population, capital, agriculture, persistent-pollution, non-renewable resources) rather than element-graph artifacts. The "enumerate at the variable level first, then expand only the cross-element subgraph" approach remains the right structural fix for pure-A2A models, but its pressure is materially lower now that FixedIndex no longer inflates SCC width.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

### 26. LTM A2A Partial Equation Is Wrong When Target Mixes Same-Element and Cross-Element References

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs `build_partial_equation`)
- **Severity**: RESOLVED (2026-04-25)
- **Description**: (**Resolved** alongside #20 via per-shape partial equations; see commit a3f595ac and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.) For a target like `share[R] = population / SUM(population[*])` the source `population` appears both as a bare (same-element) reference and inside a wildcard reducer. The old A2A ceteris-paribus wrapper left all `population` references unchanged, so when `share[nyc]` was evaluated in the partial, `SUM(population[*])` used CURRENT populations for every element instead of PREV for the non-target elements. The partial equalled the full expression, link-score magnitude was always 1, and dominance was misattributed. The fix splits the link score per `RefShape`: `model_ltm_variables` now emits one `LtmSyntheticVar` per `(from, to, shape)` triple, and `build_partial_equation_shaped` (in `ltm_augment.rs`) holds the matching-shape references live while wrapping the rest in `PREVIOUS()`. So `share = pop / SUM(pop[*])` produces both a Bare link score (`pop / PREVIOUS(SUM(pop[*]))`) and a Wildcard link score (`PREVIOUS(pop) / SUM(pop[*])`), each accurately attributing per-shape dominance.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

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

### 33. VDF 0x53 Result-Family Tail Is Undecoded

- **Component**: VDF tooling / simlin-engine
- **Severity**: medium
- **Description**: Local `third_party/uib_sd/zambaqui` files with magic `7f f7 17 53` parse like ordinary eight-section simulation-result VDFs for the primary run, but header word `0x68` points past the normal sparse-block run into an additional sensitivity/optimization-style payload. `tools/vdf_xray.py` can now inspect the ordinary run structures, but the extra tail and production Rust support for this result-family container are not decoded.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-24

### 34. A2A Loop Score Variable Broadcasts Slot 0 Across All Slots

- **Component**: simlin-engine (`src/simlin-engine/src/db_ltm.rs::compile_ltm_equation_fragment`, LTM-var-to-LTM-var dep stub)
- **Severity**: RESOLVED (2026-04-25)
- **Description**: (**Resolved** during issue #463 work.) For an A2A arrayed loop, the loop_score equation `"link_score⁚A→B" * "link_score⁚B→A" * ...` was being compiled with every link_score reference treated as a scalar (slot 0 only) instead of A2A. Root cause was at `compile_ltm_equation_fragment`'s LTM-var dep fallback (formerly `db_ltm.rs:798-816`): when an LTM equation depends on another LTM variable, the dep stub was hardcoded to `size: 1, ast: None`, forcing the compiler to emit slot-0 reads regardless of the dep's actual A2A dimensions. The fix mirrors the working pattern used for explicit model A2A vars (line 740-743 / `build_stub_variable`): look up the dep's `LtmSyntheticVar.dimensions` via salsa-cached `model_ltm_variables`, build an `Ast::ApplyToAll(canonical_dims, dummy_const)` stub when dimensions is non-empty, and use the right `product(dim_lengths)` size. Now `loop_score⁚<id>` slots correctly evaluate per-element. Verified by `test_a2a_loop_score_has_distinct_per_element_values` in `tests/simulate_ltm.rs` and the layout bite check `test_arrayed_loop_importance_matches_argmax_abs_aggregation` in `tests/layout.rs`. Two pre-existing tests (`test_arrayed_population_ltm_exhaustive`, `test_cross_element_ltm_exhaustive`) had assertions that passed pre-fix only because the broadcast bug hid equilibrium elements; relaxed to "at least one slot non-zero" to match real fixture semantics.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

### 35. A2A Loops Get `partition = None` in `loop_partitions`

- **Component**: simlin-engine (`src/simlin-engine/src/ltm.rs::CyclePartitions::partition_for_loop` + `db_ltm.rs::build_element_level_loops`)
- **Severity**: medium
- **Description**: The LTM partition map keys on **element-level** stock names (e.g., `population[nyc]`) because it is built from `model_element_cycle_partitions`. For pure-dimension A2A loops, however, `build_element_level_loops` calls `var_graph.find_stocks_in_loop(&var_level_nodes)` and stores **variable-level** stock names (e.g., `population`) in `Loop::stocks`. `partition_for_loop` then does `find_map(|s| stock_partition.get(s))` and the lookup misses, so every A2A loop returns `None`. Result: the LTM `loop_partitions` map systematically reports `None` for arrayed loops, regardless of which element-level SCC they actually belong to. Mixed/scalar loops use element-level stock names and partition correctly.
- **Why this matters**: The `compute_rel_loop_scores*` family normalises against partition denominators. With every A2A loop in the same fictitious "no parent" bucket, partition normalisation is wrong for any model that has multiple independent A2A loops (their rel-scores get cross-normalised against unrelated partitions). It also prevents mixed (A2A + scalar) partitions from arising in practice -- the codex review on PR #472 pointed at a real algorithmic bug in the layout aggregation that, today, can only be triggered through hand-crafted helper inputs because the engine never produces mixed-stride partitions due to this quirk.
- **Suspected fix**: Either (a) make `Loop::stocks` for A2A loops carry element-level names (one per element of the A2A dim, all of which should be in the same SCC by construction), or (b) extend `partition_for_loop` to expand a variable-level stock name to its element-level instances and look those up. (a) is more local but changes the `Loop` struct's semantic; (b) keeps the type unchanged. Either fix needs care that downstream consumers of `Loop::stocks` (e.g., `enumerate_module_pathways`) still get the names they expect.
- **Discovery context**: Found while writing an integration regression test for the codex P1 fix on PR #472. The test fixture deliberately built a model with both A2A and scalar feedback, expecting them to share a partition, and observed the A2A loop systematically getting `partition = None`. Documented in `test_compute_metadata_importance_series_length_matches_step_count` in `tests/layout.rs` so a future engine fix automatically begins exercising the mixed-stride path.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

### 36. darwin-x64 Not Included in @simlin/mcp and @simlin/serve npm Distributions

- **Component**: simlin-mcp, simlin-serve (`.github/workflows/serve-release.yml`, `mcp-release.yml`)
- **Severity**: low
- **Description**: The npm release workflows for `@simlin/mcp` and `@simlin/serve` do not produce a macOS Intel (darwin-x64) binary. Only `darwin-arm64` (Apple Silicon) and Linux targets are built. Intel Mac users cannot install these packages via npm optionalDependencies. The fix is to add `cargo-zigbuild --target x86_64-apple-darwin` steps to both release workflows and add `"@simlin/mcp-darwin-x64"` / `"@simlin/serve-darwin-x64"` entries to the respective `package.json` optionalDependencies.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25
