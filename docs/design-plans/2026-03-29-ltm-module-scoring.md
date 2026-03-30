# LTM Module Composite Scoring Design

## Summary

This design replaces the dual-path LTM (Loops That Matter) implementation with a unified architecture where every model -- whether it is the top-level user model, a stdlib module like SMOOTH or DELAY, or a user-defined sub-model -- receives identical LTM treatment during compilation. Today, LTM scoring for modules relies on a separate, test-only code path (`with_ltm()` / `with_ltm_all_links()`) that injects synthetic variables outside the salsa incremental compilation pipeline, using a distinct `ilink` naming prefix for sub-model internal link scores and a pre-cached `CompositePortMap` for stdlib modules. This design eliminates that bifurcation: a single `model_ltm_variables` tracked function generates link scores, loop scores, and composite scores for any model, and the `is_root` gates in `compute_layout` and `assemble_module` are removed so that LTM instrumentation flows transitively through all module instantiations when the project has `ltm_enabled` set.

The core architectural insight is that a module's composite score serves as its "LTM interface," summarizing how strongly an input propagates through the module to affect its output. Parent models reference these composites via the existing interpunct (`module·composite_var`) resolution mechanism, so no compiler changes are needed. Nesting works recursively: a user-defined module containing a SMOOTH gets composite scores that reference the SMOOTH's composites, and the root model references the user module's composites. The migration proceeds in seven phases: unify variable generation, wire LTM into sub-model assembly, validate end-to-end simulation, migrate existing non-module tests to the salsa/VM path, migrate module tests, delete the now-unused test-only code, and update documentation.

## Definition of Done

Feedback loops that pass through modules -- both stdlib (SMOOTH, DELAY, TREND) and user-defined modules with internal stocks, including nested modules at arbitrary depth -- are properly scored by LTM in the production salsa/incremental compilation path. Specifically:

1. **Module composite scoring works end-to-end in production**: `assemble_module` compiles internal LTM instrumentation (ilink, pathway, composite scores) into stdlib and user-defined sub-models. `link_score_equation_text` generates correct link score equations for module-involved links. `model_ltm_synthetic_variables` includes loops through modules instead of filtering them out. Both exhaustive and discovery modes handle modules. Nested modules (e.g., user module containing SMOOTH) are handled recursively at arbitrary depth.

2. **All LTM tests run on the salsa/VM path**: The 17 tests in `simulate_ltm.rs` plus related tests in `db_ltm_tests.rs`, `simulate.rs`, and `layout.rs` all use `compile_project_incremental` + VM. No tests use `with_ltm()`, `with_ltm_all_links()`, or the AST interpreter for LTM.

3. **Test-only LTM code is deleted**: `with_ltm()`, `with_ltm_all_links()`, `generate_ltm_variables()`, `generate_ltm_variables_all_links()`, `inject_ltm_vars()`, `compute_composite_ports()`, the test-gated `generate_link_score_variables()`, and related functions are removed from `ltm_augment.rs` and `project.rs`.

4. **User-defined and nested module coverage**: Tests exercise LTM on models with user-defined modules and nested module structures (e.g., `modules_hares_and_foxes` with a SMOOTH inside a sub-model).

**Out of scope**: Array variable support for LTM (future work).

## Acceptance Criteria

### ltm-module-scoring.AC1: Module composite scoring in production
- **ltm-module-scoring.AC1.1 Success:** SMOOTH in feedback loop produces non-zero link scores via `compile_project_incremental` + VM
- **ltm-module-scoring.AC1.2 Success:** DELAY in feedback loop produces non-zero link scores and composite scores
- **ltm-module-scoring.AC1.3 Success:** User-defined module with internal stocks gets link/loop/composite scores
- **ltm-module-scoring.AC1.4 Success:** Nested module (user module containing SMOOTH) produces composite scores at both nesting levels
- **ltm-module-scoring.AC1.5 Success:** Exhaustive mode detects loops passing through modules and generates loop/relative scores
- **ltm-module-scoring.AC1.6 Success:** Discovery mode finds loops through modules via post-simulation strongest-path search
- **ltm-module-scoring.AC1.7 Edge:** Model with passthrough module (no internal stocks) compiles with LTM without error; module gets no LTM vars
- **ltm-module-scoring.AC1.8 Edge:** Model with multiple instances of same stdlib module gets independent composite scores per instance

