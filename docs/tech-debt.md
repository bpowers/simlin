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

- **Component**: simlin-engine (src/simlin-engine/src/db/analysis.rs)
- **Severity**: RESOLVED (2026-04-25)
- **Description**: (**Resolved** during the per-reference element graph refactor; see commit ff3f1afe and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.) `classify_element_dependency` lumped both wildcard reducers (`population[*]`) and fixed-index references (`population[NYC]`) under `CrossElement`, which `expand_edge_to_elements` then expanded to the full N-by-N element cross-product. For a pattern like `relative_pop[R] = population / population[NYC]` the true element structure is two N-edge patterns (same-element and broadcast from NYC), not N-squared. On arrays with tens of elements the spurious edges triggered combinatorial loop-enumeration blow-ups even though their runtime link scores were effectively zero. The fix replaces the per-`(from, to)` classifier with an AST-walking per-reference emitter (now `collect_all_reference_sites` in `db/ltm_ir.rs` + `emit_edges_for_reference` in `db/analysis.rs`) that classifies each reference by `RefShape` and emits `source[element] -> target[d]` for `FixedIndex(element)` references rather than the all-to-all expansion. The companion `Wildcard`/`DynamicIndex` (inlined-reducer) cross-product was then also eliminated via aggregate-node hoisting -- a maximal inlined reducer is rerouted through a synthetic `$⁚ltm⁚agg⁚{n}` node (only the rows the reducer's `read_slice` reads, O(N+M)); see `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` and commits 44527d17 / 3eca55fb.
- **Note (2026-05-13):** The conservative-slice carve-out (a reducer over an explicit slice used as a sub-expression, `x[r] = ... + SUM(pop[NYC, *])`) was resolved by #514 (`docs/design-plans/2026-05-11-ltm-arrays-hardening.md`, Phase 4 -- commits ec018d0b / bfb61d08 / 7f837928): `AggNode.read_slice` (`AxisRead ∈ {Pinned, Iterated, Reduced}`) makes a sliced reducer hoistable and routes only the read rows through the agg; an arrayed reducer over an iterated dim (`SUM(matrix[D1, *])`) mints an arrayed agg with one slot per element. The only thing that still takes the full cross-product from a reducer is the *bare dynamic index* -- `SUM(pop[idx, *])` with a non-literal `idx`, or `arr[i+1]` -- which isn't statically describable; that is a deliberate narrow carve-out (the IR reclassifies it as `DynamicIndex`). Mapped-dimension sliced reducers also stay conservative -- tracked as GH #534.
- **Measure**: Build a test model with explicit subscript references and count element-level edges vs. `N + N` expected.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-13

### 21. LTM Polarity Analysis Has Reducer Blind Spots

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `analyze_expr_polarity_with_context`)
- **Severity**: medium
- **Description**: Only the scalar two-arg forms of `Max(a, Some(b))` / `Min(a, Some(b))` are handled; the array reducer forms (`Sum`, `Mean`, `Max(_, None)`, `Min(_, None)`, `Stddev`, `Rank`) fall through to `App(_, _, _) => Unknown`. Any variable computed via `SUM(x[*])` or `MEAN(x[*])` therefore contributes `Unknown` polarity, and every loop through it is classified `Undetermined`. For `Sum` and `Mean` polarity is trivially the argument's polarity (monotone in every element). Graphical-function monotonicity also uses a strict EPSILON=1e-10 check that flags numeric import noise as `Unknown`. Fix: add reducer cases (pass through for SUM/MEAN, Unknown for STDDEV/RANK) and consider a plateau-tolerant GF monotonicity check.
- **Related (different code site)**: #516 (LTM tracking epic: #488) -- cross-element loops recovered through a synthetic `$⁚ltm⁚agg` node always classify `Undetermined` because `recover_cross_agg_loops` (`src/simlin-engine/src/db/ltm.rs`) builds the agg-hop links off the variable-level graph, which has no agg node, so `polarity(X→agg)` / `polarity(agg→Y)` are `Unknown`. Same SUM/MEAN-is-monotone fact as this item, applied to the synthetic aggregate's incoming/outgoing edges rather than to a `SUM(x[*])` user-variable RHS; the fix may share a "polarity of a monotone reducer's incoming edge" helper.
- **Note (2026-05-13):** The graphical-function-monotonicity half is resolved -- #492 (`docs/design-plans/2026-05-11-ltm-arrays-hardening.md`, Phase 7 -- commit ee07c935) replaces the strict `EPSILON=1e-10` test with a y-range-relative epsilon (`max(EPSILON, range_rel * (y_max - y_min))`), so near-flat lookup arms with imported numeric noise no longer flip a monotone curve to `Unknown`. Phase 7 also added per-element graphical-function static polarity (#502, commit 4991cd6c). The reducer-blind-spot half was handled by #480 (closed); the residual nuance that the monotonicity check compares the y-delta `dy` rather than the slope `dy/dx` (non-uniform x-spacing) is tracked as GH #536. The agg-hop-polarity half (`recover_agg_hop_polarities`) is tracked as #516.
- **Note (2026-06-09):** The y-delta-vs-slope residual (GH #536) is fully resolved -- `analyze_graphical_function_polarity` classifies each segment by its slope against a tolerance of `1e-6 * (y_max - y_min) / avg_dx` where `avg_dx` is the average x-spacing. On uniformly-spaced tables every `dx == avg_dx` so the per-segment dy threshold reduces EXACTLY to `1e-6 * (y_max - y_min)` -- the same value #492 used, preserving import-noise tolerance for finely-sampled tables. For non-uniform tables the threshold scales proportionally with segment width so narrow steep changes are still caught (the original #536 motivation). A degenerate `x[i] == x[i-1]` segment is skipped (duplicate point) or bails to `Unknown` (genuine vertical step).
- **Tracked in**: #480 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-05-13

### 22. LTM Dedup Keys Fold Distinct Directed Cycles with Matching Node Sets

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `IndexedGraph::dedup_circuits`, `CausalGraph::deduplicate_loops`)
- **Severity**: medium
- **Description**: Dedup hashes the sorted node-index vector. In a multidigraph a cycle A->B->C->A and the distinct cycle A->C->B->A share a node set and are silently merged into a single `Loop`. `test_layout_arms_race` exercises this today. For scalar SD models with asymmetric dependency structure the merge happens to be benign, but the semantics are wrong: the two cycles have distinct edge sequences and potentially distinct polarity products. Fix: key dedup by a canonical edge-sequence rotation (rotate so the lex-smallest node starts the cycle, then compare the ordered edge list) instead of the sorted node set.
- **Tracked in**: #308 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

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
- **Tracked in**: #481 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

### 25. LTM Element-Level Loop Enumeration Runs at Wrong Granularity

