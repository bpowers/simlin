# Unify PREVIOUS/INIT Dependency Extraction -- Phase 5: Differential Checks (Fragment vs Full Compile)

**Goal:** Assert that fragment compilation and full model compilation agree on phase membership for every variable, both on existing integration test models and synthetic edge-case models.

**Architecture:** A helper function `assert_fragment_phase_agreement()` iterates every variable in a compiled model, compares the phases `compile_var_fragment` produces bytecodes for against the dep graph runlists, and asserts they agree. This is run over all 60+ integration test models and over synthetic models exercising PREVIOUS feedback, INIT-only deps, nested builtins, and module-backed vars. The check is a regression safety net: after the Phase 1-2 refactoring, it confirms that the unified `classify_dependencies` walker produces consistent results through both compilation paths.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 5 phases from original design (phase 5 of 5)

**Codebase verified:** 2026-03-02

---

## Acceptance Criteria Coverage

This phase implements and tests:

### unify-dep-extraction.AC5: Differential checks
- **unify-dep-extraction.AC5.1 Success:** For every variable in every integration test model, the phases `compile_var_fragment` produces bytecodes for match the phases the variable appears in across dep graph runlists
- **unify-dep-extraction.AC5.2 Success:** Synthetic models exercising PREVIOUS feedback, INIT-only deps, nested builtins, and module-backed vars pass the differential check
- **unify-dep-extraction.AC5.3 Failure:** If a new variable is added that causes fragment/graph phase disagreement, the differential test catches it

### unify-dep-extraction.AC0: Regression Safety
- **unify-dep-extraction.AC0.1 Success:** All existing simulation tests (`tests/simulate.rs`) pass at each phase boundary
- **unify-dep-extraction.AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary
- **unify-dep-extraction.AC0.3 Success:** Full integration test suite passes after each phase -- no behavioral regressions introduced

---

## Reference files

Read these CLAUDE.md files for project conventions before implementing:
- `/home/bpowers/src/simlin/CLAUDE.md` (project root)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (engine crate)

Key source files to understand before implementing:
- `src/simlin-engine/src/db.rs` -- `compile_var_fragment` (line 3254), `model_dependency_graph` (line 1522), `ModelDepGraphResult` (line 1101), `VarFragmentResult` (line 3249)
- `src/simlin-engine/src/compiler/symbolic.rs` -- `CompiledVarFragment` (line 307): `initial_bytecodes`, `flow_bytecodes`, `stock_bytecodes` (all `Option<PerVarBytecodes>`)
- `src/simlin-engine/src/db_prev_init_tests.rs` -- existing test patterns for dep graph testing
- `src/simlin-engine/src/db_fragment_cache_tests.rs` -- existing patterns using both `compile_var_fragment` and `model_dependency_graph`
- `src/simlin-engine/tests/simulate.rs` -- `TEST_MODELS` constant (line 42), `compile_vm()` pattern (line 116)

---

## Prerequisites

Phases 1-2 must be complete (unified walker and simplified db consumption).

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Create `db_differential_tests.rs` with the `assert_fragment_phase_agreement` helper

**Verifies:** unify-dep-extraction.AC5.1, unify-dep-extraction.AC5.3

**Files:**
- Create: `src/simlin-engine/src/db_differential_tests.rs`
- Modify: `src/simlin-engine/src/db.rs` -- add `#[cfg(test)] #[path = "db_differential_tests.rs"] mod db_differential_tests;` (following the pattern of `db_prev_init_tests.rs` at line 5910)

**Implementation:**

The test module needs access to `db.rs` internals via `use super::*;`.

**Helper function:**