### ltm-module-scoring.AC2: All LTM tests on salsa/VM path
- **ltm-module-scoring.AC2.1 Success:** All 9 non-module LTM tests in `simulate_ltm.rs` pass on salsa/VM path
- **ltm-module-scoring.AC2.2 Success:** All 8 module LTM tests in `simulate_ltm.rs` pass on salsa/VM path
- **ltm-module-scoring.AC2.3 Success:** Golden data validation (`logistic_growth_ltm` output) unchanged after migration
- **ltm-module-scoring.AC2.4 Failure:** No test in the codebase imports or calls `with_ltm()` or `with_ltm_all_links()`

### ltm-module-scoring.AC3: Test-only code deleted
- **ltm-module-scoring.AC3.1:** `with_ltm()`, `with_ltm_all_links()` removed from `project.rs`
- **ltm-module-scoring.AC3.2:** `generate_ltm_variables()`, `generate_ltm_variables_all_links()`, `inject_ltm_vars()`, `compute_composite_ports()`, `generate_link_score_variables()` removed from `ltm_augment.rs`
- **ltm-module-scoring.AC3.3:** No dead-code warnings from `cargo build --workspace`
- **ltm-module-scoring.AC3.4:** `testing` feature no longer gates any LTM-specific code paths

### ltm-module-scoring.AC4: User-defined and nested module coverage
- **ltm-module-scoring.AC4.1 Success:** Integration test exercises LTM on `modules_hares_and_foxes` (or equivalent user-defined module model)
- **ltm-module-scoring.AC4.2 Success:** Integration test exercises LTM on a model where a user-defined module contains a SMOOTH call (nested module)
- **ltm-module-scoring.AC4.3 Success:** Composite scores from nested modules are accessible via chained interpunct notation
- **ltm-module-scoring.AC4.4 Edge:** `ilink` naming prefix eliminated; sub-model link scores use `link_score` prefix, namespaced by interpunct

## Glossary

- **LTM (Loops That Matter)**: A feedback loop dominance analysis method that scores causal links and loops in a system dynamics model to determine which feedback loops are most influential at each point in a simulation run.
- **Composite score**: A synthetic variable that summarizes how strongly a module's input propagates through its internal structure to affect its output. Acts as the "LTM interface" of a module.
- **Link score**: A synthetic variable measuring the causal influence of one variable on another at each timestep, computed via ceteris paribus re-evaluation (holding all other inputs at their previous values).
- **Loop score**: The product of link scores around a feedback loop, representing the loop's overall strength.
- **Relative loop score**: A loop score normalized against all other loops in the same cycle partition.
- **Cycle partition**: A group of stocks connected by feedback paths (a strongly connected component in the stock-to-stock reachability graph). Relative loop scores are only compared within the same partition.
- **Synthetic variable**: A variable injected into the model that does not appear in the user's original definition. LTM generates these to compute scores using the same simulation machinery as regular variables.
- **Salsa**: An incremental computation framework. Tracked functions are memoized and automatically recomputed only when their inputs change.
- **Tracked function**: A salsa function whose results are cached and incrementally maintained (e.g., `model_ltm_variables`, `link_score_equation_text`, `compute_layout`).
- **VM (bytecode VM)**: The compiled execution engine that runs simulations from bytecode produced by `assemble_module`. The alternative is the AST interpreter, which LTM tests are being migrated away from.
- **Stdlib module**: A built-in sub-model implementing a standard SD function (SMOOTH via `smth1`, DELAY via `delay1`, TREND via `trend`). These contain internal stocks and flows.
- **Interpunct notation**: The `module_instance·variable_name` syntax (using Unicode interpunct `·`) for referencing a variable inside a module instance from the parent model.
- **`ilink` prefix**: The current naming prefix (`$⁚ltm⁚ilink⁚`) for sub-model internal link scores, distinct from root-level `$⁚ltm⁚link_score⁚`. This design eliminates the distinction.
- **`SourceProject`**: The salsa input struct representing a project's configuration, including the `ltm_enabled` flag.
- **Discovery mode**: LTM mode for large models: generate link scores for all edges, simulate, then find important loops post-simulation via strongest-path search.
- **Exhaustive mode**: LTM mode that enumerates all feedback loops via Johnson's algorithm before simulation.
- **Passthrough module**: A module with no internal stocks or feedback loops. LTM analysis produces no synthetic variables for such modules.
- **`is_root` gate**: The conditional `is_root && project.ltm_enabled(db)` that currently restricts LTM compilation to the top-level model. Removing this gate is central to the design.
- **Pathway score**: Product of link scores along one specific internal path from a module input to its output. The composite score selects the strongest pathway at each timestep.
- **Golden data validation**: Verifying simulation outputs match known-good numerical results from a previous run.