- **Component**: simlin-engine (src/simlin-engine/src/db/analysis.rs `model_loop_circuits_tiered`)
- **Severity**: RESOLVED (pending #496 merge)
- **Description**: (**Resolved** by the variable-level loop enumeration refactor; see `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`. Marked RESOLVED contingent on PR #496 landing on main.) `model_element_loop_circuits` enumerated circuits on the full element graph; for a pure-A2A model with V variables over an N-element dimension, every variable-level cycle inflated to N element-level circuits that `build_element_level_loops` collapsed back to one A2A loop. The fix is `model_loop_circuits_tiered`: it runs Johnson on the variable graph first, classifies each cycle by its `RefShape` composition via `model_edge_shapes`, and emits pure-scalar / pure-A2A cycles as Loops directly. Only cross-element / mixed cycles flow through element-level Johnson, and only on the subgraph induced by their nodes. The auto-flip gate now keys on the variable-level SCC (cheap pre-Johnson check) and the slow-path subgraph SCC (computed inside the tiered enumerator) rather than the full N-times-inflated element-graph SCC.
- **Note (2026-04-25):** The per-reference element graph refactor (`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`) had earlier eliminated the spurious NxN edge density on `FixedIndex`-using models. After that refactor pure-A2A models already had small element-graph SCCs; the remaining win for tiered enumeration is the elimination of redundant per-element circuit enumeration (O(N) circuits per pure-A2A cycle that were collapsed back to one Loop). `MAX_LTM_SCC_NODES = 50` is retained: WRLD3 still trips it because its variable-level SCC is 166 (population, capital, agriculture, persistent-pollution, non-renewable resources) and the tiered enumerator gates on that.
- **Tracked in**: #482 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-05-06

### 26. LTM A2A Partial Equation Is Wrong When Target Mixes Same-Element and Cross-Element References

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs `build_partial_equation`)
- **Severity**: RESOLVED (2026-04-25; reducer half superseded 2026-05-09)
- **Description**: (**Resolved** alongside #20 via per-shape partial equations; see commit a3f595ac and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.) For a target like `share[R] = population / SUM(population[*])` the source `population` appears both as a bare (same-element) reference and inside a wildcard reducer. The old A2A ceteris-paribus wrapper left all `population` references unchanged, so when `share[nyc]` was evaluated in the partial, `SUM(population[*])` used CURRENT populations for every element instead of PREV for the non-target elements. The partial equalled the full expression, link-score magnitude was always 1, and dominance was misattributed. The fix splits the link score per `RefShape`: `model_ltm_variables` emitted one `LtmSyntheticVar` per `(from, to, shape)` triple, and `build_partial_equation_shaped` (in `ltm_augment.rs`) holds the matching-shape references live while wrapping the rest in `PREVIOUS()`. So `share = pop / SUM(pop[*])` produced both a Bare link score (`pop / PREVIOUS(SUM(pop[*]))`) and a Wildcard link score (`PREVIOUS(pop) / SUM(pop[*])`). **Superseded-by**: the per-shape *Wildcard* link score described here (the `…⁚wildcard` variant) was retired in commit 49be92dc; reducer references are now hoisted into a `$⁚ltm⁚agg⁚{n}` aggregate node and the lumped reducer link score is decomposed into the chain `pop[d] → agg → share[r]` -- the `pop[d] → agg` half recovers each source element's fractional contribution to the aggregate's velocity (the `…⁚wildcard` variant's variable-level Δpop denominator never did). The Bare-vs-FixedIndex per-shape split survives. (Side note for #517: the A2A path emits the agg's reducer-shortcut numerator as `pop[r]/SUM(PREVIOUS(pop[*]))`, not the desired `pop[r]/PREVIOUS(SUM(pop[*]))`; `SUM(PREVIOUS(arr[*]))` evaluating to 0 under A2A is GH #517's territory.) See `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-09

### 27. LTM STDDEV/RANK Fallback Scores Are Silently Wrong

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs `generate_nonlinear_partial`)
- **Severity**: RESOLVED (2026-05-13; STDDEV) / WONTFIX-by-design (RANK)
- **Description**: For STDDEV and RANK the "nonlinear" reducer path returned the target variable itself, yielding a delta-ratio `(target - PREV(target)) / (source[d] - PREV(source[d]))` instead of a ceteris-paribus partial. Under uniform scaling of all elements, STDDEV does not change, but the delta-ratio still reported nonzero per-element attributions. The fix unrolls STDDEV element-by-element with the standard formula `sqrt(((s[d] - mean_p)^2 + sum_{i!=d}(PREV(s[i]) - mean_p)^2) / N)` where `mean_p = (s[d] + sum_{i!=d}PREV(s[i]))/N`.
- **Note (2026-05-13):** MIN and MAX (the scalar two-arg forms) were already handled correctly via explicit nested binary calls with selective `PREVIOUS()` wrapping. STDDEV is now resolved -- #483 (`docs/design-plans/2026-05-11-ltm-arrays-hardening.md`, Phase 6 -- commits 26ed48d3 / dd838dde) builds the analytic population-variance `sqrt` partial in `generate_nonlinear_partial` (divisor `N`, matching `vm.rs::Opcode::ArrayStddev`; the mean string-inlined; single-element variance is identically 0). **RANK keeps the delta-ratio by design**: RANK is an order statistic (non-differentiable) and returns an array, so it can never be a real scalar/A2A reducer RHS -- the delta-ratio is the documented conservative stand-in, pinned by `test_generate_rank_keeps_delta_ratio` so the choice is explicit, not a silent fallback. (The pre-#483 `ltm_augment.rs:1374-1381` line ref is stale; the RANK arm is the `"RANK"` case in `generate_nonlinear_partial`.)
- **Tracked in**: #483 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-05-13

### 28. LTM Discovery Truncates Before Partition-Scoped Filtering

- **Component**: simlin-engine (src/simlin-engine/src/ltm_finding.rs `rank_and_filter`)
- **Severity**: low
- **Description**: `rank_and_filter` sorts loops by average absolute score, truncates to MAX_LOOPS=200, and then applies MIN_CONTRIBUTION filtering per-partition. A loop that is dominant in a small partition but globally ranked below 200 is lost before the partition scope sees it. In practice MAX_LOOPS is generous enough that the case is rare; the comment already acknowledges the concern. Fix: filter first, truncate second.
- **Tracked in**: #310 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

### 29. LTM LOOPSCORE / PATHSCORE Builtins Not Implemented

- **Component**: simlin-engine (LTM augmentation layer)
- **Severity**: low
- **Description**: The reference treats `LOOPSCORE(path...)` and `PATHSCORE(path...)` as primitives users invoke to track loops the heuristic discovery may have missed. Simlin does not implement them. Given discovery is heuristic, users currently have no way to pin a specific loop and compare it across runs or parameter sweeps. Fix: generate one synthetic variable per user-named loop that computes the product of its constituent link scores; coexists cleanly with discovery.
- **Tracked in**: #484 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

### 30. LTM Polarity Confidence Metric Missing

- **Component**: simlin-engine (src/simlin-engine/src/ltm.rs `LoopPolarity::from_runtime_scores`)
- **Severity**: low
- **Description**: The paper's polarity-confidence metric `|r - |b|| / (r + |b|)` classifies loops as Rux/Bux when mostly one polarity. Simlin collapses any sign change to Undetermined, losing information on mostly-reinforcing loops that briefly dip balancing (or vice versa). Fix: retain the ratio alongside the categorical polarity and surface it in `DetectedLoopsResult`. Implementation in flight on branch `ltm/485-polarity-confidence` (PR #490, in draft); mark RESOLVED with the merge commit hash once the PR lands.
- **Tracked in**: #485 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-05-06

### 31. RK4 + LTM Combination Has No Hard-Error Guard

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs flow-to-stock path)
- **Severity**: medium
- **Description**: The 2023 flow-to-stock link-score formula assumes Euler integration: `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` aligns the numerator to the causal interval that drove the stock change from t-1 to t. Under RK2/RK4 this alignment breaks and link scores become mathematically nonsensical. Nothing currently prevents a user from setting `integration_method = RK4` and `ltm_enabled = true`; they'd get numbers that look plausible but are wrong. Fix: emit a compile-time diagnostic (preferably an Error) when LTM is enabled on a model whose sim specs select a non-Euler integrator.
- **Tracked in**: #486 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