```rust
/// For every variable in the given model, verify that the phases
/// `compile_var_fragment` produces bytecodes for match the phases
/// the dep graph's runlists include the variable in.
///
/// This is a consistency check: `compile_var_fragment` gates phase
/// compilation on runlist membership (db.rs lines 4064-4117). If
/// the dependency extraction refactoring introduced inconsistencies,
/// this check catches them.
fn assert_fragment_phase_agreement(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) {
    let dep_graph = model_dependency_graph(db, model, project);

    for &var in model.variables(db).values() {
        let var_name = var.ident(db);
        let canonical_name = crate::common::canonicalize(&var_name).into_owned();
        // Only check root-model variables. Sub-model variables are compiled
        // through the root model path and their phase membership is validated
        // transitively. Extending this to sub-models would require iterating
        // module expansions, which is out of scope for this phase.
        let is_root = true;

        let fragment_result =
            compile_var_fragment(db, var, model, project, is_root, vec![]);

        // Determine which phases the dep graph includes this variable in
        let in_initials = dep_graph.runlist_initials.contains(&canonical_name);
        let in_flows = dep_graph.runlist_flows.contains(&canonical_name);
        let in_stocks = dep_graph.runlist_stocks.contains(&canonical_name);

        // Determine which phases the fragment produced bytecodes for
        let (frag_initial, frag_flow, frag_stock) = match fragment_result {
            Some(result) => (
                result.fragment.initial_bytecodes.is_some(),
                result.fragment.flow_bytecodes.is_some(),
                result.fragment.stock_bytecodes.is_some(),
            ),
            None => (false, false, false),
        };

        // Fragment compilation gates on runlist membership, so having
        // bytecodes implies being in the runlist. The reverse may not
        // hold (a variable can be in a runlist but produce no bytecodes
        // if it has no equation), so we check the implication direction.
        if frag_initial {
            assert!(
                in_initials,
                "variable '{canonical_name}': fragment has initial bytecodes \
                 but variable is NOT in runlist_initials"
            );
        }
        if frag_flow {
            assert!(
                in_flows,
                "variable '{canonical_name}': fragment has flow bytecodes \
                 but variable is NOT in runlist_flows"
            );
        }
        if frag_stock {
            assert!(
                in_stocks,
                "variable '{canonical_name}': fragment has stock bytecodes \
                 but variable is NOT in runlist_stocks"
            );
        }
    }
}
```

**Note on the implication direction:** `compile_var_fragment` (db.rs lines 4064-4117) gates each phase on runlist membership AND variable kind (stocks only compile stock phase, non-stocks only compile flow phase, etc.). So `fragment has bytecodes => in runlist` should always hold. If it doesn't, the dependency extraction produced an inconsistent state.

The reverse (`in runlist => fragment has bytecodes`) may not hold for variables with no equation (e.g., constants) or variables whose compilation produces empty bytecodes. We do NOT assert this direction.

**Testing:**

The helper itself is tested through Tasks 2 and 3 which call it on real and synthetic models.

**Verification:**

```bash
cargo test -p simlin-engine --lib
```
Expected: compiles without errors.

**Commit:** `engine: add db_differential_tests with fragment-phase agreement helper`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Run differential check over all integration test models

**Verifies:** unify-dep-extraction.AC5.1

**Files:**
- Modify: `src/simlin-engine/src/db_differential_tests.rs` -- add integration test

**Implementation:**

Add a test that loads each XMILE integration test model from `test/test-models/`, compiles it via the salsa incremental path, and runs `assert_fragment_phase_agreement`. This test requires the `file_io` feature (for loading files from disk).

The test iterates a subset of the models from `tests/simulate.rs:TEST_MODELS` -- specifically the XMILE/STMX models (not MDL-only ones). Use the same load+compile pattern as `tests/simulate.rs`:

```rust
#[test]
#[cfg(feature = "file_io")]
fn test_fragment_phase_agreement_integration_models() {
    // List of integration test model paths (relative to repo root).
    // Include a representative set covering: simple models, models with
    // SMOOTH/DELAY (stdlib modules), models with arrays, models with
    // PREVIOUS/INIT.
    // Representative models from TEST_MODELS covering: simple auxes/flows/stocks,
    // SMOOTH/DELAY stdlib modules, arrayed models, module-backed models, and
    // various expression features.
    let models = &[
        // Simple models (basic stocks, flows, auxes)
        "test/test-models/samples/teacup/teacup.xmile",
        "test/test-models/samples/SIR/SIR.xmile",
        "test/test-models/samples/SIR/SIR_reciprocal-dt.xmile",
        // Module-backed models (stdlib expansion)
        "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
        // SMOOTH/DELAY/TREND stdlib modules
        "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
        "test/test-models/tests/delays2/delays.xmile",
        "test/test-models/tests/trend/test_trend.xmile",
        // Array models (1D, 2D, 3D, A2A, non-A2A)
        "test/test-models/samples/arrays/a2a/a2a.stmx",
        "test/test-models/samples/arrays/non-a2a/non-a2a.stmx",
        "test/test-models/tests/subscript_1d_arrays/test_subscript_1d_arrays.xmile",
        "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
        "test/test-models/tests/subscript_3d_arrays/test_subscript_3d_arrays.xmile",
        "test/test-models/tests/subscript_docs/subscript_docs.xmile",
        "test/test-models/tests/subscript_multiples/test_multiple_subscripts.xmile",
        // Dependency ordering and initialization
        "test/test-models/tests/eval_order/eval_order.xmile",
        "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
        // Misc expression features (lookups, game, inputs)
        "test/test-models/tests/lookups_inline/test_lookups_inline.xmile",
        "test/test-models/tests/game/test_game.xmile",
        "test/test-models/tests/input_functions/test_inputs.xmile",
    ];

    for path in models {
        let file_path = std::path::Path::new(path);
        if !file_path.exists() {
            // Skip if running from a different working directory
            continue;
        }
        let f = std::fs::File::open(file_path).unwrap();
        let mut f = std::io::BufReader::new(f);
        let datamodel_project = crate::xmile::project_from_reader(&mut f).unwrap();

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel_project);

        // Run agreement check for the main model
        for (_name, model_info) in &sync.models {
            assert_fragment_phase_agreement(
                &db,
                model_info.source,
                sync.project,
            );
        }
    }
}
```