## Architecture

### Core Insight: Uniform Model Treatment

Every model receives identical LTM treatment regardless of where it sits in the nesting hierarchy. There is no distinction between "root model LTM" and "sub-model LTM." Given any model M with `ltm_enabled` on the enclosing project:

1. **Structural analysis**: Build causal graph, detect loops, compute cycle partitions
2. **Link scores**: For each causal edge in detected loops (exhaustive mode) or all edges (discovery mode), generate a `$⁚ltm⁚link_score⁚{from}→{to}` synthetic variable
3. **Loop scores**: For each detected loop, generate `$⁚ltm⁚loop_score⁚{id}` and `$⁚ltm⁚rel_loop_score⁚{id}`
4. **Port composite scores**: For each input port with a causal pathway to the output, generate pathway scores (`$⁚ltm⁚path⁚{port}⁚{idx}`) and a composite score (`$⁚ltm⁚composite⁚{port}`) that selects the strongest pathway at each timestep

This applies uniformly to the main model, stdlib modules (SMOOTH, DELAY, TREND), and user-defined modules. SMTH1 has an internal feedback loop (`smoothed → flow → smoothed`) -- it gets link scores and loop scores for that loop, plus composite scores for its `input` port.

### Module Composite Score as LTM Interface

The composite score is the "LTM interface" of a module, analogous to how the output port is its data interface. When the parent model needs a link score for `x → smooth_instance`, it references `smooth_instance·$⁚ltm⁚composite⁚input` via the existing interpunct (`·`) resolution mechanism. This summarizes "how strongly does input propagate through this module to affect its output."

For nested modules (user module U containing SMOOTH S): S gets its own composite scores. U's link scores reference S's composites via `s_instance·$⁚ltm⁚composite⁚input`. The parent references U's composites via `u_instance·$⁚ltm⁚composite⁚{port}`. Each level only knows about its immediate children.

### Naming Convention Unification

The current codebase uses two naming prefixes: `$⁚ltm⁚link_score⁚` for root-model link scores and `$⁚ltm⁚ilink⁚` for sub-model internal link scores. With uniform treatment, the `ilink` distinction is eliminated -- every model uses `$⁚ltm⁚link_score⁚`. Sub-model scores are naturally namespaced by interpunct resolution (the parent sees them as `module·$⁚ltm⁚link_score⁚...`), so the discovery parser's prefix match on `$⁚ltm⁚link_score⁚` at root scope does not accidentally capture sub-model internals.

### LTM Flag Threading

`ltm_enabled` remains on `SourceProject` (salsa input). The current `is_root &&` gate in `compute_layout` and `assemble_module` is removed so LTM flows transitively to all module instantiations when the project has LTM enabled. This matches the conceptual model: toggling LTM on a project flows through every model in the project, including stdlib sub-models.

### Key Functions (After Unification)