### 32. LTM Unused `_is_affecting_stock` Flag

- **Component**: simlin-engine (src/simlin-engine/src/ltm_augment.rs:374)
- **Severity**: RESOLVED (2026-04-29)
- **Description**: (**Resolved** in commit f408528b "engine: drop unused `_is_affecting_stock` flag in stock-to-flow link score".) `generate_stock_to_flow_equation` no longer computes the unused flag; structural routing now happens in `generate_link_score_equation` (`ltm_augment.rs:607-637`) before the three per-shape helpers (`generate_flow_to_stock_equation`, `generate_stock_to_flow_equation`, `generate_auxiliary_to_auxiliary_equation`) are invoked. Keeping the entry as a historical pointer to the commit that removed it.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-29

### 33. VDF 0x53 Result-Family Tail Is Undecoded

- **Component**: VDF tooling / simlin-engine
- **Severity**: medium
- **Description**: Local `third_party/uib_sd/zambaqui` files with magic `7f f7 17 53` parse like ordinary eight-section simulation-result VDFs for the primary run, but header word `0x68` points past the normal sparse-block run into an additional sensitivity/optimization-style payload. `tools/vdf_xray.py` can now inspect the ordinary run structures, but the extra tail and production Rust support for this result-family container are not decoded.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-24

### 34. A2A Loop Score Variable Broadcasts Slot 0 Across All Slots