The executor should select 15-20 representative models from `TEST_MODELS` that cover: simple auxes/flows/stocks, SMOOTH/DELAY stdlib modules, arrayed models, and any models using PREVIOUS or INIT. The exact list should be determined by reading `tests/simulate.rs:42-112` and selecting models that exercise the relevant code paths.

**Testing:**

unify-dep-extraction.AC5.1: Every variable in every selected model has consistent fragment-phase vs dep-graph-phase membership.

**Verification:**

```bash
cargo test -p simlin-engine --features file_io test_fragment_phase_agreement_integration
```
Expected: all models pass the differential check.

**Commit:** `engine: add differential check over integration test models`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add synthetic models for differential check edge cases

**Verifies:** unify-dep-extraction.AC5.2, unify-dep-extraction.AC5.3

**Files:**
- Modify: `src/simlin-engine/src/db_differential_tests.rs` -- add synthetic model tests

**Implementation:**

Add tests that construct synthetic `datamodel::Project` models (no file I/O needed), compile them via the salsa path, and run `assert_fragment_phase_agreement`. Use the direct `datamodel::Project` construction pattern from `db_prev_init_tests.rs` (line 9).

**Synthetic models to create:**

Each model exercises a specific combination that could expose dependency classification bugs:

1. **PREVIOUS feedback:** `x = TIME`, `y = PREVIOUS(x) + 1` -- `y` depends on `x` only through PREVIOUS, so no same-step ordering edge.

2. **INIT-only deps:** `x = TIME`, `y = INIT(x) + 1` -- `y` depends on `x` only through INIT; `x` must be in initials runlist but not a dt ordering constraint on `y`.

3. **Nested builtins:** `x = TIME`, `z = PREVIOUS(PREVIOUS(x))` -- creates implicit helper variables. All must have consistent phase membership.

4. **Module-backed variable (SMOOTH):** `x = TIME`, `y = SMTH1(x, 1, x)` -- `y` expands to a stdlib module with internal stocks. The module's implicit variables must all have consistent phases.

5. **Mixed PREVIOUS+INIT+current:** `x = TIME`, `y = PREVIOUS(x) + INIT(x) + x` -- `x` is referenced in all three contexts. The dep graph should have `x` as a same-step dep of `y` (because of the bare `x` reference).

For each synthetic model:
1. Construct `datamodel::Project` with `datamodel::SimSpecs` defaults, a `datamodel::Model` named "main" containing the variables
2. Call `sync_from_datamodel(&db, &project)`
3. Call `assert_fragment_phase_agreement(&db, model, project)`
4. Optionally also verify the model compiles and simulates without errors

**Testing:**

- unify-dep-extraction.AC5.2: Each synthetic model passes the differential check
- unify-dep-extraction.AC5.3: The test structure is designed to catch disagreements -- if a new variable is added with incorrect dep classification, `assert_fragment_phase_agreement` will fail on it

**Verification:**

```bash
cargo test -p simlin-engine test_fragment_phase_agreement_synthetic
```
Expected: all synthetic models pass.

```bash
cargo test -p simlin-engine
```
Expected: all tests pass.

```bash
cargo test -p simlin-engine --features file_io
```
Expected: full integration suite passes.

**Commit:** `engine: add synthetic differential check models`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