| Function | Responsibility |
|----------|---------------|
| `model_ltm_variables(db, model, project)` | Unified LTM generation for any model: loop detection, link/loop/relative scores, port pathway analysis, composite scores |
| `link_score_equation_text(db, link_id, model, project)` | Per-link tracked function for incremental caching. Generates composite references for module-involved links instead of returning `None` |
| `compute_layout` | Allocates slots for LTM vars in every model (not just root) when `ltm_enabled` |
| `assemble_module` | Compiles LTM fragments for every model (not just root) when `ltm_enabled` |

### Data Flow

```
Project (ltm_enabled = true)
  │
  ├─ Main Model
  │   ├─ model_ltm_variables() → link scores, loop scores, relative scores
  │   ├─ link_score_equation_text() for module links → references module·$⁚ltm⁚composite⁚port
  │   └─ compute_layout + assemble_module include LTM vars
  │
  ├─ Stdlib Sub-Model (e.g., smth1)
  │   ├─ model_ltm_variables() → internal link/loop/relative scores + composite scores
  │   └─ compute_layout + assemble_module include LTM vars
  │
  └─ User Sub-Model (e.g., hares)
      ├─ model_ltm_variables() → internal link/loop/relative scores + composite scores
      ├─ (if contains SMOOTH) link scores reference smooth·$⁚ltm⁚composite⁚input
      └─ compute_layout + assemble_module include LTM vars
```

## Existing Patterns

The design follows existing patterns in the codebase:

**Salsa tracked function decomposition** (`src/simlin-engine/src/db.rs`): The per-link tracked function pattern (`link_score_equation_text`, `module_ilink_equation_text`) is already established. The unified `model_ltm_variables` follows the same pattern as `model_causal_edges`, `model_loop_circuits`, and `model_cycle_partitions` -- all per-model tracked functions that compose into higher-level results.

**Module interpunct resolution** (`src/simlin-engine/src/compiler/context.rs`): The `module·var` notation is already fully supported for regular variables. LTM composite score references (`module·$⁚ltm⁚composite⁚port`) use the same mechanism with no compiler changes needed.

**LTM equation compilation** (`src/simlin-engine/src/db_ltm.rs`): `compile_ltm_equation_fragment` and `compile_ltm_implicit_var_fragment` already handle mini-layout construction with module dependencies. The sub-model LTM compilation extends this by using the same compilation path -- the mini-layout includes sub-model LTM variable slots.

**Divergence from existing patterns**: The current design has separate `model_ltm_synthetic_variables` (root) and `module_ltm_synthetic_variables` (sub-model) with different naming conventions (`link_score` vs `ilink`). This design replaces both with a single `model_ltm_variables` function and a unified naming convention. This divergence simplifies the codebase by eliminating special-casing.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Unify LTM Variable Generation

**Goal:** Replace `model_ltm_synthetic_variables` and `module_ltm_synthetic_variables` with a single `model_ltm_variables` tracked function that generates appropriate LTM vars for any model.

**Components:**
- Unified `model_ltm_variables` in `src/simlin-engine/src/db.rs` -- generates link scores, loop scores, relative loop scores for detected loops; pathway and composite scores for input ports with causal pathways to output. Uses `$⁚ltm⁚link_score⁚` naming uniformly (no `ilink` prefix).
- Updated `link_score_equation_text` in `src/simlin-engine/src/db.rs` -- restored module link handling: generates composite references (`module·$⁚ltm⁚composite⁚port`) for module-involved links instead of returning `None`. Removes the loop filtering that drops module-containing loops.

**Dependencies:** None (first phase)

**Done when:** The unified function produces correct LTM synthetic variable lists for simple models (no modules), stdlib modules (smth1, delay1), and user-defined modules. Existing `db_tests.rs` and `db_stdlib_ports_tests.rs` tests adapted and passing. The `link_score_equation_text` function generates equations for module-involved links.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Wire LTM Into Sub-Model Assembly