- **Component**: simlin-engine (`src/simlin-engine/src/db/ltm.rs::compile_ltm_equation_fragment`, LTM-var-to-LTM-var dep stub)
- **Severity**: RESOLVED (2026-04-25; cross-element assertions tightened 2026-05-09)
- **Description**: (**Resolved** during issue #463 work.) For an A2A arrayed loop, the loop_score equation `"link_score⁚A→B" * "link_score⁚B→A" * ...` was being compiled with every link_score reference treated as a scalar (slot 0 only) instead of A2A. Root cause was at `compile_ltm_equation_fragment`'s LTM-var dep fallback (formerly `db/ltm.rs:798-816`): when an LTM equation depends on another LTM variable, the dep stub was hardcoded to `size: 1, ast: None`, forcing the compiler to emit slot-0 reads regardless of the dep's actual A2A dimensions. The fix mirrors the working pattern used for explicit model A2A vars (line 740-743 / `build_stub_variable`): look up the dep's `LtmSyntheticVar.dimensions` via salsa-cached `model_ltm_variables`, build an `Ast::ApplyToAll(canonical_dims, dummy_const)` stub when dimensions is non-empty, and use the right `product(dim_lengths)` size. Now `loop_score⁚<id>` slots correctly evaluate per-element. Verified by `test_a2a_loop_score_has_distinct_per_element_values` in `tests/simulate_ltm.rs` and the layout bite check `test_arrayed_loop_importance_matches_argmax_abs_aggregation` in `tests/layout.rs`. Two pre-existing tests (`test_arrayed_population_ltm_exhaustive`, `test_cross_element_ltm_exhaustive`) had assertions that passed pre-fix only because the broadcast bug hid equilibrium elements; those were initially relaxed to "at least one slot non-zero". **Subsequently tightened**: once cross-element loops became scored on the element-level path (the per-reference element graph + the aggregate-node work; see commit d14c978b and `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`), `test_cross_element_ltm_exhaustive` re-asserts a slot-by-slot non-zero check on the A2A reinforcing births loop and pins the exact element-path factor set of the cross-element migration loop's loop-score equation. The migration loops still legitimately zero out one slot due to `MAX(...)` semantics in `migration_in` / `migration_out`, which is fixture behavior, so the "at least one slot non-zero" form persists for those.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-09

### 35. A2A Loops Get `partition = None` in `loop_partitions`

- **Component**: simlin-engine (`src/simlin-engine/src/ltm.rs::CyclePartitions::partition_for_loop` + `db/ltm.rs::build_element_level_loops`)
- **Severity**: RESOLVED (2026-05-13)
- **Description**: The LTM partition map keyed on **element-level** stock names (e.g., `population[nyc]`) because it is built from `model_element_cycle_partitions`. For pure-dimension A2A loops, however, `build_element_level_loops` called `var_graph.find_stocks_in_loop(&var_level_nodes)` and stored **variable-level** stock names (e.g., `population`) in `Loop::stocks`. `partition_for_loop` then did `find_map(|s| stock_partition.get(s))` and the lookup missed, so every A2A loop returned `None`. Result: the LTM `loop_partitions` map systematically reported `None` for arrayed loops, regardless of which element-level SCC they actually belonged to.
- **Why this matters**: The `compute_rel_loop_scores*` family normalises against partition denominators. With every A2A loop in the same fictitious "no parent" bucket, partition normalisation was wrong for any model that has multiple independent A2A loops (their rel-scores got cross-normalised against unrelated partitions).
- **Note (2026-05-13):** Resolved by #487 (`docs/design-plans/2026-05-11-ltm-arrays-hardening.md`, Phase 2 -- commits 11eb1af1 / 40bf7d51 / f26330f6, plus c3318a2c relaxing the `sim_new` loop-partition assert in libsimlin). Approach (a): an A2A `Loop`'s `stocks` now carry **element-level** stock names (the element-subscripted stocks the loop actually traverses), and `loop_partitions` is keyed *per slot* of the A2A loop -- `LtmVariablesResult::loop_partitions: HashMap<String, Vec<Option<usize>>>` -- since two elements of the same A2A loop can land in different cycle partitions. `compute_rel_loop_scores` normalises each `(partition, slot)` loop score against the per-(partition, slot) sum, so an independent A2A loop no longer cross-pollutes a sibling. libsimlin consumes the per-slot data via the subscripted loop-id accessor (commit 40bf7d51); no new C accessor was needed.
- **Tracked in**: #487 (LTM tracking epic: #488)
- **Owner**: unassigned
- **Last reviewed**: 2026-05-13

### 36. darwin-x64 Not Included in @simlin/mcp and @simlin/serve npm Distributions

- **Component**: simlin-mcp, simlin-serve (`.github/workflows/serve-release.yml`, `mcp-release.yml`)
- **Severity**: low
- **Description**: The npm release workflows for `@simlin/mcp` and `@simlin/serve` do not produce a macOS Intel (darwin-x64) binary. Only `darwin-arm64` (Apple Silicon) and Linux targets are built. Intel Mac users cannot install these packages via npm optionalDependencies. The fix is to add `cargo-zigbuild --target x86_64-apple-darwin` steps to both release workflows and add `"@simlin/mcp-darwin-x64"` / `"@simlin/serve-darwin-x64"` entries to the respective `package.json` optionalDependencies.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-25

### 37. macOS Watcher Loses Pre-Existing-File Removal and Rename Events

- **Component**: simlin-serve (`src/watcher.rs`, `tests/watcher_merge.rs`)
- **Severity**: medium
- **Description**: Three watcher behaviours are lost on macOS for files that existed *before* the watcher's `FSEventStreamCreate` subscription:
  1. **Rename source side**: an external `mv a.sd.json b.sd.json` doesn't produce a paired `Modify(Name(Both))` event. FSEvents reports each side as a single-path `Modify(Name(Any))`, and `notify-debouncer-full` only pairs them when its file-id cache already knows the source — which only happens for files that arrived via a `Create` event after the watcher started. The destination side merges via `handle_model_change`; the source side stays in the registry as a phantom entry.
  2. **Rename-collision dual-removal**: `mv a b` where both are tracked never broadcasts the second `ProjectRemoved` because the destination side surfaces as a content-modify into the existing entry rather than a `Modify(Name(Both))` that would route to the `AlreadyExists` arm of `handle_model_rename`.
  3. **Plain `unlink()` of a pre-existing file**: `external_remove_drops_registry_entry_and_broadcasts_removed` consistently times out on macOS-latest waiting for `ProjectRemoved`. The classify branch added in commit 7faf89d4 (treating `Modify(Name(_))` on a missing leaf as `Removed`) covers the rename-flagged-unlink case but does not move the test, so the underlying event must be arriving as something else (or not at all). Sister tests that *create* a file inside the watch window and then mutate or remove it pass on macOS, suggesting the file-id cache miss is the common factor.
- The Linux-equivalent flows work because inotify's `MOVED_FROM`/`MOVED_TO` cookies always arrive together regardless of cache state, and `IN_DELETE` is a first-class event with no FSEvents-style flag coalescing.
- **Symptoms**: external rename of a tracked file: SPA's editor for the old path may not migrate cleanly to the new path; rename-collision over a tracked destination merges contents instead of dropping both stale Loro states; `ProjectRenamed` is never broadcast on macOS; an external `rm` of a tracked file may leave a phantom registry entry.
- **Test impact**: three integration tests are gated with `#[cfg_attr(target_os = "macos", ignore = ...)]`:
  - `external_remove_drops_registry_entry_and_broadcasts_removed`
  - `external_rename_re_keys_registry_and_emits_project_renamed`
  - `rename_over_tracked_destination_removes_both_and_rehydrates`
- **Possible fixes** (need design discussion, not papered over):
  - **Hydrate the file-id cache at watcher start** by recursively scanning the root and registering each existing file with the debouncer's cache (e.g. via `notify_debouncer_full::FileIdMap::add_path`). Closes both the rename-source-side miss and the unlink miss for pre-existing files at the cost of a one-shot scan.
  - **Heuristic post-hoc pairing / inference in our actor**: keep a short-lived (<=200ms) "recently disappeared registry entries" buffer; pair `Modify(Name(_))` events with a recent removal whose content hash matches.
  - **Switch to `notify::PollWatcher` on macOS** for the smoke / CI surface and accept higher latency in exchange for deterministic event delivery.
  - **Accept the macOS UX gap** as documented and update the SPA client to handle the unpaired event sequence (a removal followed by a separate hydrate of the destination) without losing in-flight Loro edits — likely requires Loro doc state to migrate via content rather than via path-key.
- **Investigation log (2026-04-26)**: classify-side fix routes missing-leaf `Modify(Name(_))` → `Removed` was insufficient; path-resolution fix using `resolve_canonical_path` (canonicalize the deepest existing ancestor) made the Linux semantics platform-correct but did not move the macOS test 1 either, which strongly suggests the underlying event simply isn't reaching the actor. Without a macOS box to instrument the FSEvents stream directly, the next investigative step is to spawn a debug binary on a macOS runner that subscribes to FSEvents directly and prints the raw flags it receives for the test scenario.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-26

### 38. Windows Smoke Save Returns 500 (Atomic-Replace Race)

- **Component**: simlin-serve (`src/handlers.rs::save_project`, `simlin-engine/src/io.rs::atomic_write`, `tests/smoke.rs`)
- **Severity**: medium
- **Description**: On windows-latest the smoke test's `POST /api/projects/teacup.xmile` save returns `500 Internal Server Error` with the generic `SaveError::Internal` body. The Linux and macOS smoke jobs both pass through the same end-to-end code path, so the failure is Windows-specific. The most likely root cause is the Windows-only branch in `simlin-engine::io::atomic_write` that calls `fs::remove_file(target)` before `fs::rename(tmp, target)` — std's `rename` does not overwrite on Windows, so the target must be unlinked first, but if the in-process watcher's `notify-debouncer-full` is holding a directory handle (via `ReadDirectoryChangesW`) at the same path, the unlink can be deferred or the rename can lose to a kernel-level open-handle race.
- **Symptoms**: Linux + macOS smoke pass; Windows smoke fails on the first save assertion at `tests/smoke.rs:316:5`. The save POST receives a 500 within seconds, with the response body `{"error":"internal server error"}`.
- **Test impact**: `tests/smoke.rs` is gated with `#![cfg(not(target_os = "windows"))]` so the windows-latest matrix entry compiles, links, runs the test binary, and reports zero tests run. The `Build simlin-serve binary` step on Windows still validates that the cargo build itself succeeds end-to-end, so a regression that breaks the Windows compile (rather than runtime) still trips CI.
- **Investigation hints**:
  - The harness already captures the spawned binary's stdout (where `tracing_subscriber::fmt()` writes by default) and stderr, and `ChildGuard::drop` dumps both on test panic. So the next failing CI run on Windows will surface the underlying `tracing::error!` from `handlers.rs:708` in the job log, naming the specific err value behind `SaveError::Internal`. Read that first.
  - Once the err is visible, the most likely paths are (a) `commit_write` -> `atomic_write` returning a Windows I/O error from `remove_file` or `rename`; (b) `serialize_project` failing on Windows-style line endings if Git's autocrlf checked out `teacup.xmile` with `\r\n` and the XMILE writer emits something incompatible; (c) `redirect_to_sidecar` / registry `ensure_or_get` racing with the watcher.
  - A fix worth trying as a hypothesis test: switch `atomic_write` on Windows to use `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` via `windows-sys` instead of the `remove`-then-`rename` two-step. That eliminates the open-handle window the watcher might be sitting in.
- **Owner**: unassigned
- **Last reviewed**: 2026-04-27

### 39. Web deploy uploads the whole monorepo and GAE installs the full dep set

- **Component**: deploy (`scripts/deploy-web.sh`, `app.yaml`, `.gcloudignore`, `src/server/package.json`)
- **Severity**: medium
- **Description**: `pnpm deploy:web` runs `gcloud app deploy` from the repo root, which uploads the entire workspace (minus `.gcloudignore`). On the App Engine instance, GAE's Node buildpack runs `pnpm install` against the root `package.json` + `pnpm-workspace.yaml`, which installs dependencies for **every** workspace package -- including `@rsbuild/*`, `slate`, `radix-ui`, `jest`, rspress, and so on. None of this is needed at runtime: the server's actual prod-dep closure is ~10 third-party packages plus `@simlin/core` and `@simlin/engine`. The `NODE_ENV=production` env var that should skip devDependencies is broken under pnpm v10 (upstream bug [GoogleCloudPlatform/buildpacks#591](https://github.com/GoogleCloudPlatform/buildpacks/issues/591), still open as of 2026-05-08).
- **Symptom**: GAE deploy is slow and the instance image is fat. `.npmrc`'s `strict-peer-dependencies=true` makes any unmet transitive peer in the unrelated workspace packages abort the whole deploy. A new devDep added to e.g. `src/diagram` for purely local tooling can break production deploys.
- **Status (2026-06-27): implemented as `pnpm deploy:web:staged`, locally proven, pending a real `gcloud --no-promote` test before it replaces `deploy:web`.** The staged path (`scripts/deploy-web-staged.sh` -> `scripts/build-deploy-staging.mjs` + the pure transforms in `scripts/deploy-staging-manifests.mjs`) assembles a self-contained `deploy-staging/` whose `package.json` is exactly the server's prod closure, and deploys `gcloud app deploy deploy-staging/app.yaml`. The workspace-ref blocker is resolved by vendoring `@simlin/core` and `@simlin/engine` into `deploy-staging/vendor/` and rewriting them (and core's transitive `@simlin/engine` ref) to `file:` deps; a `pnpm-lock.yaml` is generated for the minimal manifest. Confirmed the buildpack lever from Google's docs: App Engine standard ALWAYS reinstalls from the deployed `package.json` + lockfile and has no vendored-`node_modules` escape hatch, so the only thing that shrinks the install is the deployed manifest. Note the pnpm10 devDep bug is a side issue -- the dominant bloat is installing the *whole workspace* (app/diagram/website/serve-web prod deps: React, slate, radix, rspress, vite), which a `--prod` install at the root would still pull.
- **Locally verified (2026-06-27)**: builder runs; an isolated `pnpm install --frozen-lockfile --prod` OUTSIDE the repo (no workspace/`.npmrc` ancestor, the GAE-faithful case) succeeds from the generated lockfile; the full server-side `libsimlin.wasm` (with `simlin_project_render_png`) materializes at the `__dirname` path; `node lib/index.js` boots ("WASM module initialized"), serves `/`->200, `/api/user`->401, `/static/js/sd-component.js`->200, `/robots.txt`->200. Result: **590 MB / 1171 pkgs -> 80 MB / 230 pkgs** (~86% smaller), 28.7 MB upload payload. Unit tests in `scripts/tests/deploy-staging-manifests.test.mjs`.
- **Remaining before retiring `deploy:web`**: a real `gcloud app deploy --no-promote` against a staging GAE version to confirm the nodejs24 buildpack honors `packageManager: pnpm@10.6.0` (corepack) and accepts the frozen `lockfileVersion`. Until then `deploy:web` stays as the proven fallback. This also incidentally fixes #695 (the staged upload is ~clean, well under the 10k file cap) and narrows #40 (only repo `public/` is still transiently mutated by the reused `deploy:assemble`).
- **Owner**: unassigned

### 40. Web deploy mutates tracked public/

- **Component**: deploy (`src/app/package.json` `deploy:assemble` / `deploy:clean`, `scripts/deploy-web.sh`, `public/`)
- **Severity**: low
- **Description**: The deploy step copies the rsbuild output (`src/app/build/*`, `src/app/build-component/static/...`) directly into the tracked `public/` directory and removes the tracked symlinks (`src/app/public`, `src/server/public`, `src/server/default_projects`) so `gcloud` doesn't traverse the same content twice. The cleanup step (`deploy:clean`) restores them with `git checkout HEAD --` and `rm -rf public/asset-manifest.json public/static/{js,wasm,css,media}`. This works -- a bash `trap` in `scripts/deploy-web.sh` now guarantees the cleanup runs even on Ctrl-C/error -- but it's structurally fragile: any new build output category that lands under `public/static/` needs to be added to the `rm -rf` list (we missed `static/css` and `static/media` until 2026-05-08), and a partial-cleanup recovery still requires a manual `git checkout`.
- **Why it's not fixed yet**: The clean fix overlaps with item 39 (staging-dir deploy): assemble the SPA into `<dir>/public/` rather than the workspace `public/`, and `gcloud app deploy` from `<dir>`. With nothing written into the tracked tree, no cleanup is needed -- `<dir>` is just removed when done. Same blockers as 39.
- **Mitigation in place**: `scripts/verify-deploy-build.sh` and a `git status --porcelain`-must-be-empty check in CI's `frontend` job catch the "missed a path in deploy:clean" regression class on every PR.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 41. POST /api/projects/:u/:p is not transactional

- **Component**: server (`src/server/api.ts:240-307`)
- **Severity**: medium
- **Description**: `app.db.file.create(file.getId(), file)` runs unconditionally before the version-conditional `project.update`, so a 409 (concurrent update) leaves an orphaned file blob in Firestore. The `setTimeout(() => preview.deleteOne(...))` also runs unconditionally, regardless of whether the project update actually succeeded -- which then regenerates the preview from the unchanged underlying state. Pre-existing.
- **Suspected fix**: Move file create + project update into a single Firestore transaction; gate preview invalidation on `result !== null`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 42. Username and project-name validation absent; Firestore filterId is single-replace

- **Component**: server (`src/server/api.ts:309-370`, `src/server/models/table-firestore.ts:38-40`)
- **Severity**: medium
- **Description**: There is no allowlist regex on usernames or project names at the API layer. A username containing `/` registers fine, but Firestore then rejects subsequent `${userId}/${projectSlug}` writes as INVALID_ARGUMENT, bricking the account. `FirestoreTable.filterId` uses `id.replace('/', '|')` (single replace), so an id with multiple `/` is partially escaped, and a username containing a literal `|` could collide with the escaped form of another user's id. Pre-existing.
- **Suspected fix**: Apply an allowlist regex such as `/^[a-z0-9][a-z0-9-]{0,38}$/` to usernames (and a similar one to project names) at the API boundary; switch `filterId` to `replaceAll('/', '|')`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 43. Private-project name enumeration via response-shape asymmetry

- **Component**: server (`src/server/route-handlers.ts:50-73`)
- **Severity**: medium
- **Description**: `createProjectRouteHandler` returns 404 for projects that don't exist, but 302 (redirect to `/`) for private projects that the requester doesn't own. The two responses are trivially distinguishable by an unauthenticated client, allowing enumeration of private project names belonging to a known username. Mild risk for an SD-modeling tool but a real information leak. Introduced in commit d44d3aea (`#210`).
- **Suspected fix**: Standardise on the same response (e.g. always 404) for "doesn't exist" and "exists privately, not yours".
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 44. populateExamples uses an async predicate in Array.filter

- **Component**: server (`src/server/new-user.ts:58-67`)
- **Severity**: medium
- **Description**: `files.filter(async (file) => { ... return stats.isDirectory(); })` returns a Promise from every callback, and Promises are always truthy, so `filter` is a no-op: every directory entry (including non-directories) flows into the subsequent `populateExample` loop. The bug is masked today only because each `populateExample` call is wrapped in try/catch and logs failures silently. Pre-existing.
- **Suspected fix**: `for await` accumulating into a new array, or `await Promise.all(files.map(...))` followed by a synchronous `.filter`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 45. temp-<uuid> user creation race produces unrecoverable duplicates

- **Component**: server (`src/server/authn.ts:128-176`)
- **Severity**: medium
- **Description**: The first-sign-in path is check-then-insert: `findOneByScan({email})` -> if no user, create `temp-<uuidV4()>`. Two simultaneous first sign-ins for the same email both insert distinct `temp-` rows, after which every subsequent `findOneByScan({email})` throws `expected single result document, not 2`. The recovery branch only handles the temp+real case (deletes temps when a real user exists); two-temps never converge. Pre-existing.
- **Suspected fix**: Wrap the check-and-insert in a Firestore transaction that re-reads the email index, or hash the email to a deterministic temp ID so concurrent inserts collide on the primary key.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 46. interceptWriteHeaders patched res.writeHead returns undefined

- **Component**: server (`src/server/headers.ts:9-37`)
- **Severity**: low
- **Description**: The replacement `res.writeHead` is an arrow function that ends with `return this as ServerResponse;`. In strict CJS module scope `this` is `undefined`, so the patched method returns `undefined` instead of the `Response`. The only current caller (`request-logger.ts:42`) uses `res.writeHead(500)` and discards the return value, so the bug is latent -- but any future caller that chains `res.writeHead(...).end(...)` or relies on the documented `ServerResponse` return type will crash silently. Pre-existing.
- **Suspected fix**: Convert the arrow to a regular function (so `this` binds to the response) or capture `res` in the closure and `return res;` explicitly.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 47. POST /api/projects/:u/:p loosely validates currVersion

- **Component**: server (`src/server/api.ts:254-265`)
- **Severity**: low
- **Description**: `if (!req.body.currVersion)` is falsy for `0` (unreachable today since versions start at 1) but truthy for `-1`. The `as number` cast is type-only, with no runtime conversion: a client sending `currVersion: "1"` produces `newVersion = projectVersion + 1 = "11"` (string concat), and the new version is stored as a string. Pre-existing.
- **Suspected fix**: Replace with `Number.isInteger(req.body.currVersion) && req.body.currVersion > 0` (or a parsed-then-validated `Number(...)` step) before using the value.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 48. ?project= redirect input validation absent

- **Component**: app (`src/app/App.tsx:228-231`)
- **Severity**: medium
- **Description**: `urlParams.get('project')` is passed directly to `<Redirect to={projectParam}>` with no shape validation. Same-origin browser policy prevents cross-origin redirects, but a value like `?project=//evil.com/page` (or `?project=/foo/../bar`) is a valid same-origin pushState path that fools the subsequent `/\/.*\/.*/` path-shape check at `App.tsx:235` and routes the user to a confusing or attacker-controlled path. Introduced in commit 48a1e10a (`#107`).
- **Suspected fix**: Validate against an allowlist before redirecting, e.g. `if (projectParam && /^\/[^/][^/]*\/[^/]+$/.test(projectParam))`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 49. env.js dotenv block is permanently dead code

- **Component**: app (`src/app/config/env.js:25-35`)
- **Severity**: low
- **Description**: `require('dotenv-expand')(require('dotenv').config(...))` always throws `MODULE_NOT_FOUND` because neither package is installed. The catch swallows it silently, and the comment misleadingly suggests the loading is conditional rather than universally broken. Result: any `.env*` files in `src/app/` are ignored. Introduced in commit d0bc3e37 (`#214`).
- **Suspected fix**: Add `dotenv` and `dotenv-expand` to `src/app/package.json` if `.env` loading is actually wanted, or delete the block (and the `dotenv` export from `paths.js`) if not.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 50. paths.js still carries CRA leftovers

- **Component**: app (`src/app/config/paths.js:16-90`)
- **Severity**: low
- **Description**: A large fraction of `paths.js` predates the rsbuild migration and is no longer consumed: `getPublicUrlOrPath`, `publicUrlOrPath`, the `homepage` lookup, `appPublic`, `yarnLockFile`, `appNodeModules`, `appPackageJson`, `appTsConfig`, `moduleFileExtensions`, and `resolveModule`. The actually-used exports are `appPath`, `appBuild`, `componentBuild`, `appHtml`, `appIndexJs`, `componentIndexJs`, and (conditionally) `dotenv`. Introduced in commit d0bc3e37 (`#214`).
- **Suspected fix**: Trim the file to the consumed exports; delete the helpers that backed the dead exports.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 51. Dev-server proxy whitelist references non-existent endpoints

- **Component**: app (`src/app/config/rsbuild/shared.config.js:96-110`)
- **Severity**: low
- **Description**: The dev-server proxy matches paths starting with `/api/`, `/auth/`, `/oauth/`, `/logout`, `/render/`, `/session`, and `/download/`, but only `/api/*` and `/session` actually exist on the backend (see `src/server/app.ts:252,260`, `src/server/authn.ts:234,238`). The dead matchers are noise; worse, several use `startsWith` and so prefix-collide: `/sessionAlpha` would be proxied even though only `/session` is a real route. Introduced in commit 9c7403e6 (rsbuild migration).
- **Suspected fix**: Reduce the matcher to the actual endpoints; switch `/session` from `startsWith` to equality (`pathname === '/session'`).
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 52. NewProject and NewUser submit handlers have no double-click guard

- **Component**: app (`src/app/NewProject.tsx:132-140`, `src/app/NewUser.tsx:90-98`)
- **Severity**: medium
- **Description**: Both `handleClose` methods schedule the create-project / create-user POST via `setTimeout` without any in-flight guard. A second click before the first POST resolves enqueues a duplicate request. The server does not enforce uniqueness, so the user can wind up with duplicate projects (or trigger a 409 cascade) on a slow network. Pre-existing.
- **Suspected fix**: Add a `submitting` boolean to component state, set it on the first click, disable the submit button while it is true, and reset it in the request's success/error handler.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 53. InnerApp constructor side effects fire twice under StrictMode; auth listener leaks

- **Component**: app (`src/app/App.tsx:97-115`)
- **Severity**: low
- **Description**: `connectAuthEmulator`, `onAuthStateChanged`, and the initial `setTimeout(this.getUserInfo)` all run in the `InnerApp` constructor. Under React 18+ StrictMode in dev these double-fire, and the unsubscribe handle returned by `onAuthStateChanged` is discarded so the listener leaks on unmount. In production `InnerApp` is the root component and never unmounts, so the leak is cosmetic; the dev double-fire is the more visible symptom. The pattern predates the React 18->19 upgrade and was ratified by commit 846c69eb (`#162`) without revisiting it.
- **Suspected fix**: Move the side effects into `componentDidMount`; store the `onAuthStateChanged` unsubscribe handle and call it from `componentWillUnmount`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 54. printUrls strips /index with substring replace instead of an anchored regex

- **Component**: app (`src/app/config/rsbuild/shared.config.js:91-93`)
- **Severity**: low
- **Description**: `url.replace('/index', '')` strips the first occurrence of `/index` anywhere in the URL. A URL like `http://localhost:3000/index/foo/index` would become `http://localhost:3000/foo/index`, and any project path containing the literal substring `/index` (e.g. a hypothetical `username/index-models`) would have it stripped from the printed URL. Introduced in commit 9c7403e6 (rsbuild migration).
- **Suspected fix**: `url.replace(/\/index$/, '')` so only a trailing `/index` is removed.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 55. Stale rsbuild-migration leftovers in src/app

- **Component**: app (`src/app/build-comparison-results.json`, `src/app/config/build-utils.js:28`)
- **Severity**: low
- **Description**: Two artifacts of the rsbuild migration are still present and harmless but misleading: (a) `build-comparison-results.json` is a one-shot rsbuild-vs-webpack size comparison snapshot from the migration, no longer consumed by anything; (b) `canReadAsset` in `build-utils.js` still excludes `service-worker.js` from the asset-readability check, but no service worker exists in the project. Introduced in commit 9c7403e6 (rsbuild migration) / d0bc3e37 (`#214`).
- **Suspected fix**: Delete both. If a service worker is reintroduced later, the exclusion can come back with it.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 57. Stale @system-dynamics/* import path in build-selection-map.test.ts

- **Component**: diagram (`src/diagram/tests/build-selection-map.test.ts:6-7`)
- **Severity**: low
- **Description**: The file imports types from `@system-dynamics/core/datamodel` and `@system-dynamics/core/common` instead of `@simlin/core/...` like the rest of the codebase. The test passes only because TypeScript erases type-only imports during compilation, so the dangling module specifier is never resolved. Inconsistent with house style and one rename away from a confusing test failure. Introduced in commit f932a7d1.
- **Suspected fix**: Change both imports to `@simlin/core/datamodel` / `@simlin/core/common`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 58. TS Dimension type loses parent/mappings/size/mapsTo from JSON round-trip

- **Component**: core (`src/core/datamodel.ts:1336-1356`)
- **Severity**: low
- **Description**: The TS `Dimension` interface only carries `name` and `subscripts`, while the Rust JSON `Dimension` has `size`, `mapsTo`, `mappings`, and `parent`. Today the deployed save paths bypass the TS `Project` for serialisation so the loss is invisible -- but `projectAttachData()` consults `dim.subscripts` to splay arrayed-variable Series data, and for indexed subdimensions the engine emits empty `elements`. So TS sees an empty `subscripts` array and silently fails to attach data for any variable arrayed over an indexed subdimension; the user would observe missing sparkline series. Pre-existing.
- **Suspected fix**: Extend the TS `Dimension` type with `parent`/`mappings` (and optionally `size`) and use them in `projectAttachData`; alternatively add a code comment documenting the limitation and the safe paths if a full fix is out of scope.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 59. disposeProject mutates projectChildren during for-of iteration

- **Component**: engine (`src/engine/src/worker-server.ts:454-473`)
- **Severity**: low
- **Description**: `disposeProject` iterates `for (const childHandle of children)` while `disposeModel` / `disposeSim` (called from inside the loop) mutate the same `Set` via `this.projectChildren` bookkeeping. ECMAScript's `Set` iterator handles deletion of the *current* element well, so the code happens to work today, but a future change that deletes a not-yet-visited entry would silently skip handles and leak resources. The post-loop `this.projectChildren.delete(workerHandle)` is also redundant once iteration finishes. Introduced in commit 1b516b63 (`#238`).
- **Suspected fix**: Snapshot the set with `[...children]` before iterating; remove the redundant post-loop `delete`.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-08

### 60. No CI feature matrix; a no-`file_io` build break propagated undetected

- **Component**: build / CI (`scripts/pre-commit`, `.github/workflows/`, `src/simlin-engine/Cargo.toml`)
- **Severity**: medium
- **Description**: Neither the pre-commit hook nor CI exercises a Cargo **feature matrix** for `simlin-engine`, so the entire class of feature-gating bugs is invisible to the canonical gate. Concretely: during Phase 7c of the Vensim macro epic a subagent added `src/simlin-engine/tests/metasd_macros.rs` (which calls the `file_io`-gated `load_dat`/`load_csv`) WITHOUT the mirroring `[[test]] name = "metasd_macros" required-features = ["file_io"]` entry in `src/simlin-engine/Cargo.toml`. This broke `cargo build -p simlin-engine --all-targets` and `cargo test -p simlin-engine --no-run` whenever `file_io` is *not* enabled (`error[E0425]`, `load_dat`/`load_csv` configured out). The pre-commit "Running Rust checks" step passed throughout because the workspace build pulls `file_io` into scope via cross-crate feature unification (`simlin-cli` requests `simlin-engine/file_io`), so the break survived multiple commits and was caught only by a later manual minimal-feature build. The Cargo.toml fix landed in commit `7cba4ad2` (which itself documents the "fragile cross-crate feature-unification accident" root cause), but the GAP — no feature-matrix gate — remains. simlin-engine features: `file_io`, `schema`, `ai_info`, `debug-derive` (default = `schema, ai_info, debug-derive`), plus `xmutil`. Secondary (workflow, not a separate item): this propagated because multi-agent plan execution structurally trusts the inherited baseline — each subagent verified only its own delta and several built *with* `file_io` and concluded the base was clean; subagents should verify the exact configuration they will later claim green BEFORE starting and STOP if the inherited baseline is already broken. The deterministic fix is the CI matrix.
- **Suspected fix**: Add a feature-matrix build/test step to the pre-commit hook and/or CI. At minimum `cargo build -p simlin-engine --all-targets --no-default-features` and `--all-features`. Optionally `cargo hack --feature-powerset` (bounded). Keep within the documented test-time budget ([docs/dev/rust.md#test-time-budgets](dev/rust.md)) — a couple of representative configurations (`--no-default-features`, `--all-features`), not the full powerset, is the pragmatic choice given the 3-minute workspace cap.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-15

### 61. CLAUDE.md files decaying from orientation docs into append-only changelogs

- **Component**: docs / process (`src/simlin-engine/CLAUDE.md`; the `project-claude-librarian` / `maintaining-project-context` skill)
- **Severity**: low
- **Description**: `src/simlin-engine/CLAUDE.md`'s "Last updated" header has degraded into a single ~2,500-word run-on paragraph spanning two unrelated epics (LTM arrays Phases 1-8 *and* macro Phases 1-7); the `db/ltm.rs` and `ltm_agg.rs` module bullets are multi-hundred-word single paragraphs that read like changelog archaeology rather than orientation. The file's stated purpose is current-state orientation ("maps where functionality lives"; "Keep this file up to date when adding, removing, or reorganizing modules"). Root cause: the `maintaining-project-context` / `project-claude-librarian` skill optimizes *per-delta* accuracy, not whole-file readability, so the aggregate readability degrades a little every epic and nothing ever compacts it. Related but distinct from #407 (which is about pruning verbatim code snippets from `docs/design-plans/`, not CLAUDE.md orientation decay).
- **Suspected fix**: Introduce an **epic-boundary CLAUDE.md compaction pass** (a distinct step from the librarian's per-delta updates) that relocates historical/changelog narrative into the now-committed design/implementation-plan docs (`docs/design-plans/`, `docs/implementation-plans/`) and trims each CLAUDE.md back to current-state orientation. Could be wired into `finishing-a-development-branch` or a dedicated skill.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-15

### 62. Implementation plans back-load all real-corpus validation into the final phase

- **Component**: process / workflow (the `writing-implementation-plans` / `execute-implementation-plan` skills; `docs/implementation-plans/`)
- **Severity**: medium
- **Description**: Implementation plans tend to defer all real-corpus numeric/structural validation to the last phase, which hides latent pre-existing blockers until most dependent work is already layered on top. Concretely, in the Vensim macro epic two latent defects — GH #554 (false `init->init` macro recursion) and a #554-class `DELAY N`->`delayn` variant — *blocked acceptance criteria* but only surfaced in Phase 7, the first phase to exercise the renamed-builtin / stdlib-module-collision path against real models (C-LEARN, metasd), after five phases of dependent work. A cheap corpus smoke run against the Phase 1-2 datamodel would have surfaced #554 far earlier and reduced rework risk. A second symptom: discovering a known *bug-class* of latent blockers mid-plan triggered serial stop-and-ask round-trips even though both #554-class decisions were the same class with the same answer.
- **Suspected fix**: Future implementation plans should **interleave an early, cheap real-corpus smoke validation** (run against the earliest phase that produces a runnable artifact) rather than back-loading all corpus validation into the last phase; and plans should **pre-authorize fixing a known bug-class of latent blockers** discovered during corpus validation, to avoid serial stop-and-ask round-trips for what is provably one decision. This is guidance for the plan-authoring skills, not a code change.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-15

### 63. Implementation-plan directory committed only at branch finish, untracked during execution

- **Component**: process / workflow (the `writing-implementation-plans` / `execute-implementation-plan` / `finishing-a-development-branch` skills; `docs/implementation-plans/`)
- **Severity**: low
- **Description**: For the Vensim macro epic, the design plan was committed at the branch's merge-base (`86cc7fcb`), but the 8-file `docs/implementation-plans/2026-05-13-macros/` directory (phase_01..07 + test-requirements.md) that drove 40+ commits stayed **untracked in git** for the entire execution and was only committed at the very end (`d44d32fa`, "doc: add Vensim macro support implementation plan") during the finishing step. Consequences: reviewers of intermediate commits had no in-repo plan to check the work against, and a lost or compacted session could have lost the plan entirely (it existed only in the working tree). Distinct from #407 (which is about pruning *stale* plan content post-merge, not about *when* the plan enters version control).
- **Suspected fix**: Commit the implementation plan at the **start** of execution — the `starting-an-implementation-plan` / `executing-an-implementation-plan` step should commit `docs/implementation-plans/<plan>/` before the first task subagent runs — rather than rescuing it at the `finishing-a-development-branch` step. Optionally add a guard in the execute step that refuses to start if the plan directory is untracked.
- **Owner**: unassigned
- **Last reviewed**: 2026-05-15

### 64. Doubled `ImportError{generic: ...}` wrapping in user-facing CLI error messages

- **Component**: simlin-engine (`src/simlin-engine/src/common.rs:239-252` `impl fmt::Display for Error`), simlin-cli (`src/simlin-cli/src/main.rs:217` `die!`)
- **Severity**: low
- **Description**: The CLI surfaces engine import/parse errors with a verbose, doubled debug-style wrapper. For a model whose `GET DIRECT *` data file is missing, the message is `model '...' error: ImportError{generic: Failed to parse MDL: import error: ImportError{generic: cannot resolve data file 'does_not_exist.csv': No such file or directory (os error 2)}}`. The `ImportError{generic: ...}` wrapper appears **twice** and the inner OS error is nested several layers deep. Root cause: `Error`'s `Display` renders `write!(f, "{}{{{}: {}}}", kind, self.code, details)` (-> `ImportError{generic: <details>}`); when an inner `Error` is stringified into an outer error's `details` (MDL import wrapping a data-resolution `Error`), the prefix nests verbatim and double-wraps. The CLI then prints the whole thing via `die!("model '{}' error: {}", &file_path, err)`. The essential signal (file name + OS cause) is present and correct, so this is cosmetic/developer-experience polish, not a correctness bug. **Pre-existing** (present before the Phase 5 C-LEARN-residual work; the `die!` formatting is unchanged at base SHA `55d62b74`). Distinct from #581 (assembly diagnostics silently discarded), #437 (CLI debug truncation), and #598 (WASM data-supply API).
- **Measure**: Run `simlin-cli` against an MDL model with an unresolvable `GET DIRECT *` reference (e.g. a sibling `does_not_exist.csv`) and inspect the `model '...' error:` line for the doubled `ImportError{generic:` prefix.
- **Suspected fix**: Flatten the `Error` `Display` so a nested import error does not re-emit the `ImportError{generic: ...}` prefix (e.g. detect/strip the wrapper when the `details` string is itself a rendered `Error`), or have the CLI walk and render a cleaner error chain rather than printing the raw nested `Display`.
- **Owner**: unassigned
- **Discovered**: C-LEARN residual Phase 5 code review (the AC5.3 missing-data-file diagnostic path exercises it)
- **Last reviewed**: 2026-05-20

### 65. Canvas boolean CanvasState not yet migrated onto the tagged-union interaction model

- **Component**: diagram (`src/diagram/drawing/Canvas.tsx`, `src/diagram/drawing/canvas-interaction.ts`)
- **Severity**: medium
- **Description**: (**Resolved** on the canvas-interaction-migration branch; see `docs/design-plans/2026-06-07-canvas-interaction-migration.md`.) The gating prerequisite was satisfied first: a reconciler-level gesture-sequence suite (`tests/canvas-gesture-harness.tsx` + `canvas-gestures-*.test.tsx`, commit 0852bde0) drives the real `<Canvas>` through PointerEvent down/move/up/cancel sequences asserting only on prop callbacks and rendered DOM. The migration (commit c2a391a5, fixed by 061320f2) then replaced the boolean modes and loose instance fields with a single `interaction: InteractionState` field. One deliberate deviation from the suspected fix below, adjudicated in adversarial review: element/arrowhead/source press resolution (`handleSetSelection`) and pointer-up resolution stay in the shell because they are geometry-dominated; they compose the module's pure helpers and construct union variants directly. The reducer's never-shell-driven `elementPointerDown` branch was deleted rather than kept as a parallel model (it was this entry's dual-source-of-truth complaint). `reduceInteraction` owns the empty-canvas, creation-tool, pinch enter/exit, and label-drag-start mode transitions. Canvas was subsequently converted to a function component (commit fe392c1c) with the same suite as the gate.
- **Suspected fix**: Complete the migration: replace the boolean `CanvasState` modes with a single `InteractionState` field, drive `handlePointerDown`/`handleSetSelection`/`handlePointerCancel` through `reduceInteraction` (applying its effects), and read the union in render instead of the booleans. This was deliberately deferred because doing it with behavior-preservation confidence needs end-to-end gesture-sequence tests that drive the Canvas through the React reconciler (pointer down -> move -> up), which do not yet exist; those tests should be written first. Continuous pinch/pan/momentum physics stay shell-internal by design.
- **Measure**: `grep -c 'isMovingArrow\|isMovingSource\|isMovingCanvas\|isDragSelecting\|isPinching\|isMovingLabel' src/diagram/drawing/Canvas.tsx` (drops to ~0 reads of the booleans when migrated).
- **Owner**: unassigned
- **Discovered**: Phase 3 Canvas refactor (diagram-react-refactor branch)
- **Last reviewed**: 2026-06-07

### 66. Dead constrain* movement methods on Canvas

- **Component**: diagram (`src/diagram/drawing/Canvas.tsx`)
- **Severity**: low
- **Description**: (**Resolved** in commit dd624051 on the canvas-interaction-migration branch.) The three methods and the `UpdateCloudAndFlow`/`UpdateFlow`/`UpdateStockAndFlows` imports only they used were deleted ahead of the tagged-union migration (#65) so the migration diff stayed small. They predated the unified `applyGroupMovement` (`group-movement.ts`), which owns live-preview movement geometry.
- **Suspected fix**: Delete the three methods (and any now-unused helper imports they alone pull in). Left in place during Phase 3 to keep that commit scoped to render purity + the interaction model.
- **Measure**: `grep -rn 'constrainFlowMovement\|constrainCloudMovement\|constrainStockMovement' src/diagram --include='*.ts' --include='*.tsx' | grep -v '/lib'` returns nothing.
- **Owner**: unassigned
- **Discovered**: Phase 3 Canvas refactor (diagram-react-refactor branch)
- **Last reviewed**: 2026-06-07

### 67. Connector-error recompute fires on viewport-only project rebuilds

- **Component**: diagram (`src/diagram/project-controller.ts` `attachConnectorErrors` / `updateVariableErrors`, called from `updateProject`)
- **Severity**: low
- **Description**: A sketch-connector/equation consistency check (`ProjectController.attachConnectorErrors`) runs from `updateVariableErrors`, which executes on EVERY project rebuild in `updateProject`. It fetches per-variable equation dependencies via the engine's `Model.getIncomingLinks(varName)` -- one async engine call per on-view aux/flow/stock variable (batched with `Promise.all`). Because viewport-only settles (pan/zoom) route through `queueViewUpdate` -> `updateProject` -> `updateVariableErrors`, this recompute also fires when connectors cannot possibly have changed. For large models (e.g. World3, hundreds of variables) on the Web Worker backend that is N `postMessage` round-trips per pan/zoom settle, adding avoidable latency to the settle's save. The feature is correct as-is; this is purely an efficiency refinement.
- **Suggested fix**: (a) Skip the connector recompute on non-content updates by distinguishing viewport-only rebuilds from content/structure edits, or (b) memoize per-variable incoming-links keyed on the serialized model content so pan/zoom settles reuse the prior result.
- **Owner**: unassigned
- **Discovered**: sketch-connector/equation consistency-check work on branch `editor-bug-burndown-2026-07`
- **Last reviewed**: 2026-07-02