**Goal:** Every model (not just root) gets LTM vars compiled into its bytecode when `ltm_enabled`.

**Components:**
- `compute_layout` in `src/simlin-engine/src/db.rs` -- remove the `is_root &&` gate from LTM section. When `ltm_enabled`, allocate slots for LTM vars in every model's layout.
- `assemble_module` in `src/simlin-engine/src/db.rs` -- remove the `is_root &&` gate from the LTM compilation pass. When `ltm_enabled`, compile LTM fragments for every model.
- `compile_ltm_equation_fragment` in `src/simlin-engine/src/db_ltm.rs` -- extend mini-layout construction to include sub-model LTM variable slots when LTM equations reference module composites.

**Dependencies:** Phase 1 (unified generation function)

**Done when:** A model containing SMOOTH compiles with `ltm_enabled=true` via `compile_project_incremental` without panics or layout resolution failures. Internal LTM vars (link scores, pathway scores, composite scores) exist at correct offsets in the compiled sub-model bytecode. New integration tests verify compilation succeeds for models with stdlib modules, user-defined modules, and nested modules (user module containing SMOOTH).
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: End-to-End Module LTM Simulation

**Goal:** LTM-augmented models with modules produce correct simulation results via the VM.

**Components:**
- VM execution correctness in `src/simlin-engine/src/vm.rs` -- no changes expected; composite scores are just variable references resolved through existing mechanisms. This phase validates the full pipeline.
- New integration tests in `src/simlin-engine/tests/simulate_ltm.rs` -- test LTM simulation with stdlib modules (SMOOTH in feedback loop), user-defined modules (hares_and_foxes variant), and nested modules (user module containing SMOOTH). Both exhaustive and discovery modes.

**Dependencies:** Phase 2 (assembly wiring)

**Done when:** VM simulation of LTM-augmented models with modules produces non-zero link scores, loop scores, and composite scores at expected offsets. Discovery mode finds loops through modules. New tests pass on both exhaustive and discovery mode paths.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Migrate Non-Module LTM Tests

**Goal:** Move the 9 LTM tests that don't involve modules from the test-only path to the salsa/VM path.

**Components:**
- Test migration in `src/simlin-engine/tests/simulate_ltm.rs` -- rewrite `simulates_population_ltm`, `discovery_logistic_growth_finds_both_loops`, `discovery_cross_validates_with_exhaustive`, `discovery_arms_race_3party`, `discovery_decoupled_stocks`, `hero_culture_loop_sign_continuity`, `test_independent_subsystems_partitioned_relative_scores`, `test_coupled_two_stock_single_partition`, `test_discovery_independent_subsystems`, and `test_arms_race_single_partition` to use `compile_project_incremental` + VM instead of `Project::from` + `with_ltm` + `Simulation::new`.

**Dependencies:** Phase 2 (LTM vars compile correctly)

**Done when:** All 9+ non-module LTM tests pass using only the salsa/VM path. No test uses `with_ltm()` or `with_ltm_all_links()` for non-module models. Numerical results match pre-migration values (golden data validation unchanged).
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Migrate Module LTM Tests

**Goal:** Move the 8 module-containing LTM tests from the test-only path to the salsa/VM path.

**Components:**
- Test migration in `src/simlin-engine/tests/simulate_ltm.rs` -- rewrite `test_smooth_with_initial_value_ltm`, `test_smooth_goal_seeking_ltm`, `test_smooth_model_discovery_mode`, `test_discovery_ilink_not_in_search_graph`, `test_multiple_smooth_instances`, `test_internal_smooth_loop_not_in_parent`, `test_module_output_multi_input_link_score_magnitude` to use `compile_project_incremental` + VM. Remove the TODO comments about missing VM cross-checks.
- Update `test_discovery_ilink_not_in_search_graph` for the unified naming (no more `ilink` prefix; test the interpunct-based namespace separation instead).

**Dependencies:** Phase 3 (module LTM simulation works end-to-end)

**Done when:** All 8 module-containing LTM tests pass using only the salsa/VM path. No LTM test in the entire codebase uses `with_ltm()`, `with_ltm_all_links()`, or the AST interpreter.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Delete Test-Only LTM Code

**Goal:** Remove all test-gated LTM code that is no longer called.

**Components:**
- `src/simlin-engine/src/project.rs` -- delete `with_ltm()`, `with_ltm_all_links()`, and `abort_if_arrayed()` (only used by the LTM test path)
- `src/simlin-engine/src/ltm_augment.rs` -- delete `generate_ltm_variables()`, `generate_ltm_variables_all_links()`, `generate_ltm_variables_inner()`, `generate_link_score_variables()`, `generate_module_internal_ltm_variables()`, `compute_composite_ports()`, `generate_module_link_score_equation()`, `generate_module_input_link_score_equation()`, `inject_ltm_vars()` (if in this file), and their associated unit tests that test the old path
- `src/simlin-engine/src/db.rs` -- delete `get_stdlib_composite_ports()`, `ensure_stdlib_composite_ports_initialized()` (already test-gated), and `db_stdlib_ports_tests.rs` (tests the old composite port cache)
- Remove `CompositePortMap` type alias if no longer referenced

**Dependencies:** Phase 5 (all tests migrated)

**Done when:** No function gated with `#[cfg(any(test, feature = "testing"))]` exists that is specific to the old LTM augmentation path. `cargo build --workspace` and `cargo test --workspace` succeed with no warnings about dead code. The `testing` feature no longer gates any LTM-specific code paths.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Documentation Update

**Goal:** Update documentation to reflect the unified architecture.

**Components:**
- `docs/design/ltm--loops-that-matter.md` -- update "Module Links" section to describe the unified model (no black-box formula needed; every model gets uniform treatment). Remove references to `ilink` prefix. Update "Composite Port Pre-computation" to reflect that composite ports are computed per-model by `model_ltm_variables`, not pre-cached in a global `CompositePortMap`. Update test coverage section.
- `src/simlin-engine/CLAUDE.md` -- update module map entries for `db.rs`, `db_ltm.rs`, `ltm_augment.rs`, `project.rs` to reflect deleted functions and the new unified `model_ltm_variables`. Remove references to the test-only `with_ltm()` path.
- `src/libsimlin/CLAUDE.md` -- remove references to `ensure_stdlib_composite_ports_initialized` if any remain.

**Dependencies:** Phase 6 (code deletion complete)

**Done when:** Documentation accurately reflects the current codebase. No documentation references deleted functions or the test-only LTM path.
<!-- END_PHASE_7 -->

## Additional Considerations

**Discovery mode parser update**: The discovery parser in `ltm_finding.rs` (`parse_link_offsets`) currently matches `$⁚ltm⁚link_score⁚` prefix to find root-level link scores. With unified naming, sub-model link scores are accessed from root as `module·$⁚ltm⁚link_score⁚...` and appear in `results.offsets` with the interpunct prefix. The parser's existing prefix match naturally excludes these because they start with the module instance name, not `$⁚ltm⁚`. Verify this assumption during Phase 3 testing.

**Salsa cache invalidation**: Toggling `ltm_enabled` on `SourceProject` already invalidates all dependent salsa queries. Adding LTM vars to sub-model layouts doesn't introduce new invalidation concerns -- when LTM is toggled, everything recompiles.

**Module classification**: The LTM analysis for a model is a no-op when the model has no causal edges (e.g., passthrough modules without stocks). `model_ltm_variables` returns empty results for such models, so no special "passthrough" classification is needed -- it falls out naturally from the uniform treatment.

**PREVIOUS/INIT intrinsics**: The existing `LoadPrev`/`LoadInitial` opcodes handle `PREVIOUS()` and `INIT()` calls in LTM equations. LTM equations that reference module variables use `PREVIOUS(module_var)` which rewrites through a helper aux (handled by `parse_ltm_var_with_ids` and `ltm_module_idents`). This mechanism already works and requires no changes for the unified model.
